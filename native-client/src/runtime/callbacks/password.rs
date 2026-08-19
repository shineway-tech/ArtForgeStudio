use super::*;
use std::cell::Cell;

const PASSWORD_RESET_RESEND_SECONDS: i32 = 60;

type PasswordResetCodeResult = std::result::Result<PasswordResetCodeResponse, ApiError>;
type PasswordResetResult = std::result::Result<LoginResponse, ApiError>;
type PasswordChangeCodeResult = std::result::Result<PasswordCodeResponse, ApiError>;
type PasswordMutationResult = std::result::Result<PasswordMutationResponse, ApiError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PasswordInputError {
    TooShort,
    TooLong,
    TooManyBytes,
    AllWhitespace,
    ConfirmationMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PasswordLoginInputError {
    Empty,
    TooManyBytes,
}

pub(super) fn validate_login_password(password: &str) -> Result<(), PasswordLoginInputError> {
    if password.chars().all(char::is_whitespace) {
        return Err(PasswordLoginInputError::Empty);
    }
    if password.len() > 512 {
        return Err(PasswordLoginInputError::TooManyBytes);
    }
    Ok(())
}

fn validate_new_password(password: &str, confirmation: &str) -> Result<(), PasswordInputError> {
    if password.len() > 512 {
        return Err(PasswordInputError::TooManyBytes);
    }
    let characters = password.chars().count();
    if characters < 12 {
        return Err(PasswordInputError::TooShort);
    }
    if characters > 128 {
        return Err(PasswordInputError::TooLong);
    }
    if password.chars().all(char::is_whitespace) {
        return Err(PasswordInputError::AllWhitespace);
    }
    if password != confirmation {
        return Err(PasswordInputError::ConfirmationMismatch);
    }
    Ok(())
}

fn valid_password_code(code: &str) -> bool {
    code.len() == 6 && code.chars().all(|value| value.is_ascii_digit())
}

fn normalized_password_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn reset_operation_matches(
    dialog_open: bool,
    current_epoch: u64,
    request_epoch: u64,
    current_email: &str,
    request_email: &str,
    current_code: &str,
    request_code: &str,
) -> bool {
    dialog_open
        && current_epoch == request_epoch
        && normalized_password_email(current_email) == request_email
        && current_code.trim() == request_code
}

fn reset_email_operation_matches(
    dialog_open: bool,
    current_epoch: u64,
    request_epoch: u64,
    current_email: &str,
    request_email: &str,
) -> bool {
    dialog_open
        && current_epoch == request_epoch
        && normalized_password_email(current_email) == request_email
}

fn reset_submission_is_current(
    dialog_open: bool,
    current_epoch: u64,
    request_epoch: u64,
    auth_operation_current: bool,
    current_email: &str,
    request_email: &str,
    current_code: &str,
    request_code: &str,
) -> bool {
    auth_operation_current
        && reset_operation_matches(
            dialog_open,
            current_epoch,
            request_epoch,
            current_email,
            request_email,
            current_code,
            request_code,
        )
}

fn management_operation_matches(
    dialog_open: bool,
    current_epoch: u64,
    request_epoch: u64,
    current_mode: &str,
    request_mode: &str,
) -> bool {
    dialog_open && current_epoch == request_epoch && current_mode.trim() == request_mode
}

fn management_submission_is_current(
    dialog_open: bool,
    current_epoch: u64,
    request_epoch: u64,
    session_scope_current: bool,
    current_mode: &str,
    request_mode: &str,
) -> bool {
    session_scope_current
        && management_operation_matches(
            dialog_open,
            current_epoch,
            request_epoch,
            current_mode,
            request_mode,
        )
}

fn advance_password_epoch(operation_epoch: &Cell<u64>) -> u64 {
    let next = operation_epoch.get().wrapping_add(1);
    operation_epoch.set(next);
    next
}

fn valid_password_email(email: &str) -> bool {
    let mut parts = email.split('@');
    matches!((parts.next(), parts.next(), parts.next()), (Some(local), Some(domain), None) if !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.'))
}

fn password_input_error_message(error: PasswordInputError) -> &'static str {
    match error {
        PasswordInputError::TooShort => "新密码至少需要 12 个字符",
        PasswordInputError::TooLong => "新密码不能超过 128 个字符",
        PasswordInputError::TooManyBytes => "新密码不能超过 512 字节",
        PasswordInputError::AllWhitespace => "新密码不能全为空白",
        PasswordInputError::ConfirmationMismatch => "两次输入的新密码不一致",
    }
}

pub(super) fn clear_password_reset_state(state: &AppState) {
    state.set_password_reset_open(false);
    state.set_password_reset_email("".into());
    state.set_password_reset_code("".into());
    state.set_password_reset_new_password("".into());
    state.set_password_reset_confirm_password("".into());
    state.set_password_reset_code_busy(false);
    state.set_password_reset_busy(false);
    state.set_password_reset_countdown(0);
    state.set_password_reset_status("".into());
}

pub(super) fn clear_password_management_state(state: &AppState) {
    state.set_password_manage_open(false);
    state.set_password_verification_mode("email_code".into());
    state.set_password_current_password("".into());
    state.set_password_new_password("".into());
    state.set_password_confirm_password("".into());
    state.set_password_change_code("".into());
    state.set_password_change_code_busy(false);
    state.set_password_change_busy(false);
    state.set_password_change_countdown(0);
    state.set_password_change_status("".into());
}

pub(super) fn wire_password_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();
    let reset_epoch = Rc::new(Cell::new(0_u64));
    let management_epoch = Rc::new(Cell::new(0_u64));

    {
        let app_weak = app.as_weak();
        let reset_epoch = reset_epoch.clone();
        state.on_open_password_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            advance_password_epoch(&reset_epoch);
            let state = app.global::<AppState>();
            let email = normalized_password_email(state.get_auth_email().as_str());
            clear_password_reset_state(&state);
            state.set_auth_password("".into());
            state.set_password_reset_email(email.into());
            state.set_password_reset_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let reset_epoch = reset_epoch.clone();
        state.on_request_password_reset_code(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_password_reset_open()
                || state.get_password_reset_code_busy()
                || state.get_password_reset_busy()
                || state.get_password_reset_countdown() > 0
            {
                return;
            }
            let email = normalized_password_email(state.get_password_reset_email().as_str());
            if !valid_password_email(&email) {
                state.set_password_reset_status("请输入正确的邮箱地址".into());
                return;
            }
            let request_epoch = advance_password_epoch(&reset_epoch);
            state.set_password_reset_code_busy(true);
            state.set_password_reset_status("正在发送验证码...".into());
            let api = AuthApi::new(backend.api.clone());
            let worker_email = email.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(api.request_password_reset_code(&worker_email));
            });
            poll_password_reset_code_result(
                app.as_weak(),
                reset_epoch.clone(),
                request_epoch,
                email,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        let reset_epoch = reset_epoch.clone();
        state.on_reset_password(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_password_reset_open()
                || state.get_password_reset_busy()
                || state.get_password_reset_code_busy()
                || state.get_auth_busy()
            {
                return;
            }
            let email = normalized_password_email(state.get_password_reset_email().as_str());
            let code = state.get_password_reset_code().trim().to_string();
            let new_password = state.get_password_reset_new_password().to_string();
            let confirmation = state.get_password_reset_confirm_password().to_string();
            if !valid_password_email(&email) {
                state.set_password_reset_status("请输入正确的邮箱地址".into());
                return;
            }
            if !valid_password_code(&code) {
                state.set_password_reset_status("请输入 6 位数字验证码".into());
                return;
            }
            if let Err(error) = validate_new_password(&new_password, &confirmation) {
                state.set_password_reset_status(password_input_error_message(error).into());
                return;
            }
            if state.get_auth_user_terms_required() && !state.get_auth_user_terms_accepted() {
                state.set_password_reset_status("请先阅读并同意用户协议".into());
                return;
            }
            if state.get_auth_privacy_required() && !state.get_auth_privacy_accepted() {
                state.set_password_reset_status("请先阅读并同意隐私政策".into());
                return;
            }
            let acceptances = selected_login_agreement_acceptances(&state);
            let request_epoch = advance_password_epoch(&reset_epoch);
            let auth_epoch = begin_auth_operation(&context);
            state.set_password_reset_busy(true);
            state.set_session_state("authenticating".into());
            state.set_password_reset_status("正在重置密码...".into());
            let api = AuthApi::new(backend.api.clone());
            let worker_email = email.clone();
            let worker_code = code.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(api.reset_password_response(
                    &worker_email,
                    &worker_code,
                    &new_password,
                    &acceptances,
                ));
            });
            poll_password_reset_result(
                app.as_weak(),
                context.clone(),
                reset_epoch.clone(),
                request_epoch,
                auth_epoch,
                email,
                code,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let reset_epoch = reset_epoch.clone();
        state.on_close_password_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            advance_password_epoch(&reset_epoch);
            clear_password_reset_state(&app.global::<AppState>());
        });
    }

    {
        let app_weak = app.as_weak();
        let management_epoch = management_epoch.clone();
        state.on_open_password_management(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_email_bound() {
                state.set_generation_status("请先绑定并验证邮箱".into());
                return;
            }
            advance_password_epoch(&management_epoch);
            let verification_mode = if state.get_password_set() {
                "current_password"
            } else {
                "email_code"
            };
            clear_password_management_state(&state);
            state.set_password_verification_mode(verification_mode.into());
            state.set_password_manage_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        let management_epoch = management_epoch.clone();
        state.on_request_password_change_code(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_password_manage_open()
                || !state.get_email_bound()
                || state.get_password_verification_mode().as_str() != "email_code"
                || state.get_password_change_code_busy()
                || state.get_password_change_busy()
                || state.get_password_change_countdown() > 0
            {
                return;
            }
            let Some(session_scope) = context.current_account_session_scope() else {
                state.set_password_change_status("登录状态已失效，请重新登录".into());
                return;
            };
            let request_epoch = advance_password_epoch(&management_epoch);
            state.set_password_change_code_busy(true);
            state.set_password_change_status("正在发送验证码...".into());
            let api = AccountApi::new(backend.api.clone());
            let worker_scope = session_scope.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(api.request_password_code_scoped(&worker_scope));
            });
            poll_password_change_code_result(
                app.as_weak(),
                context.clone(),
                management_epoch.clone(),
                request_epoch,
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        let management_epoch = management_epoch.clone();
        state.on_save_password(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_password_manage_open()
                || !state.get_email_bound()
                || state.get_password_change_busy()
                || state.get_password_change_code_busy()
            {
                return;
            }
            let password_was_set = state.get_password_set();
            let verification_mode = if password_was_set {
                state.get_password_verification_mode().to_string()
            } else {
                state.set_password_verification_mode("email_code".into());
                "email_code".to_string()
            };
            if verification_mode != "current_password" && verification_mode != "email_code" {
                state.set_password_change_status("请选择一种密码验证方式".into());
                return;
            }
            let current_password = (verification_mode == "current_password")
                .then(|| state.get_password_current_password().to_string());
            let email_code = (verification_mode == "email_code")
                .then(|| state.get_password_change_code().trim().to_string());
            if current_password
                .as_ref()
                .is_some_and(|value| value.is_empty())
            {
                state.set_password_change_status("请输入当前密码".into());
                return;
            }
            if email_code
                .as_ref()
                .is_some_and(|value| !valid_password_code(value))
            {
                state.set_password_change_status("请输入 6 位数字验证码".into());
                return;
            }
            let new_password = state.get_password_new_password().to_string();
            let confirmation = state.get_password_confirm_password().to_string();
            if let Err(error) = validate_new_password(&new_password, &confirmation) {
                state.set_password_change_status(password_input_error_message(error).into());
                return;
            }
            let Some(session_scope) = context.current_account_session_scope() else {
                state.set_password_change_status("登录状态已失效，请重新登录".into());
                return;
            };
            let request_epoch = advance_password_epoch(&management_epoch);
            state.set_password_change_busy(true);
            state.set_password_change_status(if password_was_set {
                "正在修改密码...".into()
            } else {
                "正在设置密码...".into()
            });
            let api = AccountApi::new(backend.api.clone());
            let worker_scope = session_scope.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(api.set_password_scoped(
                    &new_password,
                    current_password.as_deref(),
                    email_code.as_deref(),
                    &worker_scope,
                ));
            });
            poll_password_mutation_result(
                app.as_weak(),
                context.clone(),
                management_epoch.clone(),
                request_epoch,
                verification_mode,
                password_was_set,
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let management_epoch = management_epoch.clone();
        state.on_close_password_management(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            advance_password_epoch(&management_epoch);
            clear_password_management_state(&app.global::<AppState>());
        });
    }
}

