use super::*;

pub(super) fn wire_notification_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else { return; };
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        let backend = backend.clone();
        state.on_mark_notification_read(move |id| {
            let Some(app) = app_weak.upgrade() else { return; };
            let id = id.to_string();
            {
                let mut store = store.borrow_mut();
                if let Some(item) = store.notifications.iter_mut().find(|item| item.id == id) {
                    item.read = true;
                }
                push_notifications(&app, &store);
            }
            let api = NotificationsApi::new(backend.api.clone());
            std::thread::spawn(move || { let _ = api.mark_read(&id); });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        let backend = backend.clone();
        state.on_mark_all_notifications_read(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            {
                let mut store = store.borrow_mut();
                for item in &mut store.notifications { item.read = true; }
                push_notifications(&app, &store);
            }
            let api = NotificationsApi::new(backend.api.clone());
            std::thread::spawn(move || { let _ = api.mark_all_read(); });
        });
    }

    state.on_clear_all_notifications(move || {});
}

pub(super) fn refresh_server_notifications(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else { return; };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(NotificationsApi::new(backend.api.clone()).list());
    });
    poll_server_notifications(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_server_notifications(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<Vec<ServerNotification>, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = receiver.borrow().as_ref().and_then(|value| value.try_recv().ok());
        let Some(result) = result else {
            poll_server_notifications(app_weak, context, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        match result {
            Ok(items) => {
                let mut store = context.store.borrow_mut();
                store.notifications = items.into_iter().map(|item| {
                    let model = item.metadata.get("model_name")
                        .or_else(|| item.metadata.get("model_code"))
                        .and_then(|value| value.as_str()).unwrap_or("").to_string();
                    let success = !item.notification_type.contains("failed")
                        && !item.notification_type.contains("expired");
                    NotificationData {
                        id: item.id,
                        title: item.title,
                        model,
                        time: format_notification_time(&item.created_at),
                        reason: item.body,
                        success,
                        read: item.read_at.is_some(),
                    }
                }).collect();
                push_notifications(&app, &store);
            }
            Err(error) => app.global::<AppState>().set_generation_status(
                format!("通知刷新失败：{error}").into(),
            ),
        }
    });
}

fn format_notification_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| value.to_string())
}
