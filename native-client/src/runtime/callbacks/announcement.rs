use super::*;
use crate::runtime::announcement_schedule::AnnouncementSchedule;
use std::collections::VecDeque;
use std::time::SystemTime;

pub(super) struct AnnouncementRuntime {
    timer: Option<slint::Timer>,
    binding: Option<NotificationCapture>,
    schedule: AnnouncementSchedule,
    queue: VecDeque<ServerNotification>,
    ready: Option<ServerNotification>,
    current_id: Option<String>,
    claiming: bool,
    batch_closed: bool,
    binding_epoch: Rc<()>,
}

impl Default for AnnouncementRuntime {
    fn default() -> Self {
        Self {
            timer: None,
            binding: None,
            schedule: AnnouncementSchedule::new(""),
            queue: VecDeque::new(),
            ready: None,
            current_id: None,
            claiming: false,
            batch_closed: false,
            binding_epoch: Rc::new(()),
        }
    }
}

fn binding_matches(context: &AppContext, capture: &NotificationCapture) -> bool {
    capture.is_current(context)
        && context
            .announcements
            .borrow()
            .binding
            .as_ref()
            .is_some_and(|active| {
                active.scope == capture.scope
                    && active.persistence.lease() == capture.persistence.lease()
            })
}

pub(super) fn clear_announcement_account_state(app: &AppWindow, context: &AppContext) {
    let mut runtime = context.announcements.borrow_mut();
    runtime.binding = None;
    runtime.binding_epoch = Rc::new(());
    runtime.schedule.reset();
    runtime.queue.clear();
    runtime.ready = None;
    runtime.current_id = None;
    runtime.claiming = false;
    runtime.batch_closed = false;
    let state = app.global::<AppState>();
    state.set_announcement_open(false);
    state.set_announcement_title("".into());
    state.set_announcement_body("".into());
    state.set_announcement_time("".into());
    state.set_announcement_pending_count(0);
}

pub(super) fn shutdown_announcement_polling(context: &AppContext) {
    let timer = context.announcements.borrow_mut().timer.take();
    if let Some(timer) = timer {
        timer.stop();
    }
}

pub(super) fn wire_announcement_callbacks(app: &AppWindow, context: AppContext) {
    if context.backend.is_none() || context.announcements.borrow().timer.is_some() {
        return;
    }
    let state = app.global::<AppState>();
    let weak = app.as_weak();
    let close_context = context.clone();
    state.on_close_announcement(move || {
        if let Some(app) = weak.upgrade() {
            close_announcement(&app, &close_context);
        }
    });
    let weak = app.as_weak();
    let center_context = context.clone();
    state.on_view_announcement_center(move || {
        if let Some(app) = weak.upgrade() {
            close_announcement(&app, &center_context);
            app.global::<AppState>()
                .invoke_navigate("notifications".into());
        }
    });
    let weak = app.as_weak();
    let history_context = context.clone();
    state.on_open_announcement(move |id| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let Some(capture) = NotificationCapture::new(&history_context) else {
            return;
        };
        let item = history_context
            .store
            .borrow()
            .notifications
            .iter()
            .find(|item| item.id.as_str() == id.as_str() && item.announcement)
            .cloned();
        if let Some(item) = item {
            apply_notification_ui(&history_context, &capture, || {
                let state = app.global::<AppState>();
                state.set_announcement_title(item.title.into());
                state.set_announcement_body(item.reason.into());
                state.set_announcement_time(item.time.into());
                state.set_announcement_important(false);
                state.set_announcement_pending_count(0);
                state.set_announcement_open(true);
                history_context.announcements.borrow_mut().current_id = Some(item.id);
            });
        }
    });
    context.announcements.borrow_mut().schedule =
        AnnouncementSchedule::new(&context.backend.as_ref().unwrap().api.device().id);
    let timer = slint::Timer::default();
    let weak = app.as_weak();
    let tick_context = context.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            if let Some(app) = weak.upgrade() {
                announcement_tick(&app, tick_context.clone());
            }
        },
    );
    context.announcements.borrow_mut().timer = Some(timer);
    announcement_tick(app, context);
}