fn poll_password_reset_code_result(
    app_weak: Weak<AppWindow>,
    reset_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    email: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<PasswordResetCodeResult>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = match poll_password_receiver(&receiver, "密码重置验证码任务意外中断")
        {
            Some(result) => result,
            None => {
                poll_password_reset_code_result(
                    app_weak,
                    reset_epoch,
                    request_epoch,
                    email,
                    receiver,
                );
                return;
            }
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !reset_email_operation_matches(
            state.get_password_reset_open(),
            reset_epoch.get(),
            request_epoch,
            state.get_password_reset_email().as_str(),
            &email,
        ) {
            if reset_epoch.get() == request_epoch && state.get_password_reset_open() {
                state.set_password_reset_code_busy(false);
                state.set_password_reset_countdown(0);
                state.set_password_reset_status("邮箱已修改，请重新获取验证码".into());
            }
            return;
        }
        state.set_password_reset_code_busy(false);
        match result {
            Ok(response) => {
                let _ = response.accepted;
                state.set_password_reset_countdown(PASSWORD_RESET_RESEND_SECONDS);
                state.set_password_reset_status(response.message.into());
                start_password_reset_countdown(app.as_weak(), reset_epoch, request_epoch, email);
            }
            Err(error) => state.set_password_reset_status(error.user_message().into()),
        }
    });
}

