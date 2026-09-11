use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
struct NotificationCapture {
    persistence: PrivatePersistence,
    scope: SessionScope,
    cancelled: Arc<AtomicBool>,
}
impl NotificationCapture {
    fn new(context: &AppContext) -> Option<Self> {
        let persistence = context.store.borrow().private_persistence.clone()?;
        let scope = context.backend.as_ref()?.api.session()
            .scope_for_user(persistence.lease().namespace.user_public_id())?;
        if scope.auth_epoch != persistence.lease().auth_epoch { return None; }
        let capture = Self { persistence, scope, cancelled: Arc::new(AtomicBool::new(false)) };
        capture.is_current(context).then_some(capture)
    }
    fn binding_matches(&self, context: &AppContext) -> bool {
        // Pure lease comparison is safe inside a short completion; same_binding
        // and is_current acquire admission and must only run outside that lock.
        context.store.borrow().private_persistence.as_ref()
            .is_some_and(|current| current.lease() == self.persistence.lease())
    }
    fn namespace_is_current(&self, context: &AppContext) -> bool {
        !NOTIFICATION_SHUTDOWN.with(|closed| closed.get())
            && !self.cancelled.load(Ordering::Acquire)
            && self.binding_matches(context)
            && context.active_namespace.lock().ok()
                .is_some_and(|active| active.as_ref() == Some(self.persistence.lease()))
            && self.persistence.is_current()
    }
    fn is_current(&self, context: &AppContext) -> bool {
        self.namespace_is_current(context)
            && context.backend.as_ref().is_some_and(|backend|
                backend.api.session().is_scope_current(&self.scope))
    }
}

struct NotificationThread {
    lease: NamespaceLease,
    cancelled: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}
thread_local! {
    // UI-thread ownership: completed handles are reaped by the poller. Handles
    // surviving event-loop exit remain here until the explicit shutdown drain.
    static NOTIFICATION_THREADS: RefCell<Vec<NotificationThread>> = const { RefCell::new(Vec::new()) };
    static NOTIFICATION_SHUTDOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NOTIFICATION_JOIN_FAILED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn reap_notification_workers() -> bool {
    let ready = NOTIFICATION_THREADS.with(|threads| {
        let mut threads = threads.borrow_mut();
        let mut ready = Vec::new();
        let mut index = 0;
        while index < threads.len() {
            if threads[index].handle.is_finished() { ready.push(threads.remove(index)); }
            else { index += 1; }
        }
        ready
    });
    let mut joined = true;
    for worker in ready { joined &= worker.handle.join().is_ok(); }
    if !joined { NOTIFICATION_JOIN_FAILED.with(|failed| failed.set(true)); }
    !NOTIFICATION_JOIN_FAILED.with(|failed| failed.get())
}
fn notification_worker_pending(capture: &NotificationCapture) -> bool {
    NOTIFICATION_THREADS.with(|threads| threads.borrow().iter()
        .any(|worker| Arc::ptr_eq(&worker.cancelled, &capture.cancelled)))
}
fn join_notification_workers() -> std::result::Result<(), String> {
    let handles = NOTIFICATION_THREADS.with(|threads| std::mem::take(&mut *threads.borrow_mut()));
    let mut failed = NOTIFICATION_JOIN_FAILED.with(|failed| failed.get());
    for worker in handles { failed |= worker.handle.join().is_err(); }
    if failed { NOTIFICATION_JOIN_FAILED.with(|failed| failed.set(true)); }
    if failed { Err("notification worker panicked".into()) } else { Ok(()) }
}
pub(super) fn cancel_notification_workers_for_retirement(lease: &NamespaceLease) {
    NOTIFICATION_THREADS.with(|threads| {
        for worker in threads.borrow().iter().filter(|worker| &worker.lease == lease) {
            worker.cancelled.store(true, Ordering::Release);
        }
    });
}
/// Call on the owning UI thread after its event loop ends, outside every
/// completion/latch/activity lock. In-flight HTTP has the finite ApiClient
/// timeout; cancellation prevents any subsequent request or publication.
pub(super) fn shutdown_notification_workers() -> std::result::Result<(), String> {
    NOTIFICATION_SHUTDOWN.with(|closed| closed.set(true));
    NOTIFICATION_THREADS.with(|threads| {
        for worker in threads.borrow().iter() { worker.cancelled.store(true, Ordering::Release); }
    });
    join_notification_workers()
}
fn spawn_notification_thread<T: Send + 'static>(
    context: &AppContext,
    capture: &NotificationCapture,
    work: impl FnOnce(NotificationsApi, &SessionScope) -> std::result::Result<T, ApiError> + Send + 'static,
) -> std::result::Result<mpsc::Receiver<std::result::Result<T, ApiError>>, ApiError> {
    if !capture.is_current(context) { return Err(ApiError::AuthenticationRequired); }
    let activity = capture.persistence.begin_activity().map_err(|_| ApiError::AuthenticationRequired)?;
    let backend = context.backend.as_ref().ok_or(ApiError::AuthenticationRequired)?.clone();
    let namespace = context.active_namespace.clone();
    let worker = capture.clone();
    let (sender, receiver) = mpsc::channel();
    // No upgrade effect/durable guard is held across HTTP: that same HTTP call
    // can trip the exact 426 latch and must never wait on its own permit.
    let handle = std::thread::Builder::new().name("notification-request".into()).spawn(move || {
        let result = if activity.is_quiescing()
            || worker.cancelled.load(Ordering::Acquire)
            || worker.persistence.upgrade_latch().is_tripped()
            || !namespace.lock().ok().is_some_and(|active| active.as_ref() == Some(worker.persistence.lease()))
            || !backend.api.session().is_scope_current(&worker.scope)
        {
            Err(ApiError::AuthenticationRequired)
        } else {
            work(NotificationsApi::new(backend.api.clone()), &worker.scope)
        };
        let _ = sender.send(result);
        drop(activity);
    }).map_err(|_| ApiError::LocalState { message: "通知任务无法启动，请重试".into() })?;
    NOTIFICATION_THREADS.with(|threads| threads.borrow_mut().push(NotificationThread {
        lease: capture.persistence.lease().clone(), cancelled: capture.cancelled.clone(), handle,
    }));
    Ok(receiver)
}

