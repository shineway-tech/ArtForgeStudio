use super::*;
use std::cell::Cell;

enum BindingPollOutcome {
    Pending,
    Scanned(String),
    Failed(String),
    Completed(WechatBindingStatusResponse),
}

enum BindingReceiverPoll<T> {
    Pending,
    Ready(T),
    Disconnected,
}

pub(super) fn wire_wechat_binding_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();
    let operation_epoch = Rc::new(Cell::new(0_u64));

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        let operation_epoch = operation_epoch.clone();
        state.on_start_wechat_binding(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_wechat_binding(
                &app,
                context.clone(),
                backend.clone(),
                operation_epoch.clone(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let operation_epoch = operation_epoch.clone();
        state.on_close_wechat_binding(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            advance_wechat_operation(&operation_epoch);
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
        let backend = backend.clone();
        let context = context.clone();
        let operation_epoch = operation_epoch.clone();
        state.on_unbind_wechat(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_wechat_bind_busy() || !state.get_wechat_can_unbind() {
                return;
            }
            let Some(session_scope) = context.current_account_session_scope() else {
                state.set_generation_status("登录状态已失效，请重新登录".into());
                return;
            };
            let request_epoch = advance_wechat_operation(&operation_epoch);
            state.set_wechat_bind_busy(true);
            state.set_wechat_unbind_confirm_open(false);
            state.set_wechat_bind_login_id("".into());
            state.set_wechat_bind_qr_ready(false);
            state.set_wechat_bind_scanned(false);
            let api = AccountApi::new(backend.api.clone());
            let worker_scope = session_scope.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(api.unbind_wechat_scoped(&worker_scope));
            });
            poll_unbind_result(
                app.as_weak(),
                context.clone(),
                operation_epoch.clone(),
                request_epoch,
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }
}

fn start_wechat_binding(
    app: &AppWindow,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    operation_epoch: Rc<Cell<u64>>,
) {
    let state = app.global::<AppState>();
    if state.get_wechat_bind_busy() || state.get_wechat_bound() {
        return;
    }
    if state.get_session_state().as_str() != "online" {
        state.set_generation_status("请先联网并登录后再绑定微信".into());
        return;
    }
    let Some(session_scope) = context.current_account_session_scope() else {
        state.set_generation_status("登录状态已失效，请重新登录".into());
        return;
    };
    let request_epoch = advance_wechat_operation(&operation_epoch);
    state.set_wechat_bind_open(true);
    state.set_wechat_bind_busy(true);
    state.set_wechat_bind_qr_ready(false);
    state.set_wechat_bind_scanned(false);
    state.set_wechat_bind_poll_elapsed_ms(0);
    state.set_wechat_bind_login_id("".into());
    state.set_wechat_bind_status("正在获取绑定二维码...".into());
    let api = AccountApi::new(backend.api.clone());
    let worker_scope = session_scope.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(api.start_wechat_binding_scoped(&worker_scope));
    });
    poll_binding_start_result(
        app.as_weak(),
        context,
        backend,
        operation_epoch,
        request_epoch,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_binding_start_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    operation_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Result<WechatBindingStartResponse, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !wechat_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &operation_epoch,
            request_epoch,
            None,
            &receiver,
        ) {
            return;
        }
        let result = match poll_binding_receiver(&receiver) {
            BindingReceiverPoll::Pending => {
                poll_binding_start_result(
                    app_weak,
                    context,
                    backend,
                    operation_epoch,
                    request_epoch,
                    session_scope,
                    receiver,
                );
                return;
            }
            BindingReceiverPoll::Ready(result) => result,
            BindingReceiverPoll::Disconnected => Err(ApiError::LocalState {
                message: "微信绑定二维码任务意外中断".to_string(),
            }),
        };
        if !wechat_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &operation_epoch,
            request_epoch,
            None,
            &receiver,
        ) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_wechat_bind_busy(false);
        if !state.get_wechat_bind_open() {
            return;
        }
        match result {
            Ok(response) => match if response.qr_image_base64.trim().is_empty() {
                qr_image(&response.authorization_url)
            } else {
                encoded_image(&response.qr_image_base64)
            } {
                Ok(image) => {
                    let expires = response.expires_in_seconds.min(i32::MAX as u64) as i32;
                    let poll_after_ms = response
                        .poll_after_milliseconds
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
                        context,
                        backend,
                        operation_epoch,
                        request_epoch,
                        session_scope,
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
    context: AppContext,
    backend: Arc<BackendRuntime>,
    operation_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    session_scope: SessionScope,
    login_id: String,
    delay_milliseconds: u64,
) {
    slint::Timer::single_shot(
        Duration::from_millis(delay_milliseconds.max(250)),
        move || {
            match context.account_scope_disposition(&session_scope) {
                AccountScopeDisposition::Current => {}
                AccountScopeDisposition::CapturedTerminal => {
                    if let Some(app) = app_weak.upgrade() {
                        sign_out_locally(
                            &app,
                            &context,
                            true,
                            Some(session_scope.auth_epoch),
                        );
                    }
                    return;
                }
                AccountScopeDisposition::Stale => return,
            }
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !wechat_operation_matches(
                operation_epoch.get(),
                request_epoch,
                state.get_wechat_bind_login_id().as_str(),
                &login_id,
            ) || !state.get_wechat_bind_open()
            {
                return;
            }
            let api = AccountApi::new(backend.api.clone());
            let request_login_id = login_id.clone();
            let worker_scope = session_scope.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let result = api
                    .wechat_binding_status_scoped(&request_login_id, &worker_scope)
                    .map(|status| match (status.status.as_str(), status.qr_status.as_deref()) {
                        ("pending", Some("scanned")) | ("scanned", _) => {
                            BindingPollOutcome::Scanned(status.message.unwrap_or_else(|| {
                                "已扫码，请在手机微信中确认绑定".to_string()
                            }))
                        }
                        ("pending", _) => BindingPollOutcome::Pending,
                        ("failed", _) => {
                            BindingPollOutcome::Failed(status.message.unwrap_or_else(|| {
                                "微信绑定未完成，请刷新二维码重试".to_string()
                            }))
                        }
                        ("completed", _) => BindingPollOutcome::Completed(status),
                        _ => BindingPollOutcome::Failed(
                            "微信绑定状态异常，请刷新二维码重试".to_string(),
                        ),
                    });
                let _ = sender.send(result);
            });
            poll_binding_status_result(
                app.as_weak(),
                context,
                backend,
                operation_epoch,
                request_epoch,
                session_scope,
                login_id,
                Rc::new(RefCell::new(Some(receiver))),
            );
        },
    );
}