fn poll_password_change_code_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    management_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<PasswordChangeCodeResult>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !password_account_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let result = match poll_password_receiver(&receiver, "密码验证码任务意外中断") {
            Some(result) => result,
            None => {
                poll_password_change_code_result(
                    app_weak,
                    context,
                    management_epoch,
                    request_epoch,
                    session_scope,
                    receiver,
                );
                return;
            }
        };
        if !password_account_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !management_submission_is_current(
            state.get_password_manage_open(),
            management_epoch.get(),
            request_epoch,
            true,
            state.get_password_verification_mode().as_str(),
            "email_code",
        ) {
            return;
        }
        state.set_password_change_code_busy(false);
        match result {
            Ok(response) => {
                let seconds = response.resend_after_seconds.min(i32::MAX as u64) as i32;
                state.set_password_change_countdown(seconds);
                state.set_password_change_status(
                    format!(
                        "验证码已发送至 {}，{} 秒内有效",
                        response.email_masked, response.expires_in_seconds,
                    )
                    .into(),
                );
                start_password_change_countdown(
                    app.as_weak(),
                    context,
                    management_epoch,
                    request_epoch,
                    session_scope,
                );
            }
            Err(error) => state.set_password_change_status(error.user_message().into()),
        }
    });
}

fn poll_password_mutation_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    management_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    verification_mode: String,
    password_was_set: bool,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<PasswordMutationResult>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !password_account_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let result = match poll_password_receiver(&receiver, "密码保存任务意外中断") {
            Some(result) => result,
            None => {
                poll_password_mutation_result(
                    app_weak,
                    context,
                    management_epoch,
                    request_epoch,
                    verification_mode,
                    password_was_set,
                    session_scope,
                    receiver,
                );
                return;
            }
        };
        if !password_account_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !management_submission_is_current(
            state.get_password_manage_open(),
            management_epoch.get(),
            request_epoch,
            true,
            state.get_password_verification_mode().as_str(),
            &verification_mode,
        ) {
            return;
        }
        state.set_password_change_busy(false);
        match result {
            Ok(response) => {
                let _ = (
                    response.set,
                    response.changed_at,
                    response.other_sessions_revoked,
                );
                clear_password_management_state(&state);
                state.set_generation_status(if password_was_set {
                    "密码修改成功，账号状态正在同步".into()
                } else {
                    "密码设置成功，账号状态正在同步".into()
                });
                refresh_backend_snapshot(&app, context);
            }
            Err(error) => state.set_password_change_status(error.user_message().into()),
        }
    });
}