fn apply_notification_ui<R>(
    context: &AppContext, capture: &NotificationCapture, apply: impl FnOnce() -> R,
) -> Option<R> {
    if !capture.is_current(context) { return None; }
    context.apply_user_completion(capture.persistence.lease(), || {
        capture.binding_matches(context).then(apply)
    }).ok().flatten()
}

/// Snapshot and enqueue are ordered on the UI thread. Guard-owning enqueue
/// errors and unused prepared writes leave the completion intact before Drop.
fn update_notification_store(
    app: &AppWindow, context: &AppContext, capture: &NotificationCapture,
    epoch: Option<u64>, update: impl FnOnce(&mut Store),
) -> bool {
    if !capture.is_current(context) { return false; }
    let Ok(prepared) = capture.persistence.prepare_ordered_save() else { return false; };
    let mut prepared = Some(prepared);
    let outcome = context.apply_user_completion(capture.persistence.lease(), || {
        if !capture.binding_matches(context) { return None; }
        let mut store = context.store.borrow_mut();
        if epoch.is_some_and(|epoch| store.notification_page_epoch != epoch) { return None; }
        update(&mut store);
        push_notifications(app, &store);
        Some(prepared.take().expect("prepared once").enqueue(local_store_data(app, &store)))
    });
    drop(prepared);
    match outcome.ok().flatten() {
        Some(Ok(receiver)) => {
            poll_notification_save(app.as_weak(), context.clone(), capture.clone(), receiver);
            true
        }
        Some(Err(error)) => {
            // This error owns guards. Release it before re-entering any latch.
            drop(error);
            apply_notification_ui(context, capture, || app.global::<AppState>()
                .set_notification_page_message("通知本地保存失败，请刷新重试".into()));
            false
        }
        None => false,
    }
}
fn poll_notification_save(
    weak: Weak<AppWindow>, context: AppContext, capture: NotificationCapture,
    receiver: mpsc::Receiver<client_state::WriteResult>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                if let Some(app) = weak.upgrade() {
                    apply_notification_ui(&context, &capture, || app.global::<AppState>()
                        .set_notification_page_message("通知本地保存失败，请刷新重试".into()));
                }
            }
            Err(TryRecvError::Empty) => poll_notification_save(weak, context, capture, receiver),
        }
    });
}

#[derive(Clone)]
enum NotificationAction { Read(String), ReadAll, Delete(String), Clear }
impl NotificationAction {
    fn execute(self, api: NotificationsApi, scope: &SessionScope) -> std::result::Result<(), ApiError> {
        match self {
            Self::Read(id) => api.mark_read_scoped(&id, scope),
            Self::ReadAll => api.mark_all_read_scoped(scope),
            Self::Delete(id) => api.delete_scoped(&id, scope),
            Self::Clear => api.delete_all_scoped(scope),
        }
    }
    fn apply(&self, store: &mut Store) {
        match self {
            Self::Read(id) => {
                if let Some(item) = store.notifications.iter_mut().find(|item| &item.id == id) { item.read = true; }
            }
            Self::ReadAll => store.notifications.iter_mut().for_each(|item| item.read = true),
            Self::Delete(id) => store.notifications.retain(|item| &item.id != id),
            Self::Clear => store.notifications.clear(),
        }
    }
}
pub(super) fn wire_notification_callbacks(app: &AppWindow, context: AppContext) {
    if context.backend.is_none() { return; }
    let state = app.global::<AppState>();
    {
        let weak = app.as_weak(); let context = context.clone();
        state.on_mark_notification_read(move |id| {
            if let Some(app) = weak.upgrade() {
                start_notification_operation(&app, context.clone(), NotificationAction::Read(id.to_string()));
            }
        });
    }
    {
        let weak = app.as_weak(); let context = context.clone();
        state.on_mark_all_notifications_read(move || {
            if let Some(app) = weak.upgrade() {
                start_notification_operation(&app, context.clone(), NotificationAction::ReadAll);
            }
        });
    }
    {
        let weak = app.as_weak(); let context = context.clone();
        state.on_delete_notification(move |id| {
            if let Some(app) = weak.upgrade() {
                start_notification_operation(&app, context.clone(), NotificationAction::Delete(id.to_string()));
            }
        });
    }
    {
        let weak = app.as_weak(); let context = context.clone();
        state.on_clear_all_notifications(move || {
            if let Some(app) = weak.upgrade() {
                start_notification_operation(&app, context.clone(), NotificationAction::Clear);
            }
        });
    }
    {
        let weak = app.as_weak();
        state.on_load_more_notifications(move || {
            let Some(app) = weak.upgrade() else { return; };
            let Some(capture) = NotificationCapture::new(&context) else { return; };
            let state = app.global::<AppState>();
            if state.get_notification_page_loading() || !state.get_notification_page_has_more() { return; }
            let cursor = state.get_notification_next_cursor().trim().to_string();
            if cursor.is_empty() {
                apply_notification_ui(&context, &capture, || state.set_notification_page_has_more(false));
                return;
            }
            start_notification_page(&app, context.clone(), Some(cursor), true);
        });
    }
}