fn close_announcement(app: &AppWindow, context: &AppContext) {
    let id = {
        let mut runtime = context.announcements.borrow_mut();
        runtime.batch_closed = true;
        runtime.queue.clear();
        runtime.ready = None;
        runtime.current_id.take()
    };
    app.global::<AppState>().set_announcement_open(false);
    if let Some(id) = id {
        app.global::<AppState>()
            .invoke_mark_notification_read(id.into());
    }
}

pub(super) fn announcement_tick(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let capture = NotificationCapture::new(&context)
        .filter(|_| state.get_logged_in() && state.get_session_state() == "online");
    let Some(capture) = capture else {
        if context.announcements.borrow().binding.is_some() || state.get_announcement_open() {
            clear_announcement_account_state(app, &context);
        }
        return;
    };
    if !binding_matches(&context, &capture) {
        clear_announcement_account_state(app, &context);
        context.announcements.borrow_mut().binding = Some(capture.clone());
    }
    request_announcement_check(app, context.clone(), false);
    if !announcement_ui_ready(app) || context.announcements.borrow().batch_closed {
        return;
    }
    let ready = context.announcements.borrow_mut().ready.take();
    if let Some(item) = ready {
        if queued_announcement_can_show(&context, &item, SystemTime::now()) {
            show_announcement(app, context.clone(), capture, item);
        }
        return;
    }
    if context.announcements.borrow().claiming {
        return;
    }
    loop {
        let Some(item) = context.announcements.borrow_mut().queue.pop_front() else {
            return;
        };
        let visible_unread = context
            .store
            .borrow()
            .notifications
            .iter()
            .any(|existing| existing.id == item.id && !existing.read);
        if !visible_unread || !announcement_is_eligible(&item, SystemTime::now()) {
            continue;
        }
        claim_announcement(app, context, capture, item.id);
        return;
    }
}

fn announcement_ui_ready(app: &AppWindow) -> bool {
    let window = slint::private_unstable_api::re_exports::WindowInner::from_pub(app.window());
    app.window().is_visible()
        && !app.window().is_minimized()
        && window.active()
        && window.active_popups().is_empty()
        && !app
            .global::<AppState>()
            .get_announcement_interaction_blocked()
        && !app.global::<AppState>().get_announcement_open()
        && app.global::<AppState>().get_page() != "notifications"
}

fn announcement_is_eligible(item: &ServerNotification, now: SystemTime) -> bool {
    item.notification_type == "system.announcement"
        && item.read_at.is_none()
        && item.metadata.get("importance").and_then(Value::as_str) == Some("important")
        && item.metadata.get("revoked_at").map_or(true, Value::is_null)
        && item
            .metadata
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|expiry| expiry > chrono::DateTime::<chrono::Utc>::from(now))
}

fn queued_announcement_can_show(
    context: &AppContext,
    item: &ServerNotification,
    now: SystemTime,
) -> bool {
    announcement_is_eligible(item, now)
        && context
            .store
            .borrow()
            .notifications
            .iter()
            .any(|stored| stored.id == item.id && stored.announcement && !stored.read)
}

