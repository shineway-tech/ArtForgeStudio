use super::*;

enum BindingPollOutcome {
    Pending,
    Scanned(String),
    Failed(String),
    Completed(WechatBindingStatusResponse),
}

pub(super) fn wire_wechat_binding_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else { return; };
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        state.on_start_wechat_binding(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            start_wechat_binding(&app, backend.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_wechat_binding(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            state.set_wechat_bind_open(false);
            state.set_wechat_bind_login_id("".into());
            state.set_wechat_bind_qr_ready(false);
            state.set_wechat_bind_scanned(false);
            state.set_wechat_bind_busy(false);
            state.set_wechat_bind_status("".into());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_unbind_wechat(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            if state.get_wechat_bind_busy() || !state.get_wechat_can_unbind() {
                return;
            }
            state.set_wechat_bind_busy(true);
            state.set_wechat_unbind_confirm_open(false);
            let api = AccountApi::new(backend.api.clone());
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(api.unbind_wechat());
            });
            poll_unbind_result(
                app.as_weak(),
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }
}

fn start_wechat_binding(app: &AppWindow, backend: Arc<BackendRuntime>) {
    let state = app.global::<AppState>();
    if state.get_wechat_bind_busy() || state.get_wechat_bound() {
        return;
    }
    if state.get_session_state().as_str() != "online" {
        state.set_generation_status("请先联网并登录后再绑定微信".into());
        return;
    }
    state.set_wechat_bind_open(true);
    state.set_wechat_bind_busy(true);
    state.set_wechat_bind_qr_ready(false);
    state.set_wechat_bind_scanned(false);
    state.set_wechat_bind_poll_elapsed_ms(0);
    state.set_wechat_bind_login_id("".into());
    state.set_wechat_bind_status("正在获取绑定二维码...".into());
    let api = AccountApi::new(backend.api.clone());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(api.start_wechat_binding());
    });
    poll_binding_start_result(
        app.as_weak(),
        backend,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_binding_start_result(
    app_weak: Weak<AppWindow>,
    backend: Arc<BackendRuntime>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Result<WechatBindingStartResponse, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = poll_binding_receiver(&receiver);
        let Some(result) = result else {
            poll_binding_start_result(app_weak, backend, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        state.set_wechat_bind_busy(false);
        if !state.get_wechat_bind_open() { return; }
        match result {
            Ok(response) => match if response.qr_image_base64.trim().is_empty() {
                qr_image(&response.authorization_url)
            } else {
                encoded_image(&response.qr_image_base64)
            } {
                Ok(image) => {
                    let expires = response.expires_in_seconds.min(i32::MAX as u64) as i32;
                    let poll_after_ms = response.poll_after_milliseconds
                        .unwrap_or_else(|| response.poll_after_seconds.saturating_mul(1000))
                        .clamp(250, 10_000) as i32;
                    state.set_wechat_bind_qr_image(image);
                    state.set_wechat_bind_qr_ready(true);
                    state.set_wechat_bind_login_id(response.login_id.clone().into());
                    state.set_wechat_bind_expires_in(expires);
                    state.set_wechat_bind_poll_after_ms(poll_after_ms);
                    state.set_wechat_bind_poll_elapsed_ms(0);
                    state.set_wechat_bind_status(
                        format!("请使用微信扫码，二维码 {expires} 秒后失效").into(),
                    );
                    schedule_binding_status_poll(
                        app.as_weak(),
                        backend,
                        response.login_id,
                        poll_after_ms as u64,
                    );
                }
                Err(_) => {
                    state.set_wechat_bind_qr_ready(false);
                    state.set_wechat_bind_status("二维码生成失败，请点击刷新".into());
                }
            },
            Err(error) => {
                state.set_wechat_bind_qr_ready(false);
                state.set_wechat_bind_status(error.user_message().into());
            }
        }
    });
}

fn schedule_binding_status_poll(
    app_weak: Weak<AppWindow>,
    backend: Arc<BackendRuntime>,
    login_id: String,
    delay_milliseconds: u64,
) {
    slint::Timer::single_shot(Duration::from_millis(delay_milliseconds.max(250)), move || {
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if !state.get_wechat_bind_open()
            || state.get_wechat_bind_login_id().as_str() != login_id
        {
            return;
        }
        let api = AccountApi::new(backend.api.clone());
        let request_login_id = login_id.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = api.wechat_binding_status(&request_login_id).map(|status| {
                match (status.status.as_str(), status.qr_status.as_deref()) {
                    ("pending", Some("scanned")) | ("scanned", _) => BindingPollOutcome::Scanned(
                        status.message.unwrap_or_else(|| "已扫码，请在手机微信中确认绑定".to_string()),
                    ),
                    ("pending", _) => BindingPollOutcome::Pending,
                    ("failed", _) => BindingPollOutcome::Failed(
                        status.message.unwrap_or_else(|| "微信绑定未完成，请刷新二维码重试".to_string()),
                    ),
                    ("completed", _) => BindingPollOutcome::Completed(status),
                    _ => BindingPollOutcome::Failed("微信绑定状态异常，请刷新二维码重试".to_string()),
                }
            });
            let _ = sender.send(result);
        });
        poll_binding_status_result(
            app.as_weak(),
            backend,
            login_id,
            Rc::new(RefCell::new(Some(receiver))),
        );
    });
}