fn start_notification_operation(app: &AppWindow, context: AppContext, action: NotificationAction) {
    let Some(capture) = NotificationCapture::new(&context) else { return; };
    let Some(epoch) = context.store.borrow().notification_page_epoch.checked_add(1) else {
        apply_notification_ui(&context, &capture, || app.global::<AppState>()
            .set_notification_page_message("通知分页版本已用尽，请重新登录".into()));
        return;
    };
    let clear = matches!(action, NotificationAction::Clear);
    if !update_notification_store(app, &context, &capture, None, |store| {
        store.notification_page_epoch = epoch;
        action.apply(store);
        let state = app.global::<AppState>();
        state.set_notification_page_loading(false);
        if clear { reset_notification_pagination_ui(app); }
    }) { return; }
    let receiver = match spawn_notification_thread(&context, &capture, move |api, scope| action.execute(api, scope)) {
        Ok(receiver) => receiver,
        Err(error) => {
            notification_error(app, &context, &capture, Some(epoch), true, error);
            return;
        }
    };
    poll_notification_operation(app.as_weak(), context, capture, epoch, receiver);
}

pub(super) fn refresh_server_notifications(app: &AppWindow, context: AppContext) {
    start_notification_page(app, context, None, false);
}

/// Control-plane cleanup called for the namespace currently being retired.
/// It intentionally does not require an ordinary admission that is now closed.
pub(super) fn clear_notification_account_state(app: &AppWindow, context: &AppContext) {
    if let Some(persistence) = context.store.borrow().private_persistence.as_ref() {
        cancel_notification_workers_for_retirement(persistence.lease());
    }
    let mut store = context.store.borrow_mut();
    store.notifications.clear();
    // Saturation in cleanup is safe: no ordinary request may allocate MAX+1.
    store.notification_page_epoch = store.notification_page_epoch.saturating_add(1);
    push_notifications(app, &store);
    drop(store);
    reset_notification_pagination_ui(app);
}
fn reset_notification_pagination_ui(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_notification_page_loading(false);
    state.set_notification_page_has_more(false);
    state.set_notification_next_cursor("".into());
    state.set_notification_page_message("".into());
}
fn start_notification_page(app: &AppWindow, context: AppContext, cursor: Option<String>, append: bool) {
    let Some(capture) = NotificationCapture::new(&context) else { return; };
    let Some(epoch) = context.store.borrow().notification_page_epoch.checked_add(1) else {
        apply_notification_ui(&context, &capture, || app.global::<AppState>()
            .set_notification_page_message("通知分页版本已用尽，请重新登录".into()));
        return;
    };
    if apply_notification_ui(&context, &capture, || {
        context.store.borrow_mut().notification_page_epoch = epoch;
        let state = app.global::<AppState>();
        state.set_notification_page_loading(true);
        state.set_notification_page_message("".into());
        if !append {
            state.set_notification_page_has_more(false);
            state.set_notification_next_cursor("".into());
        }
    }).is_none() { return; }
    match spawn_notification_thread(&context, &capture, move |api, scope| api.list_page_scoped(cursor.as_deref(), scope)) {
        Ok(receiver) => poll_server_notifications(app.as_weak(), context, capture, epoch, append, receiver),
        Err(error) => notification_error(app, &context, &capture, Some(epoch), false, error),
    }
}

fn notification_session_ended(error: &ApiError) -> bool {
    matches!(error, ApiError::AuthenticationRequired) || error.is_terminal_session_error()
}
fn notification_error(
    app: &AppWindow, context: &AppContext, capture: &NotificationCapture,
    page_epoch: Option<u64>, operation: bool, error: ApiError,
) {
    // Session invalidation can intentionally make is_scope_current false.
    // Dispatch this control action outside apply_user_completion: logout
    // quiesces/joins the old user and cannot be called while holding its latch.
    if notification_session_ended(&error) {
        if capture.namespace_is_current(context)
            && terminal_auth_scope_matches_context(context, &capture.scope)
        {
            sign_out_locally(app, context, true, Some(capture.scope.auth_epoch));
        }
        return;
    }
    apply_notification_ui(context, capture, || {
        if page_epoch.is_some_and(|epoch| context.store.borrow().notification_page_epoch != epoch) { return; }
        let state = app.global::<AppState>();
        state.set_notification_page_loading(false);
        state.set_notification_page_message(error.user_message().into());
        if operation {
            state.set_generation_status(format!("通知操作失败：{}", error.user_message()).into());
        }
    });
}