pub(super) fn request_announcement_check(app: &AppWindow, context: AppContext, force: bool) {
    let Some(capture) = NotificationCapture::new(&context) else {
        return;
    };
    if app.global::<AppState>().get_session_state() != "online"
        || !binding_matches(&context, &capture)
    {
        return;
    }
    let now = SystemTime::now();
    let periodic = context.announcements.borrow().schedule.due(now);
    if !periodic && (!force || context.announcements.borrow().schedule.in_flight()) {
        return;
    }
    {
        let mut runtime = context.announcements.borrow_mut();
        runtime.schedule.started(now);
        if periodic {
            runtime.batch_closed = false;
        }
    }
    let epoch = context.store.borrow().notification_page_epoch;
    let binding_epoch = context.announcements.borrow().binding_epoch.clone();
    let receiver = match spawn_notification_thread(&context, &capture, |api, scope| {
        api.announcements_scoped(scope)
    }) {
        Ok(receiver) => receiver,
        Err(_) => {
            context
                .announcements
                .borrow_mut()
                .schedule
                .finished(false, now);
            return;
        }
    };
    await_announcement_result(
        app.as_weak(),
        context,
        capture,
        receiver,
        move |app, context, capture, result| {
            if !binding_matches(context, capture)
                || !Rc::ptr_eq(
                    &context.announcements.borrow().binding_epoch,
                    &binding_epoch,
                )
            {
                return;
            }
            let success = result.is_ok();
            context
                .announcements
                .borrow_mut()
                .schedule
                .finished(success, SystemTime::now());
            match result {
                Ok(items) => {
                    if context.store.borrow().notification_page_epoch != epoch {
                        return;
                    }
                    let items: Vec<_> = items
                        .into_iter()
                        .take(50)
                        .filter(|item| announcement_is_eligible(item, SystemTime::now()))
                        .collect();
                    if !update_notification_store(app, context, capture, Some(epoch), |store| {
                        for item in &items {
                            let mut data = notification_data(item.clone());
                            if let Some(existing) =
                                store.notifications.iter_mut().find(|row| row.id == data.id)
                            {
                                data.read |= existing.read;
                                *existing = data;
                            } else {
                                store.notifications.insert(0, data);
                            }
                        }
                    }) {
                        return;
                    }
                    context.announcements.borrow_mut().queue = items.into();
                }
                Err(error) => quiet_announcement_error(app, context, capture, error),
            }
        },
    );
}

fn claim_announcement(
    app: &AppWindow,
    context: AppContext,
    capture: NotificationCapture,
    id: String,
) {
    context.announcements.borrow_mut().claiming = true;
    let expected = id.clone();
    let epoch = context.store.borrow().notification_page_epoch;
    let binding_epoch = context.announcements.borrow().binding_epoch.clone();
    let receiver = match spawn_notification_thread(&context, &capture, move |api, scope| {
        api.claim_announcement_scoped(&id, scope)
    }) {
        Ok(receiver) => receiver,
        Err(_) => {
            context.announcements.borrow_mut().claiming = false;
            return;
        }
    };
    await_announcement_result(
        app.as_weak(),
        context,
        capture,
        receiver,
        move |app, context, capture, result| {
            if !binding_matches(context, capture)
                || !Rc::ptr_eq(
                    &context.announcements.borrow().binding_epoch,
                    &binding_epoch,
                )
            {
                return;
            }
            context.announcements.borrow_mut().claiming = false;
            if context.store.borrow().notification_page_epoch != epoch {
                return;
            }
            match result {
                Ok(AnnouncementClaim {
                    claimed: true,
                    item: Some(item),
                }) if item.id == expected && announcement_is_eligible(&item, SystemTime::now()) => {
                    context.announcements.borrow_mut().ready = Some(item);
                }
                Ok(_) => {}
                Err(error) => quiet_announcement_error(app, context, capture, error),
            }
        },
    );
}

fn show_announcement(
    app: &AppWindow,
    context: AppContext,
    capture: NotificationCapture,
    item: ServerNotification,
) {
    let id = item.id.clone();
    let pending = context
        .announcements
        .borrow()
        .queue
        .len()
        .min(i32::MAX as usize) as i32;
    if apply_notification_ui(&context, &capture, || {
        let state = app.global::<AppState>();
        state.set_announcement_title(item.title.into());
        state.set_announcement_body(item.body.into());
        state.set_announcement_time(format_notification_time(&item.created_at).into());
        state.set_announcement_important(true);
        state.set_announcement_pending_count(pending);
        state.set_announcement_open(true);
        context.announcements.borrow_mut().current_id = Some(id.clone());
    })
    .is_none()
    {
        return;
    }
    let receiver = match spawn_notification_thread(&context, &capture, move |api, scope| {
        api.announcement_shown_scoped(&id, scope)
    }) {
        Ok(receiver) => receiver,
        Err(_) => return,
    };
    await_announcement_result(
        app.as_weak(),
        context,
        capture,
        receiver,
        |app, context, capture, result| {
            if let Err(error) = result {
                quiet_announcement_error(app, context, capture, error);
            }
        },
    );
}