fn poll_binding_status_result(
    app_weak: Weak<AppWindow>,
    backend: Arc<BackendRuntime>,
    login_id: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Result<BindingPollOutcome, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = poll_binding_receiver(&receiver);
        let Some(result) = result else {
            poll_binding_status_result(app_weak, backend, login_id, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if state.get_wechat_bind_login_id().as_str() != login_id { return; }
        match result {
            Ok(BindingPollOutcome::Pending) => {
                let poll_after_ms = state.get_wechat_bind_poll_after_ms().max(250);
                let (remaining, elapsed_ms) = advance_second_countdown(
                    state.get_wechat_bind_expires_in(),
                    state.get_wechat_bind_poll_elapsed_ms(),
                    poll_after_ms,
                );
                state.set_wechat_bind_expires_in(remaining);
                state.set_wechat_bind_poll_elapsed_ms(elapsed_ms);
                if remaining == 0 {
                    state.set_wechat_bind_login_id("".into());
                    state.set_wechat_bind_qr_ready(false);
                    state.set_wechat_bind_scanned(false);
                    state.set_wechat_bind_status("绑定二维码已失效，请点击刷新".into());
                    return;
                }
                state.set_wechat_bind_status(
                    format!("请使用微信扫码，二维码 {remaining} 秒后失效").into(),
                );
                schedule_binding_status_poll(
                    app.as_weak(),
                    backend,
                    login_id,
                    poll_after_ms as u64,
                );
            }
            Ok(BindingPollOutcome::Scanned(message)) => {
                let poll_after_ms = state.get_wechat_bind_poll_after_ms().max(250);
                let (remaining, elapsed_ms) = advance_second_countdown(
                    state.get_wechat_bind_expires_in(),
                    state.get_wechat_bind_poll_elapsed_ms(),
                    poll_after_ms,
                );
                state.set_wechat_bind_expires_in(remaining);
                state.set_wechat_bind_poll_elapsed_ms(elapsed_ms);
                if remaining == 0 {
                    state.set_wechat_bind_login_id("".into());
                    state.set_wechat_bind_qr_ready(false);
                    state.set_wechat_bind_scanned(false);
                    state.set_wechat_bind_status("绑定二维码已失效，请点击刷新".into());
                    return;
                }
                state.set_wechat_bind_scanned(true);
                state.set_wechat_bind_status(message.into());
                schedule_binding_status_poll(
                    app.as_weak(),
                    backend,
                    login_id,
                    poll_after_ms as u64,
                );
            }
            Ok(BindingPollOutcome::Failed(message)) => {
                state.set_wechat_bind_login_id("".into());
                state.set_wechat_bind_qr_ready(false);
                state.set_wechat_bind_scanned(false);
                state.set_wechat_bind_status(message.into());
            }
            Ok(BindingPollOutcome::Completed(status)) => {
                state.set_wechat_bound(status.bound);
                state.set_wechat_can_unbind(status.can_unbind.unwrap_or(true));
                let nickname = status.nickname.unwrap_or_default();
                state.set_wechat_bound_name(nickname.clone().into());
                if !nickname.trim().is_empty() {
                    state.set_nickname(nickname.into());
                    save_user_profile(&app);
                }
                state.set_wechat_bind_login_id("".into());
                state.set_wechat_bind_qr_ready(false);
                state.set_wechat_bind_scanned(false);
                state.set_wechat_bind_open(false);
                state.set_wechat_bind_status("".into());
                state.set_generation_status("微信绑定成功".into());
            }
            Err(error) => {
                state.set_wechat_bind_login_id("".into());
                state.set_wechat_bind_qr_ready(false);
                state.set_wechat_bind_scanned(false);
                state.set_wechat_bind_status(error.user_message().into());
            }
        }
    });
}

fn poll_unbind_result(
    app_weak: Weak<AppWindow>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Result<WechatAuthMethod, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = poll_binding_receiver(&receiver);
        let Some(result) = result else {
            poll_unbind_result(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        state.set_wechat_bind_busy(false);
        match result {
            Ok(status) => {
                state.set_wechat_bound(status.bound);
                state.set_wechat_can_unbind(status.can_unbind);
                state.set_wechat_bound_name(status.nickname.unwrap_or_default().into());
                state.set_generation_status("微信已解绑".into());
            }
            Err(error) => {
                state.set_generation_status(format!("解绑微信失败：{}", error.user_message()).into());
            }
        }
    });
}

fn poll_binding_receiver<T>(receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>) -> Option<T> {
    let mut slot = receiver.borrow_mut();
    let receiver = slot.as_ref()?;
    match receiver.try_recv() {
        Ok(value) => {
            slot.take();
            Some(value)
        }
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            slot.take();
            None
        }
    }
}