fn poll_notification_operation(
    weak: Weak<AppWindow>, context: AppContext, capture: NotificationCapture, epoch: u64,
    receiver: mpsc::Receiver<std::result::Result<(), ApiError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let _ = reap_notification_workers();
        if notification_worker_pending(&capture) {
            poll_notification_operation(weak, context, capture, epoch, receiver); return;
        }
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                poll_notification_operation(weak, context, capture, epoch, receiver); return;
            }
            Err(TryRecvError::Disconnected) => Err(ApiError::LocalState { message: "通知任务意外中断，请刷新重试".into() }),
        };
        let Some(app) = weak.upgrade() else { return; };
        if let Err(error) = result {
            let terminal = notification_session_ended(&error);
            notification_error(&app, &context, &capture, Some(epoch), true, error);
            if !terminal && capture.is_current(&context)
                && context.store.borrow().notification_page_epoch == epoch
            {
                // Restore authoritative read/delete state after a failed optimistic
                // action. This is a GET, never a blind mutation retry.
                start_notification_page(&app, context, None, false);
            }
        }
    });
}
fn poll_server_notifications(
    weak: Weak<AppWindow>, context: AppContext, capture: NotificationCapture,
    epoch: u64, append: bool,
    receiver: mpsc::Receiver<std::result::Result<NotificationPage, ApiError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let _ = reap_notification_workers();
        if notification_worker_pending(&capture) {
            poll_server_notifications(weak, context, capture, epoch, append, receiver); return;
        }
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                poll_server_notifications(weak, context, capture, epoch, append, receiver); return;
            }
            Err(TryRecvError::Disconnected) => Err(ApiError::LocalState { message: "通知加载任务意外中断".into() }),
        };
        let Some(app) = weak.upgrade() else { return; };
        match result {
            Ok(page) => {
                let updated = update_notification_store(&app, &context, &capture, Some(epoch), |store| {
                    if !append { store.notifications.clear(); }
                    for item in page.items.into_iter().map(notification_data) {
                        if !store.notifications.iter().any(|existing| existing.id == item.id) {
                            store.notifications.push(item);
                        }
                    }
                    let state = app.global::<AppState>();
                    let cursor = page.next_cursor.unwrap_or_default();
                    state.set_notification_next_cursor(cursor.clone().into());
                    state.set_notification_page_has_more(!cursor.trim().is_empty());
                    state.set_notification_page_loading(false);
                    state.set_notification_page_message("".into());
                });
                if !updated {
                    apply_notification_ui(&context, &capture, || {
                        if context.store.borrow().notification_page_epoch == epoch {
                            let state = app.global::<AppState>();
                            state.set_notification_page_loading(false);
                            state.set_notification_page_message("通知本地保存失败，请刷新重试".into());
                        }
                    });
                }
            }
            Err(error) => notification_error(&app, &context, &capture, Some(epoch), false, error),
        }
    });
}

pub(super) fn notification_is_success(item: &ServerNotification) -> bool {
    if has_failure_marker(&item.notification_type) || has_failure_marker(&item.title) {
        return false;
    }

    !["status", "task_status", "result", "outcome"]
        .iter()
        .filter_map(|key| item.metadata.get(key).and_then(Value::as_str))
        .any(has_failure_marker)
}

fn has_failure_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "failed",
        "failure",
        "error",
        "expired",
        "cancelled",
        "canceled",
        "rejected",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || ["失败", "未完成", "错误", "已取消", "已过期"]
            .iter()
            .any(|marker| value.contains(marker))
}