fn password_account_poll_is_current<T>(
    app_weak: &Weak<AppWindow>,
    context: &AppContext,
    session_scope: &SessionScope,
    receiver: &Rc<RefCell<Option<mpsc::Receiver<std::result::Result<T, ApiError>>>>>,
) -> bool {
    match context.account_scope_disposition(session_scope) {
        AccountScopeDisposition::Current => true,
        AccountScopeDisposition::CapturedTerminal => {
            receiver.borrow_mut().take();
            if let Some(app) = app_weak.upgrade() {
                sign_out_locally(&app, context, true, Some(session_scope.auth_epoch));
            }
            false
        }
        AccountScopeDisposition::Stale => {
            receiver.borrow_mut().take();
            false
        }
    }
}

fn start_password_change_countdown(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    management_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    session_scope: SessionScope,
) {
    slint::Timer::single_shot(Duration::from_secs(1), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match context.account_scope_disposition(&session_scope) {
            AccountScopeDisposition::Current => {}
            AccountScopeDisposition::CapturedTerminal => {
                sign_out_locally(&app, &context, true, Some(session_scope.auth_epoch));
                return;
            }
            AccountScopeDisposition::Stale => return,
        }
        let state = app.global::<AppState>();
        if !management_submission_is_current(
            state.get_password_manage_open(),
            management_epoch.get(),
            request_epoch,
            true,
            state.get_password_verification_mode().as_str(),
            "email_code",
        ) {
            return;
        }
        let remaining = (state.get_password_change_countdown() - 1).max(0);
        state.set_password_change_countdown(remaining);
        if remaining > 0 {
            start_password_change_countdown(
                app.as_weak(),
                context,
                management_epoch,
                request_epoch,
                session_scope,
            );
        }
    });
}

