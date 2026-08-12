use super::*;
use std::cell::Cell;

pub(super) fn wire_email_binding_callbacks(app: &AppWindow, context: AppContext) {
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
        state.on_request_email_binding_code(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_email_bind_code_busy()
                || state.get_email_bind_busy()
                || state.get_email_bind_countdown() > 0
            {
                return;
            }
            let email = normalized_binding_email(state.get_email_bind_email().as_str());
            if !valid_binding_email(&email) {
                state.set_email_bind_status("请输入正确的邮箱地址".into());
                return;
            }
            let Some(session_scope) = context.current_account_session_scope() else {
                state.set_email_bind_status("登录状态已失效，请重新登录".into());
                return;
            };
            let request_epoch = advance_operation_epoch(&operation_epoch);
            state.set_email_bind_code_busy(true);
            state.set_email_bind_status("正在发送验证码...".into());
            let api = AccountApi::new(backend.api.clone());
            let worker_scope = session_scope.clone();
            let worker_email = email.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(
                    api.request_email_binding_code_scoped(&worker_email, &worker_scope),
                );
            });
            poll_email_binding_code_result(
                app.as_weak(),
                context.clone(),
                operation_epoch.clone(),
                request_epoch,
                email,
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        let operation_epoch = operation_epoch.clone();
        state.on_bind_email(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_email_bind_busy() || state.get_email_bind_code_busy() {
                return;
            }
            let email = normalized_binding_email(state.get_email_bind_email().as_str());
            let code = state.get_email_bind_code().trim().to_string();
            if !valid_binding_email(&email) {
                state.set_email_bind_status("请输入正确的邮箱地址".into());
                return;
            }
            if code.len() != 6 || !code.chars().all(|value| value.is_ascii_digit()) {
                state.set_email_bind_status("请输入 6 位数字验证码".into());
                return;
            }
            let Some(session_scope) = context.current_account_session_scope() else {
                state.set_email_bind_status("登录状态已失效，请重新登录".into());
                return;
            };
            let request_epoch = advance_operation_epoch(&operation_epoch);
            state.set_email_bind_busy(true);
            state.set_email_bind_status("正在绑定邮箱...".into());
            let api = AccountApi::new(backend.api.clone());
            let worker_scope = session_scope.clone();
            let worker_email = email.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(api.bind_email_scoped(&worker_email, &code, &worker_scope));
            });
            poll_email_binding_result(
                app.as_weak(),
                context.clone(),
                operation_epoch.clone(),
                request_epoch,
                email,
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let operation_epoch = operation_epoch.clone();
        state.on_close_email_binding(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            advance_operation_epoch(&operation_epoch);
            let state = app.global::<AppState>();
            state.set_email_bind_open(false);
            state.set_email_bind_email("".into());
            state.set_email_bind_code("".into());
            state.set_email_bind_code_busy(false);
            state.set_email_bind_busy(false);
            state.set_email_bind_countdown(0);
            state.set_email_bind_status("".into());
        });
    }
}

fn poll_email_binding_code_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    operation_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    email: String,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Result<EmailBindingCodeResponse, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !account_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &receiver,
        ) {
            return;
        }
        if operation_epoch.get() != request_epoch {
            receiver.borrow_mut().take();
            return;
        }
        let result = poll_email_receiver(&receiver, "邮箱验证码发送任务意外中断");
        let Some(result) = result else {
            poll_email_binding_code_result(
                app_weak,
                context,
                operation_epoch,
                request_epoch,
                email,
                session_scope,
                receiver,
            );
            return;
        };
        if !account_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &receiver,
        ) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !email_operation_matches(
            operation_epoch.get(),
            request_epoch,
            state.get_email_bind_email().as_str(),
            &email,
        ) {
            if operation_epoch.get() == request_epoch {
                state.set_email_bind_code_busy(false);
                state.set_email_bind_countdown(0);
                state.set_email_bind_status("邮箱已修改，请重新获取验证码".into());
            }
            return;
        }
        state.set_email_bind_code_busy(false);
        match result {
            Ok(response) => {
                let seconds = response.resend_after_seconds.min(i32::MAX as u64) as i32;
                state.set_email_bind_countdown(seconds);
                state.set_email_bind_status(
                    format!(
                        "验证码已发送至 {}，{} 秒内有效",
                        response.email_masked, response.expires_in_seconds,
                    )
                    .into(),
                );
                start_email_binding_countdown(
                    app.as_weak(),
                    operation_epoch,
                    request_epoch,
                    email,
                );
            }
            Err(error) => state.set_email_bind_status(error.user_message().into()),
        }
    });
}