fn notification_display_model(item: &ServerNotification) -> String {
    match item.metadata.get("task_type").and_then(Value::as_str) {
        Some("image_upscale") => "图片清晰".to_string(),
        Some("image_watermark_removal") => "去水印".to_string(),
        Some("image_cutout") => "智能抠图".to_string(),
        Some("image_colorization") => "老照片上色".to_string(),
        _ => item
            .metadata
            .get("model_name")
            .or_else(|| item.metadata.get("model_code"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn notification_data(item: ServerNotification) -> NotificationData {
    let model = notification_display_model(&item);
    let success = notification_is_success(&item);
    NotificationData {
        id: item.id,
        title: item.title,
        model,
        time: format_notification_time(&item.created_at),
        reason: item.body,
        success,
        read: item.read_at.is_some(),
    }
}

fn format_notification_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| {
            time.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_notification(task_type: &str, model_name: &str) -> ServerNotification {
        ServerNotification {
            id: "notification-1".to_string(),
            notification_type: "generation.finished".to_string(),
            title: "生成完成".to_string(),
            body: "图片已经生成，可以下载到本地图库。".to_string(),
            metadata: serde_json::json!({
                "task_type": task_type,
                "model_name": model_name,
            }),
            created_at: "2026-07-30T00:00:00Z".to_string(),
            read_at: None,
        }
    }

    #[test]
    fn toolbox_notifications_use_product_names_instead_of_provider_models() {
        assert_eq!(
            notification_display_model(&server_notification("image_upscale", "阿里云图像超分",)),
            "图片清晰",
        );
        assert_eq!(
            notification_display_model(&server_notification(
                "image_watermark_removal",
                "gpt-image-2",
            )),
            "去水印",
        );
        assert_eq!(
            notification_display_model(&server_notification("image_colorization", "老照片上色",)),
            "老照片上色",
        );
        assert_eq!(
            notification_display_model(&server_notification("image_cutout", "阿里云图像分割",)),
            "智能抠图",
        );
        assert_eq!(
            notification_display_model(&server_notification("image_generation", "gpt-image-2",)),
            "gpt-image-2",
        );
    }
}

#[cfg(test)]
mod core_notification_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    const OWNER: &str = "11111111-1111-4111-8111-111111111111";
    const OTHER: &str = "22222222-2222-4222-8222-222222222222";

    // Every listener, connection handler, HTTP caller and SQLite writer is joined.
    // No fixture uses the application's HOME or a production persistence path.
    struct Transport {
        url: String,
        seen: mpsc::Receiver<usize>,
        replies: Vec<Option<mpsc::Sender<String>>>,
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<Vec<String>>>,
    }
    impl Transport {
        fn new(slots: usize) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let (seen_tx, seen) = mpsc::channel();
            let mut replies = Vec::new();
            let mut receivers = Vec::new();
            for _ in 0..slots {
                let (tx, rx) = mpsc::channel();
                replies.push(Some(tx));
                receivers.push(Some(rx));
            }
            let handle = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(12);
                let mut children = Vec::new();
                while !worker_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let index = children.len();
                            let reply = receivers.get_mut(index).and_then(Option::take);
                            let seen_tx = seen_tx.clone();
                            children.push(std::thread::spawn(move || {
                                stream.set_nonblocking(false).unwrap();
                                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                                stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                                let mut bytes = Vec::new();
                                let mut byte = [0u8; 1];
                                while bytes.len() < 16_384 && !bytes.ends_with(b"\r\n\r\n") {
                                    if stream.read(&mut byte).unwrap_or(0) == 0 { break; }
                                    bytes.push(byte[0]);
                                }
                                let header = String::from_utf8(bytes).unwrap();
                                assert!(header.ends_with("\r\n\r\n"), "bounded request header missing");
                                assert!(!header.to_ascii_lowercase().contains("x-account-group-id:"),
                                    "identity notifications must remain group-header-free");
                                let line = header.lines().next().unwrap().to_string();
                                let _ = seen_tx.send(index);
                                let response = reply.and_then(|rx| rx.recv_timeout(Duration::from_secs(5)).ok())
                                    .unwrap_or_else(|| response(500, serde_json::json!({"items":[]})));
                                let _ = stream.write_all(response.as_bytes());
                                line
                            }));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock =>
                            std::thread::sleep(Duration::from_millis(2)),
                        Err(_) => panic!("fixture listener failed"),
                    }
                }
                let mut lines = Vec::new();
                let mut failed = false;
                for child in children {
                    match child.join() { Ok(line) => lines.push(line), Err(_) => failed = true }
                }
                assert!(!failed, "fixture connection panicked");
                lines
            });
            Self { url, seen, replies, stop, handle: Some(handle) }
        }
        fn wait_request(&self) {
            self.seen.recv_timeout(Duration::from_secs(4)).expect("notification request was not sent");
        }
        fn reply(&mut self, index: usize, value: String) {
            self.replies[index].take().unwrap().send(value).unwrap();
        }
        fn finish(mut self) -> Vec<String> {
            self.stop.store(true, Ordering::Release);
            for reply in &mut self.replies { reply.take(); }
            self.handle.take().unwrap().join().expect("fixture listener panicked")
        }
    }
    impl Drop for Transport {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            for reply in &mut self.replies { reply.take(); }
            if let Some(handle) = self.handle.take() {
                let joined = handle.join();
                if !std::thread::panicking() { assert!(joined.is_ok(), "fixture listener panicked"); }
            }
        }
    }

    struct JoinedUpgradeTrip {
        handle: Option<std::thread::JoinHandle<()>>,
    }
    impl JoinedUpgradeTrip {
        fn start(latch: api::UpgradeLatch) -> Self {
            let observed = latch.clone();
            let handle = std::thread::spawn(move || {
                latch.trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
            });
            let trip = Self { handle: Some(handle) };
            let deadline = Instant::now() + Duration::from_secs(3);
            while !observed.is_tripped() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(observed.is_tripped(), "upgrade did not close admission");
            trip
        }
        fn join(mut self) {
            self.handle.take().unwrap().join().expect("upgrade trip worker panicked");
        }
    }
    impl Drop for JoinedUpgradeTrip {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                let joined = handle.join();
                if !std::thread::panicking() { assert!(joined.is_ok(), "upgrade trip worker panicked"); }
            }
        }
    }

    struct Fixture {
        context: AppContext,
        persistence: PrivatePersistence,
        lease: NamespaceLease,
        writer: client_state::tests::Fixture,
        expected_join_failure: bool,
    }
    impl Fixture {
        fn new(url: &str) -> Self {
            let writer = client_state::tests::Fixture::new(false, false);
            let session = Arc::new(SessionManager::new(Arc::new(
                crate::runtime::test_support::MemoryRefreshTokenStore::default(),
            )));
            let scope = session.install_tokens_for_user(&tokens(), OWNER).unwrap();
            let lease = writer.lease(OWNER, scope.auth_epoch, 1);
            writer.activate(lease.clone()).unwrap();
            let api = ApiClient::new(ApiClientConfig {
                base_url: reqwest::Url::parse(url).unwrap(), app_version: "999.0.0".into(),
                timeout: Duration::from_secs(3),
            }, DeviceIdentity { id: Uuid::new_v4().to_string(), name: "notification-fixture".into(),
                platform: "macos".into() }, session).unwrap();
            let context = AppContext {
                backend: Some(Arc::new(BackendRuntime { api })),
                current_user_id: Arc::new(Mutex::new(Some(OWNER.into()))), ..Default::default()
            };
            context.user_activity.activate(lease.clone()).unwrap();
            *context.active_namespace.lock().unwrap() = Some(lease.clone());
            let persistence = PrivatePersistence::for_test(
                (*writer).clone(), lease.clone(), context.user_activity.clone(),
                context.backend.as_ref().unwrap().api.upgrade_latch().clone(),
            );
            context.store.borrow_mut().private_persistence = Some(persistence.clone());
            Self { context, persistence, lease, writer, expected_join_failure: false }
        }
        fn setup(&self) -> AppWindow {
            let app = AppWindow::new().unwrap();
            self.context.store.borrow_mut().notifications = vec![notification_data(item("first"))];
            push_notifications(&app, &self.context.store.borrow());
            wire_notification_callbacks(&app, self.context.clone());
            app
        }
        fn retire(&self) {
            self.context.user_activity.begin_quiesce(&self.lease).unwrap().retire();
        }
        fn stored_ids(&self) -> Vec<String> {
            self.writer.flush(&self.lease).unwrap();
            self.writer.load_client_state_for_namespace(&self.lease).unwrap()
                .map(|data| data.notifications.into_iter().map(|item| item.id).collect())
                .unwrap_or_default()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let joined = join_notification_workers();
            if !std::thread::panicking() {
                assert_eq!(joined.is_err(), self.expected_join_failure, "notification worker join outcome");
            }
        }
    }
    fn tokens() -> TokenSet {
        TokenSet { access_token: "fixture-access".into(), access_expires_in_seconds: 1800,
            refresh_token: "fixture-refresh".into(), refresh_expires_at: "2099-01-01T00:00:00Z".into(),
            token_type: "X-Token".into() }
    }
    fn item(id: &str) -> ServerNotification {
        ServerNotification { id: id.into(), notification_type: "generation.finished".into(),
            title: id.into(), body: "private notification".into(), metadata: serde_json::json!({}),
            created_at: "2026-09-01T00:00:00Z".into(), read_at: None }
    }
    fn item_json(id: &str) -> Value {
        serde_json::json!({"id":id,"type":"generation.finished","title":id,"body":"private notification",
            "metadata":{},"created_at":"2026-09-01T00:00:00Z","read_at":null})
    }
    fn response(status: u16, data: Value) -> String {
        let body = if status == 200 {
            serde_json::json!({"request_id":"fixture","data":data,"error":null,"meta":null})
        } else {
            serde_json::json!({"request_id":"fixture","data":null,
                "error":{"code":"fixture_failure","message":"controlled failure","details":null},"meta":null})
        }.to_string();
        format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }
    fn page(ids: &[&str], cursor: Option<&str>) -> String {
        response(200, serde_json::json!({"items":ids.iter().map(|id| item_json(id)).collect::<Vec<_>>(),
            "next_cursor":cursor}))
    }
    fn pump_for(duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn pump(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready() && Instant::now() < deadline { pump_for(Duration::from_millis(5)); }
        assert!(ready(), "notification completion was not observed");
    }
    fn ids(context: &AppContext) -> Vec<String> {
        context.store.borrow().notifications.iter().map(|item| item.id.clone()).collect()
    }
    fn invoke_mutations(app: &AppWindow) {
        let state = app.global::<AppState>();
        state.invoke_mark_notification_read("first".into());
        state.invoke_mark_all_notifications_read();
        state.invoke_delete_notification("first".into());
        state.invoke_clear_all_notifications();
    }

    #[test]
    fn core_notifications_missing_store_authority_rejects_all_four_actions() {
        i_slint_backend_testing::init_no_event_loop();
        let transport = Transport::new(0);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        fixture.context.store.borrow_mut().private_persistence = None;
        invoke_mutations(&app);
        join_notification_workers().unwrap();
        assert_eq!(ids(&fixture.context), vec!["first"]);
        assert!(!fixture.context.store.borrow().notifications[0].read);
        assert_eq!(app.global::<AppState>().get_notifications().row_count(), 1);
        assert!(transport.finish().is_empty());
        assert!(fixture.stored_ids().is_empty());
    }

    #[test]
    fn core_notifications_exact_upgrade_rejects_all_actions_and_refresh() {
        i_slint_backend_testing::init_no_event_loop();
        let transport = Transport::new(0);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        invoke_mutations(&app);
        refresh_server_notifications(&app, fixture.context.clone());
        join_notification_workers().unwrap();
        assert_eq!(ids(&fixture.context), vec!["first"]);
        assert!(!fixture.context.store.borrow().notifications[0].read);
        assert!(!app.global::<AppState>().get_notification_page_loading());
        assert!(transport.finish().is_empty());
        assert!(fixture.stored_ids().is_empty());
    }

    #[test]
    fn core_notifications_late_success_and_error_after_upgrade_do_not_publish() {
        i_slint_backend_testing::init_no_event_loop();
        for success in [true, false] {
            let mut transport = Transport::new(1);
            let fixture = Fixture::new(&transport.url);
            let app = fixture.setup();
            refresh_server_notifications(&app, fixture.context.clone());
            transport.wait_request();
            let trip = JoinedUpgradeTrip::start(fixture.persistence.upgrade_latch());
            app.global::<AppState>().set_notification_page_message("upgrade boundary".into());
            app.global::<AppState>().set_notification_page_loading(false);
            transport.reply(0, if success { page(&["late"], None) } else { response(500, Value::Null) });
            trip.join();
            join_notification_workers().unwrap();
            pump_for(Duration::from_millis(30));
            assert_eq!(ids(&fixture.context), vec!["first"]);
            assert_eq!(app.global::<AppState>().get_notification_page_message(), "upgrade boundary");
            assert!(!app.global::<AppState>().get_notification_page_loading());
            assert!(fixture.stored_ids().is_empty());
            assert_eq!(transport.finish().len(), 1);
        }
    }

    #[test]
    fn core_notifications_retired_a_completion_cannot_fill_b_store() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport = Transport::new(1);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        refresh_server_notifications(&app, fixture.context.clone());
        transport.wait_request();
        transport.reply(0, page(&["private-a"], None));
        join_notification_workers().unwrap();
        fixture.retire();
        // Session can remain A while its old namespace has already retired.
        let b = fixture.writer.lease(OTHER, fixture.lease.auth_epoch, 2);
        *fixture.context.active_namespace.lock().unwrap() = Some(b);
        fixture.context.store.borrow_mut().private_persistence = None;
        fixture.context.store.borrow_mut().notifications = vec![notification_data(item("b"))];
        push_notifications(&app, &fixture.context.store.borrow());
        app.global::<AppState>().set_notification_page_message("B state".into());
        pump_for(Duration::from_millis(30));
        assert_eq!(ids(&fixture.context), vec!["b"]);
        assert_eq!(app.global::<AppState>().get_notification_page_message(), "B state");
        assert!(fixture.stored_ids().is_empty());
        assert_eq!(transport.finish().len(), 1);
    }

    #[test]
    fn core_notifications_clear_invalidates_inflight_page_and_persists_empty() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport = Transport::new(2);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        fixture.persistence.save_store(local_store_data(&app, &fixture.context.store.borrow())).unwrap();
        assert_eq!(fixture.stored_ids(), vec!["first"]);
        refresh_server_notifications(&app, fixture.context.clone());
        transport.wait_request();
        app.global::<AppState>().invoke_clear_all_notifications();
        transport.wait_request();
        transport.reply(1, response(200, serde_json::json!({})));
        transport.reply(0, page(&["stale"], Some("stale-cursor")));
        join_notification_workers().unwrap();
        pump_for(Duration::from_millis(50));
        assert!(ids(&fixture.context).is_empty());
        assert!(fixture.stored_ids().is_empty());
        assert!(!app.global::<AppState>().get_notification_page_has_more());
        assert!(!app.global::<AppState>().get_notification_page_loading());
        assert_eq!(transport.finish().len(), 2);
    }

    #[test]
    fn core_notifications_normal_page_merge_read_and_delete_use_real_callbacks_and_sqlite() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport = Transport::new(4);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        refresh_server_notifications(&app, fixture.context.clone());
        transport.wait_request();
        transport.reply(0, page(&["first"], Some("next")));
        pump(|| !app.global::<AppState>().get_notification_page_loading());
        app.global::<AppState>().invoke_load_more_notifications();
        transport.wait_request();
        transport.reply(1, page(&["first", "second"], None));
        pump(|| !app.global::<AppState>().get_notification_page_loading());
        assert_eq!(ids(&fixture.context), vec!["first", "second"]);
        assert_eq!(fixture.stored_ids(), vec!["first", "second"]);
        app.global::<AppState>().invoke_mark_notification_read("first".into());
        transport.wait_request();
        transport.reply(2, response(200, item_json("first")));
        pump_for(Duration::from_millis(30));
        assert!(fixture.context.store.borrow().notifications[0].read);
        app.global::<AppState>().invoke_delete_notification("first".into());
        transport.wait_request();
        transport.reply(3, response(200, serde_json::json!({})));
        join_notification_workers().unwrap();
        pump_for(Duration::from_millis(50));
        assert_eq!(ids(&fixture.context), vec!["second"]);
        assert_eq!(fixture.stored_ids(), vec!["second"]);
        let paths = transport.finish();
        assert_eq!(paths.len(), 4);
        assert!(paths[1].contains("cursor=next"));
        assert!(paths[2].starts_with("POST /v1/notifications/first/read "));
        assert!(paths[3].starts_with("DELETE /v1/notifications/first "));
    }

    #[test]
    fn core_notifications_page_epoch_exhaustion_never_wraps_or_dispatches() {
        i_slint_backend_testing::init_no_event_loop();
        let transport = Transport::new(0);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        fixture.context.store.borrow_mut().notification_page_epoch = u64::MAX;
        refresh_server_notifications(&app, fixture.context.clone());
        join_notification_workers().unwrap();
        assert_eq!(fixture.context.store.borrow().notification_page_epoch, u64::MAX);
        assert_eq!(ids(&fixture.context), vec!["first"]);
        assert!(!app.global::<AppState>().get_notification_page_loading());
        assert!(transport.finish().is_empty());
    }

    #[test]
    fn core_notifications_terminal_a_error_does_not_sign_out_actual_b_session() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport = Transport::new(1);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        refresh_server_notifications(&app, fixture.context.clone());
        transport.wait_request();
        let b_scope = fixture.context.backend.as_ref().unwrap().api.session()
            .install_tokens_for_user(&tokens(), OTHER).unwrap();
        *fixture.context.current_user_id.lock().unwrap() = Some(OTHER.into());
        let b_lease = fixture.writer.lease(OTHER, b_scope.auth_epoch, 2);
        *fixture.context.active_namespace.lock().unwrap() = Some(b_lease);
        fixture.context.store.borrow_mut().private_persistence = None;
        fixture.context.store.borrow_mut().notifications = vec![notification_data(item("b"))];
        push_notifications(&app, &fixture.context.store.borrow());
        app.global::<AppState>().set_notification_page_message("B state".into());
        let body = serde_json::json!({"request_id":"fixture","data":null,"meta":null,
            "error":{"code":"session_invalid","message":"revoked","details":null}}).to_string();
        transport.reply(0, format!("HTTP/1.1 401 Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()));
        join_notification_workers().unwrap();
        pump_for(Duration::from_millis(40));
        assert!(fixture.context.backend.as_ref().unwrap().api.session().is_scope_current(&b_scope));
        assert_eq!(ids(&fixture.context), vec!["b"]);
        assert_eq!(app.global::<AppState>().get_notification_page_message(), "B state");
        assert_eq!(transport.finish().len(), 1);
    }

    #[test]
    fn core_notifications_disconnect_finishes_loading_and_joins_real_worker() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport = Transport::new(1);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        refresh_server_notifications(&app, fixture.context.clone());
        transport.wait_request();
        transport.reply(0, String::new()); // Real peer close without HTTP headers.
        pump(|| !app.global::<AppState>().get_notification_page_loading());
        assert!(!app.global::<AppState>().get_notification_page_message().is_empty());
        assert_eq!(ids(&fixture.context), vec!["first"]);
        assert!(NOTIFICATION_THREADS.with(|threads| threads.borrow().is_empty()));
        assert_eq!(transport.finish().len(), 1);
    }

    #[test]
    fn core_notifications_cancelled_original_lease_and_shutdown_reap_actual_workers() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport = Transport::new(1);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        refresh_server_notifications(&app, fixture.context.clone());
        transport.wait_request();
        assert_eq!(NOTIFICATION_THREADS.with(|threads| threads.borrow().len()), 1);
        let other = fixture.writer.lease(OTHER, fixture.lease.auth_epoch, 2);
        cancel_notification_workers_for_retirement(&other);
        assert!(!NOTIFICATION_THREADS.with(|threads| threads.borrow()[0].cancelled.load(Ordering::Acquire)));
        cancel_notification_workers_for_retirement(&fixture.lease);
        assert!(NOTIFICATION_THREADS.with(|threads| threads.borrow()[0].cancelled.load(Ordering::Acquire)));
        transport.reply(0, page(&["cancelled-a"], None));
        // No timer is needed to drain: shutdown survives event-loop termination.
        shutdown_notification_workers().unwrap();
        assert!(NOTIFICATION_THREADS.with(|threads| threads.borrow().is_empty()));
        pump_for(Duration::from_millis(40));
        assert_eq!(ids(&fixture.context), vec!["first"]);
        invoke_mutations(&app);
        refresh_server_notifications(&app, fixture.context.clone());
        assert_eq!(ids(&fixture.context), vec!["first"]);
        assert_eq!(transport.finish().len(), 1);
    }

    #[test]
    fn core_notifications_read_all_remains_available_and_persisted() {
        i_slint_backend_testing::init_no_event_loop();
        let mut transport = Transport::new(1);
        let fixture = Fixture::new(&transport.url);
        let app = fixture.setup();
        fixture.context.store.borrow_mut().notifications.push(notification_data(item("second")));
        app.global::<AppState>().invoke_mark_all_notifications_read();
        transport.wait_request();
        transport.reply(0, response(200, serde_json::json!({})));
        join_notification_workers().unwrap();
        pump_for(Duration::from_millis(40));
        assert!(fixture.context.store.borrow().notifications.iter().all(|item| item.read));
        fixture.writer.flush(&fixture.lease).unwrap();
        let saved = fixture.writer.load_client_state_for_namespace(&fixture.lease).unwrap().unwrap();
        assert_eq!(saved.notifications.len(), 2);
        assert!(saved.notifications.iter().all(|item| item.read));
        assert_eq!(transport.finish().len(), 1);
    }

    #[test]
    fn core_notifications_reaped_panic_remains_a_shutdown_failure() {
        let mut fixture = Fixture::new("http://127.0.0.1:9/");
        fixture.expected_join_failure = true;
        let panic_capture = NotificationCapture::new(&fixture.context).unwrap();
        let panic_result = spawn_notification_thread::<()>(&fixture.context, &panic_capture,
            |_, _| panic!("controlled notification worker panic")).unwrap();
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = completed.clone();
        let good_capture = NotificationCapture::new(&fixture.context).unwrap();
        let good_result = spawn_notification_thread(&fixture.context, &good_capture, move |_, _| {
            worker_completed.store(true, Ordering::Release);
            Ok(())
        }).unwrap();
        assert!(panic_result.recv_timeout(Duration::from_secs(3)).is_err());
        good_result.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while NOTIFICATION_THREADS.with(|threads| !threads.borrow().is_empty())
            && Instant::now() < deadline
        {
            let _ = reap_notification_workers();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(completed.load(Ordering::Acquire));
        assert!(NOTIFICATION_THREADS.with(|threads| threads.borrow().is_empty()));
        assert!(!reap_notification_workers(), "a second empty reap must retain the earlier failure");
        assert!(shutdown_notification_workers().is_err(),
            "an already-reaped panic must not become successful shutdown");
        assert!(join_notification_workers().is_err(),
            "the failure is sticky across repeated empty drains");
    }

    #[test]
    fn core_notifications_page_waits_for_registered_worker_after_result_send() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = Fixture::new("http://127.0.0.1:9/");
        let app = fixture.setup();
        let capture = NotificationCapture::new(&fixture.context).unwrap();
        fixture.context.store.borrow_mut().notification_page_epoch = 1;
        app.global::<AppState>().set_notification_page_loading(true);
        let activity = fixture.persistence.begin_activity().unwrap();
        let (sender, receiver) = mpsc::channel();
        let (sent_tx, sent_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        // Real registered thread and production poller. This controlled interval
        // reproduces sender.send succeeding before activity Drop/thread exit.
        let handle = std::thread::spawn(move || {
            sender.send(Ok(NotificationPage { items: vec![item("after-join")], next_cursor: None })).unwrap();
            sent_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(3));
            drop(activity);
        });
        NOTIFICATION_THREADS.with(|threads| threads.borrow_mut().push(NotificationThread {
            lease: fixture.lease.clone(), cancelled: capture.cancelled.clone(), handle,
        }));
        sent_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        poll_server_notifications(app.as_weak(), fixture.context.clone(), capture, 1, false, receiver);
        pump_for(Duration::from_millis(30));
        assert_eq!(ids(&fixture.context), vec!["first"]);
        assert!(app.global::<AppState>().get_notification_page_loading());
        assert_eq!(NOTIFICATION_THREADS.with(|threads| threads.borrow().len()), 1);
        release_tx.send(()).unwrap();
        pump(|| !app.global::<AppState>().get_notification_page_loading());
        assert_eq!(ids(&fixture.context), vec!["after-join"]);
        assert!(NOTIFICATION_THREADS.with(|threads| threads.borrow().is_empty()));
        assert_eq!(fixture.stored_ids(), vec!["after-join"]);
    }
}
