use super::*;

pub(super) fn wire_notification_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        let backend = backend.clone();
        let context = context.clone();
        state.on_mark_notification_read(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(session_scope) = current_notification_session_scope(&context) else {
                return;
            };
            let id = id.to_string();
            {
                let mut store = store.borrow_mut();
                if let Some(item) = store.notifications.iter_mut().find(|item| item.id == id) {
                    item.read = true;
                }
                push_notifications(&app, &store);
            }
            let api = NotificationsApi::new(backend.api.clone());
            let (sender, receiver) = mpsc::channel();
            let worker_scope = session_scope.clone();
            std::thread::spawn(move || {
                let _ = sender.send(api.mark_read_scoped(&id, &worker_scope));
            });
            poll_notification_operation(
                app.as_weak(),
                context.clone(),
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
                None,
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        let backend = backend.clone();
        let context = context.clone();
        state.on_mark_all_notifications_read(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(session_scope) = current_notification_session_scope(&context) else {
                return;
            };
            {
                let mut store = store.borrow_mut();
                for item in &mut store.notifications {
                    item.read = true;
                }
                push_notifications(&app, &store);
            }
            let api = NotificationsApi::new(backend.api.clone());
            let (sender, receiver) = mpsc::channel();
            let worker_scope = session_scope.clone();
            std::thread::spawn(move || {
                let _ = sender.send(api.mark_all_read_scoped(&worker_scope));
            });
            poll_notification_operation(
                app.as_weak(),
                context.clone(),
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
                None,
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        let backend = backend.clone();
        let context = context.clone();
        state.on_delete_notification(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(session_scope) = current_notification_session_scope(&context) else {
                return;
            };
            let id = id.to_string();
            {
                let mut store = store.borrow_mut();
                store.notifications.retain(|item| item.id != id);
                push_notifications(&app, &store);
            }
            let api = NotificationsApi::new(backend.api.clone());
            let (sender, receiver) = mpsc::channel();
            let worker_scope = session_scope.clone();
            std::thread::spawn(move || {
                let _ = sender.send(api.delete_scoped(&id, &worker_scope));
            });
            poll_notification_operation(
                app.as_weak(),
                context.clone(),
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
                Some("通知删除失败"),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        let backend = backend.clone();
        let context = context.clone();
        state.on_clear_all_notifications(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(session_scope) = current_notification_session_scope(&context) else {
                return;
            };
            {
                let mut store = store.borrow_mut();
                store.notifications.clear();
                store.notification_page_epoch = store.notification_page_epoch.wrapping_add(1);
                push_notifications(&app, &store);
            }
            reset_notification_pagination_ui(&app);
            let api = NotificationsApi::new(backend.api.clone());
            let (sender, receiver) = mpsc::channel();
            let worker_scope = session_scope.clone();
            std::thread::spawn(move || {
                let _ = sender.send(api.delete_all_scoped(&worker_scope));
            });
            poll_notification_operation(
                app.as_weak(),
                context.clone(),
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
                Some("通知清空失败"),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_load_more_notifications(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_notification_page_loading() || !state.get_notification_page_has_more() {
                return;
            }
            let cursor = state.get_notification_next_cursor().trim().to_string();
            if cursor.is_empty() {
                state.set_notification_page_has_more(false);
                return;
            }
            start_notification_page(&app, context.clone(), Some(cursor), true);
        });
    }
}

pub(super) fn refresh_server_notifications(app: &AppWindow, context: AppContext) {
    start_notification_page(app, context, None, false);
}

pub(super) fn clear_notification_account_state(app: &AppWindow, context: &AppContext) {
    let mut store = context.store.borrow_mut();
    store.notifications.clear();
    store.notification_page_epoch = store.notification_page_epoch.wrapping_add(1);
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

fn start_notification_page(
    app: &AppWindow,
    context: AppContext,
    cursor: Option<String>,
    append: bool,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let Some(session_scope) = current_notification_session_scope(&context) else {
        return;
    };
    let request_epoch = {
        let mut store = context.store.borrow_mut();
        store.notification_page_epoch = store.notification_page_epoch.wrapping_add(1);
        store.notification_page_epoch
    };
    let state = app.global::<AppState>();
    state.set_notification_page_loading(true);
    state.set_notification_page_message("".into());
    if !append {
        state.set_notification_page_has_more(false);
        state.set_notification_next_cursor("".into());
    }

    let (sender, receiver) = mpsc::channel();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let result = NotificationsApi::new(backend.api.clone())
            .list_page_scoped(cursor.as_deref(), &worker_scope);
        let _ = sender.send(result);
    });
    poll_server_notifications(
        app.as_weak(),
        context,
        session_scope,
        request_epoch,
        append,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn current_notification_session_scope(context: &AppContext) -> Option<SessionScope> {
    let owner_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone()
        .filter(|value| !value.trim().is_empty())?;
    context
        .backend
        .as_ref()?
        .api
        .session()
        .scope_for_user(&owner_user_id)
}

fn notification_scope_is_current(
    current_user_id: &Arc<Mutex<Option<String>>>,
    session: &Arc<SessionManager>,
    scope: &SessionScope,
) -> bool {
    current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .as_deref()
        == Some(scope.owner_user_id.as_str())
        && session.is_scope_current(scope)
}

fn notification_session_ended(error: &ApiError) -> bool {
    matches!(error, ApiError::AuthenticationRequired) || error.is_terminal_session_error()
}

fn handle_notification_operation_error(
    app: &AppWindow,
    context: &AppContext,
    scope: &SessionScope,
    error: ApiError,
    status_prefix: Option<&str>,
) {
    if notification_session_ended(&error)
        && terminal_auth_scope_matches_context(context, scope)
    {
        sign_out_locally(app, context, true, Some(scope.auth_epoch));
        return;
    }
    let Some(backend) = context.backend.as_ref() else {
        return;
    };
    if !notification_scope_is_current(
        &context.current_user_id,
        backend.api.session(),
        scope,
    ) {
        return;
    }
    if let Some(prefix) = status_prefix {
        app.global::<AppState>().set_generation_status(
            format!("{prefix}：{}", error.user_message()).into(),
        );
    }
}

fn poll_notification_operation(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<(), ApiError>>>>>,
    status_prefix: Option<&'static str>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    return;
                }
            }
        };
        let Some(result) = result else {
            poll_notification_operation(
                app_weak,
                context,
                session_scope,
                receiver,
                status_prefix,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if let Err(error) = result {
            handle_notification_operation_error(
                &app,
                &context,
                &session_scope,
                error,
                status_prefix,
            );
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

fn poll_server_notifications(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    request_epoch: u64,
    append: bool,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<NotificationPage, ApiError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(ApiError::LocalState {
                        message: "通知加载任务意外中断".to_string(),
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_server_notifications(
                app_weak,
                context,
                session_scope,
                request_epoch,
                append,
                receiver,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if context.store.borrow().notification_page_epoch != request_epoch {
            return;
        }
        let state = app.global::<AppState>();
        match result {
            Ok(page) => {
                let Some(backend) = context.backend.as_ref() else {
                    return;
                };
                if !notification_scope_is_current(
                    &context.current_user_id,
                    backend.api.session(),
                    &session_scope,
                ) {
                    return;
                }
                state.set_notification_page_loading(false);
                let mapped = page
                    .items
                    .into_iter()
                    .map(notification_data)
                    .collect::<Vec<_>>();
                let mut store = context.store.borrow_mut();
                if append {
                    let mut ids = store
                        .notifications
                        .iter()
                        .map(|item| item.id.clone())
                        .collect::<BTreeSet<_>>();
                    store.notifications.extend(
                        mapped
                            .into_iter()
                            .filter(|item| ids.insert(item.id.clone())),
                    );
                } else {
                    store.notifications = mapped;
                }
                push_notifications(&app, &store);
                drop(store);
                let next_cursor = page.next_cursor.unwrap_or_default();
                state.set_notification_page_has_more(!next_cursor.is_empty());
                state.set_notification_next_cursor(next_cursor.into());
                state.set_notification_page_message("".into());
            }
            Err(error) => {
                if notification_session_ended(&error)
                    && terminal_auth_scope_matches_context(&context, &session_scope)
                {
                    handle_notification_operation_error(
                        &app,
                        &context,
                        &session_scope,
                        error,
                        None,
                    );
                    return;
                }
                let Some(backend) = context.backend.as_ref() else {
                    return;
                };
                if !notification_scope_is_current(
                    &context.current_user_id,
                    backend.api.session(),
                    &session_scope,
                ) {
                    return;
                }
                state.set_notification_page_loading(false);
                state.set_notification_page_message(
                    format!("通知加载失败：{}", error.user_message()).into(),
                );
                handle_notification_operation_error(
                    &app,
                    &context,
                    &session_scope,
                    error,
                    None,
                );
            }
        }
    });
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