fn poll_email_binding_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    operation_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    email: String,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Result<EmailBindingResponse, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !account_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &receiver,
        ) {
            return;
        }
        if operation_epoch.get() != request_epoch {
            receiver.borrow_mut().take();
            return;
        }
        let result = poll_email_receiver(&receiver, "邮箱绑定任务意外中断");
        let Some(result) = result else {
            poll_email_binding_result(
                app_weak,
                context,
                operation_epoch,
                request_epoch,
                email,
                session_scope,
                receiver,
            );
            return;
        };
        if !account_poll_is_current(
            &app_weak,
            &context,
            &session_scope,
            &receiver,
        ) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !email_operation_matches(
            operation_epoch.get(),
            request_epoch,
            state.get_email_bind_email().as_str(),
            &email,
        ) {
            if operation_epoch.get() == request_epoch {
                state.set_email_bind_busy(false);
                state.set_email_bind_status("邮箱已修改，请重新获取验证码".into());
            }
            return;
        }
        state.set_email_bind_busy(false);
        match result {
            Ok(response) if response.bound => {
                state.set_email_bound(true);
                state.set_email_mask(response.email_masked.into());
                state.set_email_bind_open(false);
                state.set_email_bind_email("".into());
                state.set_email_bind_code("".into());
                state.set_email_bind_countdown(0);
                state.set_email_bind_status("".into());
                state.set_generation_status("邮箱绑定成功".into());
                save_user_profile(&app);
            }
            Ok(_) => state.set_email_bind_status("邮箱绑定未完成，请重试".into()),
            Err(error) => state.set_email_bind_status(error.user_message().into()),
        }
    });
}

fn account_poll_is_current<T>(
    app_weak: &Weak<AppWindow>,
    context: &AppContext,
    session_scope: &SessionScope,
    receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>,
) -> bool {
    match context.account_scope_disposition(session_scope) {
        AccountScopeDisposition::Current => true,
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
            false
        }
        AccountScopeDisposition::Stale => {
            receiver.borrow_mut().take();
            false
        }
    }
}

fn poll_email_receiver<T>(
    receiver: &Rc<RefCell<Option<mpsc::Receiver<Result<T, ApiError>>>>>,
    disconnected_message: &str,
) -> Option<Result<T, ApiError>> {
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
            Some(Err(ApiError::LocalState {
                message: disconnected_message.to_string(),
            }))
        }
    }
}

fn start_email_binding_countdown(
    app_weak: Weak<AppWindow>,
    operation_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    email: String,
) {
    slint::Timer::single_shot(Duration::from_secs(1), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !state.get_email_bind_open() || operation_epoch.get() != request_epoch {
            return;
        }
        if normalized_binding_email(state.get_email_bind_email().as_str()) != email {
            state.set_email_bind_countdown(0);
            return;
        }
        let remaining = (state.get_email_bind_countdown() - 1).max(0);
        state.set_email_bind_countdown(remaining);
        if remaining > 0 {
            start_email_binding_countdown(
                app.as_weak(),
                operation_epoch,
                request_epoch,
                email,
            );
        }
    });
}

fn advance_operation_epoch(operation_epoch: &Cell<u64>) -> u64 {
    let next = operation_epoch.get().wrapping_add(1);
    operation_epoch.set(next);
    next
}

fn normalized_binding_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn email_operation_matches(
    current_operation_epoch: u64,
    request_epoch: u64,
    current_email: &str,
    request_email: &str,
) -> bool {
    current_operation_epoch == request_epoch
        && normalized_binding_email(current_email) == request_email
}

fn valid_binding_email(email: &str) -> bool {
    let mut parts = email.split('@');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(local), Some(domain), None)
            if !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_response_requires_matching_operation_and_normalized_email_snapshot() {
        assert!(email_operation_matches(
            4,
            4,
            "  USER@Example.com ",
            "user@example.com"
        ));
        assert!(!email_operation_matches(
            5,
            4,
            "user@example.com",
            "user@example.com"
        ));
        assert!(!email_operation_matches(
            4,
            4,
            "other@example.com",
            "user@example.com"
        ));
    }
}