fn quiet_announcement_error(
    app: &AppWindow,
    context: &AppContext,
    capture: &NotificationCapture,
    error: ApiError,
) {
    if notification_session_ended(&error) {
        notification_error(app, context, capture, None, false, error);
    }
}

fn await_announcement_result<T: 'static>(
    weak: Weak<AppWindow>,
    context: AppContext,
    capture: NotificationCapture,
    receiver: mpsc::Receiver<std::result::Result<T, ApiError>>,
    completion: impl FnOnce(&AppWindow, &AppContext, &NotificationCapture, std::result::Result<T, ApiError>)
        + 'static,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let _ = reap_notification_workers();
        if notification_worker_pending(&capture) {
            await_announcement_result(weak, context, capture, receiver, completion);
            return;
        }
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                await_announcement_result(weak, context, capture, receiver, completion);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(ApiError::LocalState {
                message: "公告检查任务已结束".into(),
            }),
        };
        if let Some(app) = weak.upgrade() {
            match result {
                Err(error) if notification_session_ended(&error) => {
                    quiet_announcement_error(&app, &context, &capture, error)
                }
                result => completion(&app, &context, &capture, result),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_announcements_cannot_reappear_after_local_read_or_delete() {
        let context = AppContext::default();
        let item = ServerNotification {
            id: Uuid::new_v4().to_string(),
            notification_type: "system.announcement".into(),
            title: "维护通知".into(),
            body: "正文".into(),
            created_at: "2026-10-15T00:00:00Z".into(),
            read_at: None,
            metadata: serde_json::json!({"importance":"important","expires_at":"2099-01-01T00:00:00Z"}),
        };
        context
            .store
            .borrow_mut()
            .notifications
            .push(notification_data(item.clone()));
        assert!(queued_announcement_can_show(
            &context,
            &item,
            SystemTime::now()
        ));
        context.store.borrow_mut().notifications[0].read = true;
        assert!(!queued_announcement_can_show(
            &context,
            &item,
            SystemTime::now()
        ));
        context.store.borrow_mut().notifications.clear();
        assert!(!queued_announcement_can_show(
            &context,
            &item,
            SystemTime::now()
        ));
    }
    #[test]
    fn announcements_require_valid_expiry_and_never_use_task_title_failure_markers() {
        let mut item = ServerNotification {
            id: Uuid::new_v4().to_string(),
            notification_type: "system.announcement".into(),
            title: "故障修复公告".into(),
            body: "正文".into(),
            created_at: "2026-10-15T00:00:00Z".into(),
            read_at: None,
            metadata: serde_json::json!({"importance":"important","expires_at":"2099-01-01T00:00:00Z","revoked_at":null}),
        };
        assert!(announcement_is_eligible(&item, SystemTime::now()));
        for expiry in ["invalid", "2000-01-01T00:00:00Z", ""] {
            item.metadata["expires_at"] = expiry.into();
            assert!(!announcement_is_eligible(&item, SystemTime::now()));
        }
        item.metadata["expires_at"] = "2099-01-01T00:00:00Z".into();
        item.metadata["revoked_at"] = "2026-10-15T00:00:00Z".into();
        assert!(!announcement_is_eligible(&item, SystemTime::now()));
        item.metadata["revoked_at"] = Value::Null;
        item.read_at = Some("2026-10-15T00:00:00Z".into());
        assert!(!announcement_is_eligible(&item, SystemTime::now()));
    }
}

#[cfg(test)]
#[path = "announcement_ui_tests.rs"]
mod ui_tests;