fn poll_password_reset_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    reset_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    auth_epoch: u64,
    email: String,
    code: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<PasswordResetResult>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = match poll_password_receiver(&receiver, "密码重置任务意外中断") {
            Some(result) => result,
            None => {
                poll_password_reset_result(
                    app_weak,
                    context,
                    reset_epoch,
                    request_epoch,
                    auth_epoch,
                    email,
                    code,
                    receiver,
                );
                return;
            }
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let current = reset_submission_is_current(
            state.get_password_reset_open(),
            reset_epoch.get(),
            request_epoch,
            auth_operation_is_current(&context, auth_epoch),
            state.get_password_reset_email().as_str(),
            &email,
            state.get_password_reset_code().as_str(),
            &code,
        );
        if !current {
            return;
        }
        state.set_password_reset_busy(false);
        match result {
            Ok(response) => {
                let Some(backend) = context.backend.as_ref() else {
                    state.set_session_state("signed_out".into());
                    state.set_password_reset_status("客户端服务尚未就绪，请重试".into());
                    return;
                };
                match backend
                    .api
                    .session()
                    .install_tokens_for_user(&response.tokens, &response.user.id)
                {
                    Ok(_) => {
                        clear_password_reset_state(&state);
                        state.set_auth_password("".into());
                        finish_login(&app, &context, response, None);
                        refresh_backend_snapshot(&app, context);
                    }
                    Err(error) => {
                        state.set_session_state("signed_out".into());
                        state.set_password_reset_status(error.user_message().into());
                    }
                }
            }
            Err(error) => {
                state.set_session_state("signed_out".into());
                state.set_password_reset_status(error.user_message().into());
            }
        }
    });
}