fn poll_binding_status_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    backend: Arc<BackendRuntime>,
    operation_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    session_scope: SessionScope,
    login_id: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Result<BindingPollOutcome, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !wechat_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &operation_epoch,
            request_epoch,
            Some(&login_id),
            &receiver,
        ) {
            return;
        }
        let result = match poll_binding_receiver(&receiver) {
            BindingReceiverPoll::Pending => {
                poll_binding_status_result(
                    app_weak,
                    context,
                    backend,
                    operation_epoch,
                    request_epoch,
                    session_scope,
                    login_id,
                    receiver,
                );
                return;
            }
            BindingReceiverPoll::Ready(result) => result,
            BindingReceiverPoll::Disconnected => Err(ApiError::LocalState {
                message: "微信绑定状态任务意外中断".to_string(),
            }),
        };
        if !wechat_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &operation_epoch,
            request_epoch,
            Some(&login_id),
            &receiver,
        ) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
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
                    context,
                    backend,
                    operation_epoch,
                    request_epoch,
                    session_scope,
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
                    context,
                    backend,
                    operation_epoch,
                    request_epoch,
                    session_scope,
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
    context: AppContext,
    operation_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Result<WechatAuthMethod, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !wechat_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &operation_epoch,
            request_epoch,
            Some(""),
            &receiver,
        ) {
            return;
        }
        let result = match poll_binding_receiver(&receiver) {
            BindingReceiverPoll::Pending => {
                poll_unbind_result(
                    app_weak,
                    context,
                    operation_epoch,
                    request_epoch,
                    session_scope,
                    receiver,
                );
                return;
            }
            BindingReceiverPoll::Ready(result) => result,
            BindingReceiverPoll::Disconnected => Err(ApiError::LocalState {
                message: "微信解绑任务意外中断".to_string(),
            }),
        };
        if !wechat_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &operation_epoch,
            request_epoch,
            Some(""),
            &receiver,
        ) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
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
                state.set_generation_status(
                    format!("解绑微信失败：{}", error.user_message()).into(),
                );
            }
        }
    });
}

fn wechat_poll_is_current<T>(
    app_weak: &Weak<AppWindow>,
    context: &AppContext,
    session_scope: &SessionScope,
    operation_epoch: &Cell<u64>,
    request_epoch: u64,
    login_id: Option<&str>,
    receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>,
) -> bool {
    match context.account_scope_disposition(session_scope) {
        AccountScopeDisposition::Current => {}
        AccountScopeDisposition::CapturedTerminal => {
            receiver.borrow_mut().take();
            if let Some(app) = app_weak.upgrade() {
                sign_out_locally(
                    &app,
                    context,
                    true,
                    Some(session_scope.auth_epoch),
                );
            }
            return false;
        }
        AccountScopeDisposition::Stale => {
            receiver.borrow_mut().take();
            return false;
        }
    }
    if operation_epoch.get() != request_epoch {
        receiver.borrow_mut().take();
        return false;
    }
    if let Some(login_id) = login_id {
        let Some(app) = app_weak.upgrade() else {
            receiver.borrow_mut().take();
            return false;
        };
        if app
            .global::<AppState>()
            .get_wechat_bind_login_id()
            .as_str()
            != login_id
        {
            receiver.borrow_mut().take();
            return false;
        }
    }
    true
}

fn poll_binding_receiver<T>(
    receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>,
) -> BindingReceiverPoll<T> {
    let mut slot = receiver.borrow_mut();
    let Some(receiver) = slot.as_ref() else {
        return BindingReceiverPoll::Disconnected;
    };
    match receiver.try_recv() {
        Ok(value) => {
            slot.take();
            BindingReceiverPoll::Ready(value)
        }
        Err(TryRecvError::Empty) => BindingReceiverPoll::Pending,
        Err(TryRecvError::Disconnected) => {
            slot.take();
            BindingReceiverPoll::Disconnected
        }
    }
}

fn advance_wechat_operation(operation_epoch: &Cell<u64>) -> u64 {
    let next = operation_epoch.get().wrapping_add(1);
    operation_epoch.set(next);
    next
}

fn wechat_operation_matches(
    current_operation_epoch: u64,
    request_epoch: u64,
    current_login_id: &str,
    request_login_id: &str,
) -> bool {
    current_operation_epoch == request_epoch && current_login_id == request_login_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wechat_response_requires_matching_operation_and_login_id() {
        assert!(wechat_operation_matches(7, 7, "login-a", "login-a"));
        assert!(!wechat_operation_matches(8, 7, "login-a", "login-a"));
        assert!(!wechat_operation_matches(7, 7, "login-b", "login-a"));
    }
}