fn poll_password_receiver<T>(
    receiver: &Rc<RefCell<Option<mpsc::Receiver<std::result::Result<T, ApiError>>>>>,
    disconnected_message: &str,
) -> Option<std::result::Result<T, ApiError>> {
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

fn start_password_reset_countdown(
    app_weak: Weak<AppWindow>,
    reset_epoch: Rc<Cell<u64>>,
    request_epoch: u64,
    email: String,
) {
    slint::Timer::single_shot(Duration::from_secs(1), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !reset_email_operation_matches(
            state.get_password_reset_open(),
            reset_epoch.get(),
            request_epoch,
            state.get_password_reset_email().as_str(),
            &email,
        ) {
            state.set_password_reset_countdown(0);
            return;
        }
        let remaining = (state.get_password_reset_countdown() - 1).max(0);
        state.set_password_reset_countdown(remaining);
        if remaining > 0 {
            start_password_reset_countdown(app.as_weak(), reset_epoch, request_epoch, email);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_password_validation_preserves_unicode_and_enforces_every_structural_bound() {
        assert!(validate_new_password("  正确 horse battery  ", "  正确 horse battery  ").is_ok());
        assert_eq!(
            validate_new_password("short", "short"),
            Err(PasswordInputError::TooShort)
        );
        assert_eq!(
            validate_new_password("            ", "            "),
            Err(PasswordInputError::AllWhitespace)
        );
        assert_eq!(
            validate_new_password(&"界".repeat(129), &"界".repeat(129)),
            Err(PasswordInputError::TooLong)
        );
        assert_eq!(
            validate_new_password(&"🦀".repeat(129), &"🦀".repeat(129)),
            Err(PasswordInputError::TooManyBytes)
        );
        assert_eq!(
            validate_new_password("a sufficiently long passphrase", "different confirmation"),
            Err(PasswordInputError::ConfirmationMismatch)
        );
    }

    #[test]
    fn password_code_requires_exactly_six_ascii_digits() {
        assert!(valid_password_code("123456"));
        assert!(!valid_password_code("12345"));
        assert!(!valid_password_code("１２３４５６"));
        assert!(!valid_password_code("12345a"));
    }

    #[test]
    fn password_operation_snapshots_reject_closed_or_changed_flows() {
        assert!(reset_operation_matches(
            true,
            4,
            4,
            " Artist@Example.com ",
            "artist@example.com",
            " 123456 ",
            "123456",
        ));
        assert!(!reset_operation_matches(
            false,
            4,
            4,
            "artist@example.com",
            "artist@example.com",
            "123456",
            "123456",
        ));
        assert!(!reset_operation_matches(
            true,
            5,
            4,
            "artist@example.com",
            "artist@example.com",
            "123456",
            "123456",
        ));
        assert!(!management_operation_matches(
            true,
            9,
            9,
            "email_code",
            "current_password",
        ));
    }

    #[test]
    fn password_reset_installs_a_session_only_for_the_current_auth_operation() {
        assert!(reset_submission_is_current(
            true,
            12,
            12,
            true,
            " Artist@Example.com ",
            "artist@example.com",
            " 123456 ",
            "123456",
        ));
        assert!(!reset_submission_is_current(
            true,
            12,
            12,
            false,
            "artist@example.com",
            "artist@example.com",
            "123456",
            "123456",
        ));
        assert!(!reset_submission_is_current(
            true,
            13,
            12,
            true,
            "artist@example.com",
            "artist@example.com",
            "654321",
            "123456",
        ));
    }

    #[test]
    fn password_management_rejects_stale_dialog_epoch_scope_and_mode() {
        assert!(management_submission_is_current(
            true,
            8,
            8,
            true,
            "email_code",
            "email_code",
        ));
        for current in [
            management_submission_is_current(false, 8, 8, true, "email_code", "email_code"),
            management_submission_is_current(true, 9, 8, true, "email_code", "email_code"),
            management_submission_is_current(true, 8, 8, false, "email_code", "email_code"),
            management_submission_is_current(true, 8, 8, true, "current_password", "email_code"),
        ] {
            assert!(!current);
        }
    }

    #[test]
    fn password_login_uses_transport_bounds_without_new_password_policy() {
        assert!(validate_login_password("short").is_ok());
        assert!(validate_login_password(&"🦀".repeat(128)).is_ok());
        assert_eq!(
            validate_login_password("        "),
            Err(PasswordLoginInputError::Empty)
        );
        assert_eq!(
            validate_login_password(&"🦀".repeat(129)),
            Err(PasswordLoginInputError::TooManyBytes)
        );
    }
}
