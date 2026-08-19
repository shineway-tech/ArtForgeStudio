use super::*;

struct StartupAuthResult {
    auth_epoch: u64,
    credit_sync_epoch: u64,
    agreements: std::result::Result<Vec<AgreementItem>, ApiError>,
    refresh: Option<std::result::Result<String, ApiError>>,
    snapshot: Option<std::result::Result<BackendSnapshot, ApiError>>,
}

type LoginResult = std::result::Result<LoginResponse, ApiError>;

enum WechatPollOutcome {
    Pending,
    Scanned(String),
    AgreementRequired(String),
    Failed(String),
    Completed(LoginResponse),
}

pub(super) fn begin_auth_operation(context: &AppContext) -> u64 {
    context
        .auth_operation_epoch
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_add(1)
}

pub(super) fn auth_operation_is_current(context: &AppContext, operation_epoch: u64) -> bool {
    context.auth_operation_epoch.load(Ordering::SeqCst) == operation_epoch
}

fn invalidate_auth_operations(context: &AppContext) {
    context.auth_operation_epoch.fetch_add(1, Ordering::SeqCst);
}

fn install_login_if_current(
    current: bool,
    install: impl FnOnce() -> std::result::Result<SessionScope, ApiError>,
) -> std::result::Result<Option<SessionScope>, ApiError> {
    if !current {
        return Ok(None);
    }
    install().map(Some)
}

fn expire_wechat_login(state: &AppState) {
    state.set_auth_wechat_login_id("".into());
    state.set_auth_wechat_qr_ready(false);
    state.set_auth_wechat_scanned(false);
    state.set_auth_wechat_poll_elapsed_ms(0);
    state.set_auth_wechat_status("微信二维码已失效，请点击刷新".into());
    state.set_auth_error("微信二维码已失效，请点击刷新".into());
}

pub(super) fn wire_auth_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        state.on_request_code(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_auth_busy() || state.get_auth_code_busy() || state.get_auth_countdown() > 0
                || state.get_auth_email_mode().as_str() != "code"
            {
                return;
            }
            let email = state.get_auth_email().trim().to_ascii_lowercase();
            if !valid_email(&email) {
                state.set_auth_error("请输入正确的邮箱地址".into());
                return;
            }
            state.set_auth_code_busy(true);
            state.set_auth_error("".into());
            let api = AuthApi::new(backend.api.clone());
            let weak = app.as_weak();
            std::thread::spawn(move || {
                let result = api.request_email_code(&email);
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<AppState>();
                    state.set_auth_code_busy(false);
                    match result {
                        Ok(response) => {
                            let seconds = response.resend_after_seconds.min(i32::MAX as u64) as i32;
                            state.set_auth_countdown(seconds);
                            state.set_auth_error(
                                format!(
                                    "验证码已发送至 {}，{} 秒内有效",
                                    response.email_masked, response.expires_in_seconds
                                )
                                .into(),
                            );
                            start_countdown(app.as_weak());
                        }
                        Err(error) => apply_auth_error(&app, error),
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        state.on_start_wechat_login(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            begin_wechat_login(&app, context.clone(), backend.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        state.on_revoke_session(move |session_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let session_id = session_id.to_string();
            if session_id.trim().is_empty() {
                return;
            }
            let Some(session_scope) = current_auth_session_scope(&context) else {
                app.global::<AppState>()
                    .set_generation_status("登录状态已变化，请重新登录后操作".into());
                return;
            };
            let state = app.global::<AppState>();
            let previous_sessions = state.get_account_sessions().iter().collect::<Vec<_>>();
            let revoked_current = previous_sessions
                .iter()
                .any(|session| session.id.as_str() == session_id && session.is_current);
            state.set_account_sessions(ModelRc::new(VecModel::from(
                previous_sessions
                    .iter()
                    .cloned()
                    .filter(|session| session.id.as_str() != session_id)
                    .collect::<Vec<_>>(),
            )));
            let api = AccountApi::new(backend.api.clone());
            let (sender, receiver) = mpsc::channel();
            let worker_scope = session_scope.clone();
            std::thread::spawn(move || {
                let _ = sender.send(api.revoke_session_scoped(&session_id, &worker_scope));
            });
            poll_revoke_session_result(
                app.as_weak(),
                context.clone(),
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
                previous_sessions,
                revoked_current,
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        state.on_login_or_register(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_auth_busy() || state.get_auth_code_busy() {
                return;
            }
            let email = state.get_auth_email().trim().to_ascii_lowercase();
            let login_mode = state.get_auth_email_mode().to_string();
            let credential = if login_mode == "password" {
                state.get_auth_password().to_string()
            } else {
                state.get_auth_code().trim().to_string()
            };
            if !valid_email(&email) {
                state.set_auth_error("请输入正确的邮箱地址".into());
                return;
            }
            if login_mode == "password" {
                if credential.trim().is_empty() {
                    state.set_auth_error("请输入密码".into());
                    return;
                }
                if credential.chars().count() > 256 {
                    state.set_auth_error("密码长度不能超过 256 个字符".into());
                    return;
                }
            } else if credential.len() != 6
                || !credential.chars().all(|value| value.is_ascii_digit())
            {
                state.set_auth_error("请输入 6 位数字验证码".into());
                return;
            }
            if state.get_auth_user_terms_required() && !state.get_auth_user_terms_accepted() {
                state.set_auth_error("请先阅读并同意用户协议".into());
                return;
            }
            if state.get_auth_privacy_required() && !state.get_auth_privacy_accepted() {
                state.set_auth_error("请先阅读并同意隐私政策".into());
                return;
            }
            let mut acceptances = Vec::new();
            if state.get_auth_user_terms_accepted() {
                acceptances.push(AgreementAcceptance {
                    agreement_type: "user_terms".to_string(),
                    version: state.get_auth_user_terms_version().to_string(),
                });
            }
            if state.get_auth_privacy_accepted() {
                acceptances.push(AgreementAcceptance {
                    agreement_type: "privacy_policy".to_string(),
                    version: state.get_auth_privacy_version().to_string(),
                });
            }
            let auth_operation_epoch = begin_auth_operation(&context);
            state.set_auth_busy(true);
            state.set_session_state("authenticating".into());
            state.set_auth_error("".into());
            let api = AuthApi::new(backend.api.clone());
            let context = context.clone();
            let (sender, receiver) = mpsc::channel();
            let worker_login_mode = login_mode.clone();
            std::thread::spawn(move || {
                let result = if worker_login_mode == "password" {
                    api.password_login_response(&email, &credential, &acceptances)
                } else {
                    api.login_response(&email, &credential, &acceptances)
                };
                let _ = sender.send(result);
            });
            poll_login_result(
                app.as_weak(),
                context,
                auth_operation_epoch,
                login_mode,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_enter_offline(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_offline_available() {
                return;
            }
            invalidate_auth_operations(&context);
            state.set_auth_busy(false);
            state.set_auth_wechat_busy(false);
            state.set_logged_in(true);
            state.set_offline_mode(true);
            state.set_session_state("offline".into());
            state.set_auth_open(false);
            state.set_auth_password("".into());
            state.set_auth_error("".into());
            navigate_to_with_store(&app, &context.store.borrow(), "assets");
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_open_agreement(move |title, url| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let title = title.trim().to_string();
            let url = url.trim().to_string();
            close_agreement_window();
            state.set_agreement_viewer_title(if title.is_empty() {
                "协议".into()
            } else {
                title.into()
            });
            state.set_agreement_viewer_url(url.clone().into());
            state.set_agreement_viewer_message("".into());
            state.set_agreement_viewer_open(true);
            if open_agreement_window(&app, &url).is_err() {
                state.set_agreement_viewer_message("协议内容加载失败，请稍后重试".into());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_agreement(move || {
            close_agreement_window();
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_agreement_viewer_open(false);
                state.set_agreement_viewer_message("".into());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        state.on_accept_current_agreements(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_agreement_update_busy() {
                return;
            }
            let Some(session_scope) = current_auth_session_scope(&context) else {
                state.set_agreement_update_message("登录状态已变化，请重新登录后操作".into());
                return;
            };
            if state.get_auth_user_terms_required() && !state.get_auth_user_terms_accepted() {
                state.set_agreement_update_message("请同意用户协议".into());
                return;
            }
            if state.get_auth_privacy_required() && !state.get_auth_privacy_accepted() {
                state.set_agreement_update_message("请同意隐私政策".into());
                return;
            }
            let mut acceptances = Vec::new();
            if state.get_auth_user_terms_required() {
                acceptances.push(AgreementAcceptance {
                    agreement_type: "user_terms".to_string(),
                    version: state.get_auth_user_terms_version().to_string(),
                });
            }
            if state.get_auth_privacy_required() {
                acceptances.push(AgreementAcceptance {
                    agreement_type: "privacy_policy".to_string(),
                    version: state.get_auth_privacy_version().to_string(),
                });
            }
            let accepted_user_terms_version = state.get_auth_user_terms_version().to_string();
            let accepted_privacy_version = state.get_auth_privacy_version().to_string();
            state.set_agreement_update_busy(true);
            state.set_agreement_update_message("".into());
            let api = AuthApi::new(backend.api.clone());
            let (sender, receiver) = mpsc::channel();
            let worker_scope = session_scope.clone();
            std::thread::spawn(move || {
                let _ = sender.send(api.accept_agreements_scoped(&acceptances, &worker_scope));
            });
            poll_agreement_acceptance_result(
                app.as_weak(),
                context.clone(),
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
                accepted_user_terms_version,
                accepted_privacy_version,
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        state.on_logout(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let logout_scope = current_auth_session_scope(&context);
            let logout_token = logout_scope.as_ref().and_then(|scope| {
                backend
                    .api
                    .session()
                    .access()
                    .filter(|access| access.auth_epoch == scope.auth_epoch)
                    .map(|access| access.access_token)
            });
            if let Some(scope) = logout_scope.as_ref() {
                let _ = backend.api.session().clear_scope(scope);
            }
            sign_out_locally(&app, &context, false, None);
            if let Some(logout_token) = logout_token {
                let api = AuthApi::new(backend.api.clone());
                std::thread::spawn(move || {
                    let _ = api.logout_with_fixed_token(false, &logout_token);
                });
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        let context = context.clone();
        state.on_logout_all(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let logout_scope = current_auth_session_scope(&context);
            let logout_token = logout_scope.as_ref().and_then(|scope| {
                backend
                    .api
                    .session()
                    .access()
                    .filter(|access| access.auth_epoch == scope.auth_epoch)
                    .map(|access| access.access_token)
            });
            if let Some(scope) = logout_scope.as_ref() {
                let _ = backend.api.session().clear_scope(scope);
            }
            sign_out_locally(&app, &context, false, None);
            if let Some(logout_token) = logout_token {
                let api = AuthApi::new(backend.api.clone());
                std::thread::spawn(move || {
                    let _ = api.logout_with_fixed_token(true, &logout_token);
                });
            }
        });
    }
}

fn current_auth_session_scope(context: &AppContext) -> Option<SessionScope> {
    let owner_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone()
        .filter(|value| !value.trim().is_empty())?;
    let session = context.backend.as_ref()?.api.session();
    let scope = SessionScope {
        owner_user_id,
        auth_epoch: session.auth_epoch(),
    };
    session.is_scope_current(&scope).then_some(scope)
}

fn auth_scope_matches_context(context: &AppContext, scope: &SessionScope) -> bool {
    let current_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone();
    current_user_id.as_deref() == Some(scope.owner_user_id.as_str())
        && context
            .backend
            .as_ref()
            .is_some_and(|backend| backend.api.session().is_scope_current(scope))
}

pub(super) fn terminal_auth_scope_matches_context(
    context: &AppContext,
    scope: &SessionScope,
) -> bool {
    let current_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone();
    let Some(backend) = context.backend.as_ref() else {
        return false;
    };
    let session = backend.api.session();
    let current_epoch = session.auth_epoch();
    current_user_id.as_deref() == Some(scope.owner_user_id.as_str())
        && session.access().is_none()
        && (current_epoch == scope.auth_epoch || current_epoch == scope.auth_epoch.wrapping_add(1))
}

fn terminal_auth_epoch_matches_context(context: &AppContext, auth_epoch: u64) -> bool {
    let Some(backend) = context.backend.as_ref() else {
        return false;
    };
    let session = backend.api.session();
    let current_epoch = session.auth_epoch();
    session.access().is_none()
        && (current_epoch == auth_epoch || current_epoch == auth_epoch.wrapping_add(1))
}

fn captured_session_error(error: &ApiError) -> bool {
    matches!(error, ApiError::AuthenticationRequired) || error.is_terminal_session_error()
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScopedAuthOutcome {
    Current,
    CapturedTerminal,
    Stale,
}

#[cfg(test)]
fn classify_scoped_auth_guards(
    captured_session_error: bool,
    exact_scope_matches: bool,
    terminal_scope_matches: bool,
) -> ScopedAuthOutcome {
    if captured_session_error && terminal_scope_matches {
        ScopedAuthOutcome::CapturedTerminal
    } else if exact_scope_matches {
        ScopedAuthOutcome::Current
    } else {
        ScopedAuthOutcome::Stale
    }
}

fn scoped_auth_poll_is_current<T>(
    app_weak: &Weak<AppWindow>,
    context: &AppContext,
    scope: &SessionScope,
    receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>,
) -> bool {
    if auth_scope_matches_context(context, scope) {
        return true;
    }
    receiver.borrow_mut().take();
    if terminal_auth_scope_matches_context(context, scope) {
        if let Some(app) = app_weak.upgrade() {
            sign_out_locally(&app, context, true, Some(scope.auth_epoch));
        }
    }
    false
}

fn poll_scoped_auth_receiver<T>(
    receiver: &Rc<RefCell<Option<mpsc::Receiver<std::result::Result<T, ApiError>>>>>,
    disconnected_message: &str,
) -> Option<std::result::Result<T, ApiError>> {
    let mut slot = receiver.borrow_mut();
    let rx = slot.as_ref()?;
    match rx.try_recv() {
        Ok(result) => {
            slot.take();
            Some(result)
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

fn poll_revoke_session_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<(), ApiError>>>>>,
    previous_sessions: Vec<AccountSession>,
    revoked_current: bool,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !scoped_auth_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let result = poll_scoped_auth_receiver(&receiver, "设备会话撤销请求已中断");
        let Some(result) = result else {
            poll_revoke_session_result(
                app_weak,
                context,
                session_scope,
                receiver,
                previous_sessions,
                revoked_current,
            );
            return;
        };
        if !scoped_auth_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match result {
            Ok(()) if revoked_current => {
                sign_out_locally(&app, &context, true, Some(session_scope.auth_epoch));
            }
            Ok(()) => app
                .global::<AppState>()
                .set_generation_status("设备会话已撤销".into()),
            Err(error) => {
                let state = app.global::<AppState>();
                state.set_account_sessions(ModelRc::new(VecModel::from(previous_sessions)));
                state.set_generation_status(
                    format!("撤销设备失败：{}", error.user_message()).into(),
                );
            }
        }
    });
}

fn poll_agreement_acceptance_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<(), ApiError>>>>>,
    accepted_user_terms_version: String,
    accepted_privacy_version: String,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !scoped_auth_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let result = poll_scoped_auth_receiver(&receiver, "协议确认请求已中断");
        let Some(result) = result else {
            poll_agreement_acceptance_result(
                app_weak,
                context,
                session_scope,
                receiver,
                accepted_user_terms_version,
                accepted_privacy_version,
            );
            return;
        };
        if !scoped_auth_poll_is_current(&app_weak, &context, &session_scope, &receiver) {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_agreement_update_busy(false);
        match result {
            Ok(()) => {
                state.set_accepted_user_terms_version(accepted_user_terms_version.into());
                state.set_accepted_privacy_version(accepted_privacy_version.into());
                state.set_agreement_update_open(false);
                state.set_agreement_update_message("".into());
                save_user_profile(&app);
            }
            Err(error) => {
                state.set_agreement_update_message(auth_error_message(&error).into());
            }
        }
    });
}

pub(super) fn initialize_auth(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let Some(backend) = context.backend.clone() else {
        state.set_auth_open(true);
        state.set_auth_error("后端客户端初始化失败".into());
        return;
    };
    state.set_auth_open(true);
    state.set_auth_busy(true);
    state.set_session_state("refreshing".into());
    state.set_auth_error("正在连接服务端...".into());
    let api = AuthApi::new(backend.api.clone());
    let account_api = AccountApi::new(backend.api.clone());
    let auth_epoch = backend.api.session().auth_epoch();
    let credit_sync_epoch = begin_credit_sync_epoch(&mut context.store.borrow_mut());
    state.set_credit_ledger_loading(false);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let agreements = api.list_agreements();
        let refresh = match backend.api.session().has_refresh_token() {
            Ok(true) => Some(api.refresh_epoch(auth_epoch)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        };
        let snapshot = if matches!(refresh, Some(Ok(_))) {
            Some(account_api.snapshot_epoch(auth_epoch))
        } else {
            None
        };
        let result = StartupAuthResult {
            auth_epoch,
            credit_sync_epoch,
            agreements,
            refresh,
            snapshot,
        };
        let _ = sender.send(result);
    });
    poll_startup_auth_result(
        app.as_weak(),
        context.clone(),
        auth_epoch,
        Rc::new(RefCell::new(Some(receiver))),
    );
    schedule_network_recovery(app.as_weak(), context);
}

fn schedule_network_recovery(app_weak: Weak<AppWindow>, context: AppContext) {
    slint::Timer::single_shot(Duration::from_secs(8), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        try_network_recovery(&app, context.clone());
        schedule_network_recovery(app.as_weak(), context);
    });
}

fn try_network_recovery(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if state.get_auth_busy() || !state.get_ever_authenticated() {
        return;
    }
    if !matches!(state.get_session_state().as_str(), "offline" | "signed_out") {
        return;
    }
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let Ok(true) = backend.api.session().has_refresh_token() else {
        return;
    };
    let auth_epoch = backend.api.session().auth_epoch();
    let credit_sync_epoch = begin_credit_sync_epoch(&mut context.store.borrow_mut());
    state.set_credit_ledger_loading(false);
    state.set_auth_busy(true);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let auth = AuthApi::new(backend.api.clone());
        let result = auth.refresh_epoch(auth_epoch).and_then(|_| {
            let snapshot = AccountApi::new(backend.api.clone()).snapshot_epoch(auth_epoch)?;
            let agreements = auth.list_agreements()?;
            Ok((snapshot, agreements))
        });
        let _ = sender.send((auth_epoch, result));
    });
    poll_network_recovery(
        app.as_weak(),
        context,
        auth_epoch,
        credit_sync_epoch,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_network_recovery(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    expected_auth_epoch: u64,
    credit_sync_epoch: u64,
    receiver: Rc<
        RefCell<
            Option<
                mpsc::Receiver<(
                    u64,
                    std::result::Result<(BackendSnapshot, Vec<AgreementItem>), ApiError>,
                )>,
            >,
        >,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let (auth_epoch, result) = match poll_receiver(&receiver) {
            ReceiverPoll::Pending => {
                poll_network_recovery(
                    app_weak,
                    context,
                    expected_auth_epoch,
                    credit_sync_epoch,
                    receiver,
                );
                return;
            }
            ReceiverPoll::Ready(result) => result,
            ReceiverPoll::Disconnected => {
                if let Some(app) = app_weak.upgrade() {
                    let state = app.global::<AppState>();
                    let epoch_matches = context.backend.as_ref().is_some_and(|backend| {
                        backend.api.session().auth_epoch() == expected_auth_epoch
                    });
                    if epoch_matches
                        && matches!(state.get_session_state().as_str(), "offline" | "signed_out")
                    {
                        state.set_auth_busy(false);
                        state.set_session_state("offline".into());
                        state.set_generation_status("网络恢复任务意外中断，将稍后自动重试".into());
                    }
                }
                return;
            }
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let captured_session_ended = result.as_ref().is_err_and(captured_session_error);
        let terminal_context_matches = terminal_auth_epoch_matches_context(&context, auth_epoch);
        let outcome_is_current = if captured_session_ended {
            terminal_context_matches
        } else {
            context
                .backend
                .as_ref()
                .is_some_and(|backend| backend.api.session().auth_epoch() == auth_epoch)
        };
        if !outcome_is_current {
            return;
        }
        let state = app.global::<AppState>();
        state.set_auth_busy(false);
        match result {
            Ok((snapshot, agreements)) => {
                apply_agreements(&app, &agreements);
                apply_backend_snapshot(&app, &context, snapshot, credit_sync_epoch);
                state.set_logged_in(true);
                state.set_offline_mode(false);
                state.set_session_state("online".into());
                state.set_auth_open(false);
                state.set_auth_error("".into());
                state.set_generation_status("网络已恢复，账号数据已同步".into());
                require_updated_agreements(&app);
                recover_pending_generations(&app, context.clone());
                recover_pending_prompt_tasks(&app, context.clone());
                recover_prompt_optimization(&app, context.clone());
                recover_pending_orders(&app, context.clone());
                refresh_server_notifications(&app, context);
            }
            Err(error) if captured_session_error(&error) && terminal_context_matches => {
                sign_out_locally(&app, &context, true, Some(auth_epoch))
            }
            Err(error) if error.is_client_update_required() => {
                state.set_session_state("update_required".into());
                state.set_auth_open(true);
                state.set_auth_error(update_required_message(&error).into());
                show_required_update_prompt(&app, minimum_version_from_error(&error));
            }
            Err(_) => {}
        }
    });
}

pub(super) fn selected_login_agreement_acceptances(state: &AppState) -> Vec<AgreementAcceptance> {
    let mut acceptances = Vec::new();
    if state.get_auth_user_terms_accepted() {
        acceptances.push(AgreementAcceptance {
            agreement_type: "user_terms".to_string(),
            version: state.get_auth_user_terms_version().to_string(),
        });
    }
    if state.get_auth_privacy_accepted() {
        acceptances.push(AgreementAcceptance {
            agreement_type: "privacy_policy".to_string(),
            version: state.get_auth_privacy_version().to_string(),
        });
    }
    acceptances
}

pub(super) fn begin_wechat_login(
    app: &AppWindow,
    context: AppContext,
    backend: Arc<BackendRuntime>,
) {
    let state = app.global::<AppState>();
    if state.get_auth_wechat_busy() || state.get_auth_busy() {
        return;
    }
    let auth_operation_epoch = begin_auth_operation(&context);
    let acceptances = selected_login_agreement_acceptances(&state);
    state.set_auth_wechat_busy(true);
    state.set_auth_wechat_qr_ready(false);
    state.set_auth_wechat_scanned(false);
    state.set_auth_wechat_poll_elapsed_ms(0);
    state.set_auth_wechat_login_id("".into());
    state.set_auth_wechat_status("正在获取二维码...".into());
    state.set_auth_error("".into());
    let api = AuthApi::new(backend.api.clone());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(api.start_wechat_login(&acceptances));
    });
    poll_wechat_start_result(
        app.as_weak(),
        context,
        auth_operation_epoch,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_wechat_start_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    auth_operation_epoch: u64,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<WechatLoginStartResponse, ApiError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = match poll_receiver(&receiver) {
            ReceiverPoll::Pending => {
                poll_wechat_start_result(app_weak, context, auth_operation_epoch, receiver);
                return;
            }
            ReceiverPoll::Ready(result) => result,
            ReceiverPoll::Disconnected => {
                if let Some(app) = app_weak.upgrade() {
                    let state = app.global::<AppState>();
                    if auth_operation_is_current(&context, auth_operation_epoch)
                        && state.get_auth_open()
                        && state.get_auth_method().as_str() == "wechat"
                    {
                        state.set_auth_wechat_busy(false);
                        state.set_auth_wechat_qr_ready(false);
                        state.set_auth_wechat_status("微信登录任务已中断，请刷新二维码重试".into());
                        state.set_auth_error("微信登录任务已中断，请刷新二维码重试".into());
                    }
                }
                return;
            }
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !auth_operation_is_current(&context, auth_operation_epoch)
            || !state.get_auth_open()
            || state.get_auth_method().as_str() != "wechat"
        {
            return;
        }
        state.set_auth_wechat_busy(false);
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
                    state.set_auth_wechat_qr_image(image);
                    state.set_auth_wechat_qr_ready(true);
                    state.set_auth_wechat_login_id(response.login_id.clone().into());
                    state.set_auth_wechat_expires_in(expires);
                    state.set_auth_wechat_poll_after_ms(poll_after_ms);
                    state.set_auth_wechat_poll_elapsed_ms(0);
                    state.set_auth_wechat_status(
                        format!("等待扫码，二维码 {expires} 秒后失效").into(),
                    );
                    state.set_auth_error("".into());
                    schedule_wechat_status_poll(
                        app.as_weak(),
                        context,
                        auth_operation_epoch,
                        response.login_id,
                        poll_after_ms as u64,
                    );
                }
                Err(_) => {
                    state.set_auth_wechat_qr_ready(false);
                    state.set_auth_wechat_status("二维码生成失败，请点击刷新".into());
                    state.set_auth_error("二维码生成失败，请点击刷新".into());
                }
            },
            Err(error) => {
                let message = auth_error_message(&error);
                state.set_auth_wechat_qr_ready(false);
                state.set_auth_wechat_status(message.clone().into());
                state.set_auth_error(message.into());
            }
        }
    });
}

fn schedule_wechat_status_poll(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    auth_operation_epoch: u64,
    login_id: String,
    delay_milliseconds: u64,
) {
    slint::Timer::single_shot(
        Duration::from_millis(delay_milliseconds.max(250)),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !auth_operation_is_current(&context, auth_operation_epoch)
                || !state.get_auth_open()
                || state.get_auth_method().as_str() != "wechat"
                || state.get_auth_wechat_login_id().as_str() != login_id
            {
                return;
            }
            let Some(backend) = context.backend.clone() else {
                return;
            };
            let api = AuthApi::new(backend.api.clone());
            let request_login_id = login_id.clone();
            let acceptances = selected_login_agreement_acceptances(&state);
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let result = api
                    .wechat_login_status(&request_login_id, &acceptances)
                    .and_then(|status| {
                        match (status.status.as_str(), status.qr_status.as_deref()) {
                            ("pending", Some("scanned")) | ("scanned", _) => {
                                Ok(WechatPollOutcome::Scanned(status.message.unwrap_or_else(
                                    || "已扫码，请在手机微信中确认登录".to_string(),
                                )))
                            }
                            ("pending", _) => Ok(WechatPollOutcome::Pending),
                            ("agreement_required", _) => Ok(WechatPollOutcome::AgreementRequired(
                                status.message.unwrap_or_else(|| {
                                    "请先阅读并同意用户协议和隐私政策".to_string()
                                }),
                            )),
                            ("failed", _) => {
                                Ok(WechatPollOutcome::Failed(status.message.unwrap_or_else(
                                    || "微信登录未完成，请刷新二维码重试".to_string(),
                                )))
                            }
                            ("completed", _) => {
                                let login = status.login.ok_or_else(|| ApiError::Protocol {
                                    message: "微信登录响应缺少登录信息".to_string(),
                                    request_id: None,
                                })?;
                                Ok(WechatPollOutcome::Completed(login))
                            }
                            _ => Err(ApiError::Protocol {
                                message: "微信登录响应状态无效".to_string(),
                                request_id: None,
                            }),
                        }
                    });
                let _ = sender.send(result);
            });
            poll_wechat_status_result(
                app.as_weak(),
                context,
                auth_operation_epoch,
                login_id,
                Rc::new(RefCell::new(Some(receiver))),
            );
        },
    );
}

fn poll_wechat_status_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    auth_operation_epoch: u64,
    login_id: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<WechatPollOutcome, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = match poll_receiver(&receiver) {
            ReceiverPoll::Pending => {
                poll_wechat_status_result(
                    app_weak,
                    context,
                    auth_operation_epoch,
                    login_id,
                    receiver,
                );
                return;
            }
            ReceiverPoll::Ready(result) => result,
            ReceiverPoll::Disconnected => {
                if let Some(app) = app_weak.upgrade() {
                    let state = app.global::<AppState>();
                    if auth_operation_is_current(&context, auth_operation_epoch)
                        && state.get_auth_open()
                        && state.get_auth_method().as_str() == "wechat"
                        && state.get_auth_wechat_login_id().as_str() == login_id
                    {
                        state.set_auth_wechat_login_id("".into());
                        state.set_auth_wechat_qr_ready(false);
                        state.set_auth_wechat_scanned(false);
                        state.set_auth_wechat_status(
                            "微信登录状态检查已中断，请刷新二维码重试".into(),
                        );
                        state.set_auth_error("微信登录状态检查已中断，请刷新二维码重试".into());
                    }
                }
                return;
            }
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !auth_operation_is_current(&context, auth_operation_epoch)
            || !state.get_auth_open()
            || state.get_auth_method().as_str() != "wechat"
            || state.get_auth_wechat_login_id().as_str() != login_id
        {
            return;
        }
        match result {
            Ok(WechatPollOutcome::Pending) => {
                let poll_after_ms = state.get_auth_wechat_poll_after_ms().max(250);
                let (remaining, elapsed_ms) = advance_second_countdown(
                    state.get_auth_wechat_expires_in(),
                    state.get_auth_wechat_poll_elapsed_ms(),
                    poll_after_ms,
                );
                state.set_auth_wechat_expires_in(remaining);
                state.set_auth_wechat_poll_elapsed_ms(elapsed_ms);
                if remaining == 0 {
                    expire_wechat_login(&state);
                    return;
                }
                state.set_auth_wechat_status(
                    format!("等待扫码，二维码 {remaining} 秒后失效").into(),
                );
                schedule_wechat_status_poll(
                    app.as_weak(),
                    context,
                    auth_operation_epoch,
                    login_id,
                    poll_after_ms as u64,
                );
            }
            Ok(WechatPollOutcome::Scanned(message)) => {
                let poll_after_ms = state.get_auth_wechat_poll_after_ms().max(250);
                let (remaining, elapsed_ms) = advance_second_countdown(
                    state.get_auth_wechat_expires_in(),
                    state.get_auth_wechat_poll_elapsed_ms(),
                    poll_after_ms,
                );
                state.set_auth_wechat_expires_in(remaining);
                state.set_auth_wechat_poll_elapsed_ms(elapsed_ms);
                if remaining == 0 {
                    expire_wechat_login(&state);
                    return;
                }
                state.set_auth_wechat_scanned(true);
                state.set_auth_wechat_status(message.into());
                state.set_auth_error("".into());
                schedule_wechat_status_poll(
                    app.as_weak(),
                    context,
                    auth_operation_epoch,
                    login_id,
                    poll_after_ms as u64,
                );
            }
            Ok(WechatPollOutcome::AgreementRequired(message)) => {
                let poll_after_ms = state.get_auth_wechat_poll_after_ms().max(250);
                let (remaining, elapsed_ms) = advance_second_countdown(
                    state.get_auth_wechat_expires_in(),
                    state.get_auth_wechat_poll_elapsed_ms(),
                    poll_after_ms,
                );
                state.set_auth_wechat_expires_in(remaining);
                state.set_auth_wechat_poll_elapsed_ms(elapsed_ms);
                if remaining == 0 {
                    expire_wechat_login(&state);
                    return;
                }
                state.set_auth_wechat_scanned(true);
                state.set_auth_wechat_status(message.clone().into());
                state.set_auth_error(message.into());
                schedule_wechat_status_poll(
                    app.as_weak(),
                    context,
                    auth_operation_epoch,
                    login_id,
                    poll_after_ms as u64,
                );
            }
            Ok(WechatPollOutcome::Failed(message)) => {
                state.set_auth_wechat_login_id("".into());
                state.set_auth_wechat_qr_ready(false);
                state.set_auth_wechat_scanned(false);
                state.set_auth_wechat_status(message.clone().into());
                state.set_auth_error(message.into());
            }
            Ok(WechatPollOutcome::Completed(response)) => {
                let Some(backend) = context.backend.as_ref() else {
                    return;
                };
                let current = auth_operation_is_current(&context, auth_operation_epoch)
                    && state.get_auth_open()
                    && state.get_auth_method().as_str() == "wechat"
                    && state.get_auth_wechat_login_id().as_str() == login_id;
                let installed = install_login_if_current(current, || {
                    backend
                        .api
                        .session()
                        .install_tokens_for_user(&response.tokens, &response.user.id)
                });
                match installed {
                    Ok(Some(_)) => {}
                    Ok(None) => return,
                    Err(error) => {
                        let message = auth_error_message(&error);
                        state.set_auth_wechat_status(message.clone().into());
                        state.set_auth_error(message.into());
                        return;
                    }
                }
                state.set_auth_wechat_login_id("".into());
                state.set_auth_wechat_qr_ready(false);
                state.set_auth_wechat_scanned(false);
                state.set_auth_wechat_status("登录成功".into());
                finish_login(&app, &context, response, None);
                refresh_backend_snapshot(&app, context);
            }
            Err(error) => {
                let message = auth_error_message(&error);
                state.set_auth_wechat_login_id("".into());
                state.set_auth_wechat_qr_ready(false);
                state.set_auth_wechat_scanned(false);
                state.set_auth_wechat_status(message.clone().into());
                state.set_auth_error(message.into());
            }
        }
    });
}

fn poll_login_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    auth_operation_epoch: u64,
    login_mode: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<LoginResult>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = match poll_receiver(&receiver) {
            ReceiverPoll::Pending => {
                poll_login_result(
                    app_weak,
                    context,
                    auth_operation_epoch,
                    login_mode,
                    receiver,
                );
                return;
            }
            ReceiverPoll::Ready(result) => result,
            ReceiverPoll::Disconnected => {
                if let Some(app) = app_weak.upgrade() {
                    let state = app.global::<AppState>();
                    if auth_operation_is_current(&context, auth_operation_epoch)
                        && state.get_auth_open()
                        && state.get_auth_method().as_str() == "email"
                        && state.get_auth_email_mode().as_str() == login_mode.as_str()
                        && state.get_session_state().as_str() == "authenticating"
                    {
                        state.set_auth_busy(false);
                        state.set_session_state("signed_out".into());
                        state.set_auth_error("登录任务已中断，请重试".into());
                    }
                }
                return;
            }
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !auth_operation_is_current(&context, auth_operation_epoch)
            || !state.get_auth_open()
            || state.get_auth_method().as_str() != "email"
            || state.get_auth_email_mode().as_str() != login_mode.as_str()
            || state.get_session_state().as_str() != "authenticating"
        {
            return;
        }
        state.set_auth_busy(false);
        match result {
            Ok(response) => {
                let Some(backend) = context.backend.as_ref() else {
                    return;
                };
                let current = auth_operation_is_current(&context, auth_operation_epoch)
                    && state.get_auth_open()
                    && state.get_auth_method().as_str() == "email"
                    && state.get_auth_email_mode().as_str() == login_mode.as_str();
                match install_login_if_current(current, || {
                    backend
                        .api
                        .session()
                        .install_tokens_for_user(&response.tokens, &response.user.id)
                }) {
                    Ok(Some(_)) => {
                        finish_login(&app, &context, response, None);
                        refresh_backend_snapshot(&app, context);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        state.set_session_state("signed_out".into());
                        apply_auth_error(&app, error);
                    }
                }
            }
            Err(error) => {
                state.set_session_state("signed_out".into());
                apply_email_login_error(&app, &login_mode, error);
            }
        }
    });
}

pub(super) fn finish_login(
    app: &AppWindow,
    context: &AppContext,
    response: LoginResponse,
    snapshot: Option<(u64, std::result::Result<BackendSnapshot, ApiError>)>,
) {
    // Never expose the previous account's membership, credits, catalog, or purchase state while
    // the new account snapshot is still in flight.
    if snapshot.is_none() {
        invalidate_credit_sync_epoch(&mut context.store.borrow_mut());
    }
    clear_account_snapshot_state(app, context);
    clear_payment_account_state(app, context);
    *context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner()) = Some(response.user.id.clone());
    let state = app.global::<AppState>();
    state.set_logged_in(true);
    state.set_offline_mode(false);
    state.set_session_state("online".into());
    state.set_ever_authenticated(true);
    state.set_offline_available(true);
    state.set_email_mask(response.user.email_masked.into());
    state.set_nickname(response.user.nickname.unwrap_or_default().into());
    state.set_auth_code("".into());
    state.set_auth_password("".into());
    clear_password_reset_state(&state);
    state.set_auth_error("".into());
    state.set_auth_open(false);
    state.set_agreement_update_busy(false);
    if state.get_auth_user_terms_accepted() {
        state.set_accepted_user_terms_version(state.get_auth_user_terms_version());
    }
    if state.get_auth_privacy_accepted() {
        state.set_accepted_privacy_version(state.get_auth_privacy_version());
    }
    save_user_profile(app);
    if let Some((credit_sync_epoch, snapshot)) = snapshot {
        match snapshot {
            Ok(snapshot) => apply_backend_snapshot(app, context, snapshot, credit_sync_epoch),
            Err(error) => state.set_generation_status(
                format!("账号数据同步失败：{}", auth_error_message(&error)).into(),
            ),
        }
    }
    recover_pending_generations(app, context.clone());
    recover_pending_prompt_tasks(app, context.clone());
    recover_prompt_optimization(app, context.clone());
    recover_pending_orders(app, context.clone());
    navigate_to_with_store(app, &context.store.borrow(), "generation");
}

fn poll_startup_auth_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    startup_auth_epoch: u64,
    receiver: Rc<RefCell<Option<mpsc::Receiver<StartupAuthResult>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = match poll_receiver(&receiver) {
            ReceiverPoll::Pending => {
                poll_startup_auth_result(app_weak, context, startup_auth_epoch, receiver);
                return;
            }
            ReceiverPoll::Ready(result) => result,
            ReceiverPoll::Disconnected => {
                if let Some(app) = app_weak.upgrade() {
                    let state = app.global::<AppState>();
                    let epoch_matches = context.backend.as_ref().is_some_and(|backend| {
                        backend.api.session().auth_epoch() == startup_auth_epoch
                    });
                    if epoch_matches && state.get_session_state().as_str() == "refreshing" {
                        state.set_auth_busy(false);
                        state.set_session_state("signed_out".into());
                        state.set_auth_open(true);
                        state.set_auth_error("登录状态恢复任务已中断，请重试".into());
                    }
                }
                return;
            }
        };
        if let Some(app) = app_weak.upgrade() {
            apply_startup_auth(&app, &context, result);
        }
    });
}

#[derive(Debug, Eq, PartialEq)]
enum ReceiverPoll<T> {
    Pending,
    Ready(T),
    Disconnected,
}

fn poll_receiver<T>(receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>) -> ReceiverPoll<T> {
    let mut slot = receiver.borrow_mut();
    let Some(receiver) = slot.as_ref() else {
        return ReceiverPoll::Disconnected;
    };
    match receiver.try_recv() {
        Ok(result) => {
            slot.take();
            ReceiverPoll::Ready(result)
        }
        Err(TryRecvError::Empty) => ReceiverPoll::Pending,
        Err(TryRecvError::Disconnected) => {
            slot.take();
            ReceiverPoll::Disconnected
        }
    }
}

fn apply_startup_auth(app: &AppWindow, context: &AppContext, result: StartupAuthResult) {
    let startup_auth_epoch = result.auth_epoch;
    let credit_sync_epoch = result.credit_sync_epoch;
    let Some(session) = context
        .backend
        .as_ref()
        .map(|backend| backend.api.session())
    else {
        return;
    };
    let terminal_context_matches = terminal_auth_epoch_matches_context(context, result.auth_epoch);
    let captured_session_ended = startup_result_ended_captured_session(&result);
    let outcome_is_current = if captured_session_ended {
        terminal_context_matches
    } else {
        match result.refresh.as_ref() {
            Some(Ok(_)) => session.auth_epoch() == result.auth_epoch && session.access().is_some(),
            Some(Err(_)) | None => session.auth_epoch() == result.auth_epoch,
        }
    };
    if !outcome_is_current {
        return;
    }
    if captured_session_ended {
        sign_out_locally(app, context, true, Some(startup_auth_epoch));
        return;
    }
    let state = app.global::<AppState>();
    state.set_auth_busy(false);
    let agreement_error = match result.agreements {
        Ok(items) => {
            apply_agreements(app, &items);
            None
        }
        Err(error) => Some(auth_error_message(&error)),
    };
    match result.refresh {
        Some(Ok(_)) => {
            clear_account_snapshot_state(app, context);
            clear_payment_account_state(app, context);
            state.set_logged_in(true);
            state.set_offline_mode(false);
            state.set_session_state("online".into());
            state.set_ever_authenticated(true);
            state.set_offline_available(true);
            state.set_auth_open(false);
            state.set_auth_error("".into());
            save_user_profile(app);
            if let Some(snapshot) = result.snapshot {
                match snapshot {
                    Ok(snapshot) => {
                        apply_backend_snapshot(app, context, snapshot, credit_sync_epoch)
                    }
                    Err(error) => state.set_generation_status(
                        format!("账号数据同步失败：{}", auth_error_message(&error)).into(),
                    ),
                }
            }
            recover_pending_generations(app, context.clone());
            recover_pending_prompt_tasks(app, context.clone());
            recover_prompt_optimization(app, context.clone());
            recover_pending_orders(app, context.clone());
            require_updated_agreements(app);
            navigate_to_with_store(app, &context.store.borrow(), "generation");
        }
        Some(Err(error)) => {
            let disposition = if captured_session_error(&error) && terminal_context_matches {
                StartupErrorDisposition::TerminalSession
            } else {
                startup_error_disposition(&error, state.get_offline_available())
            };
            match disposition {
                StartupErrorDisposition::UpdateRequired => {
                    state.set_session_state("update_required".into());
                    state.set_auth_open(true);
                    state.set_auth_error(update_required_message(&error).into());
                    show_required_update_prompt(app, minimum_version_from_error(&error));
                }
                StartupErrorDisposition::OfferOffline => {
                    state.set_session_state("signed_out".into());
                    state.set_auth_open(true);
                    state.set_auth_error("暂时无法连接服务端，可重试登录或离线使用".into());
                    if state.get_auth_method().as_str() == "wechat" {
                        if let Some(backend) = context.backend.clone() {
                            begin_wechat_login(app, context.clone(), backend);
                        }
                    }
                }
                StartupErrorDisposition::TerminalSession => {
                    let _ = state;
                    sign_out_locally(app, context, true, Some(startup_auth_epoch));
                }
                StartupErrorDisposition::Recoverable => {
                    state.set_session_state("signed_out".into());
                    state.set_auth_open(true);
                    state.set_auth_error(auth_error_message(&error).into());
                    if state.get_auth_method().as_str() == "wechat" {
                        if let Some(backend) = context.backend.clone() {
                            begin_wechat_login(app, context.clone(), backend);
                        }
                    }
                }
            }
        }
        None => {
            state.set_session_state("signed_out".into());
            state.set_auth_open(true);
            state.set_auth_error(agreement_error.unwrap_or_default().into());
            if state.get_auth_method().as_str() == "wechat" {
                if let Some(backend) = context.backend.clone() {
                    begin_wechat_login(app, context.clone(), backend);
                }
            }
        }
    }
}

fn startup_result_ended_captured_session(result: &StartupAuthResult) -> bool {
    result
        .refresh
        .as_ref()
        .is_some_and(|refresh| refresh.as_ref().is_err_and(captured_session_error))
        || result
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.as_ref().is_err_and(captured_session_error))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupErrorDisposition {
    UpdateRequired,
    OfferOffline,
    TerminalSession,
    Recoverable,
}

fn startup_error_disposition(error: &ApiError, offline_available: bool) -> StartupErrorDisposition {
    if error.is_client_update_required() {
        StartupErrorDisposition::UpdateRequired
    } else if error.is_terminal_session_error() {
        StartupErrorDisposition::TerminalSession
    } else if error.is_network_error() && offline_available {
        StartupErrorDisposition::OfferOffline
    } else {
        StartupErrorDisposition::Recoverable
    }
}

fn clear_credit_redemption_state(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_credit_redemption_code("".into());
    state.set_credit_redemption_busy(false);
    state.set_credit_redemption_success(false);
    state.set_credit_redemption_message("".into());
}

pub(super) fn clear_account_snapshot_state(app: &AppWindow, context: &AppContext) {
    *context
        .account_snapshot_scope
        .lock()
        .unwrap_or_else(|value| value.into_inner()) = None;

    let state = app.global::<AppState>();
    state.set_email_mask("".into());
    state.set_nickname("".into());
    state.set_profile_name("".into());
    state.set_membership_plan_code("free".into());
    state.set_membership_plan_name("免费版".into());
    state.set_membership_ends_at("".into());
    state.set_membership_expiry_message("".into());
    state.set_membership_tier_rank(0);
    state.set_membership_plans(ModelRc::new(VecModel::from(
        Vec::<MembershipPlanView>::new(),
    )));
    state.set_membership_open(false);
    state.set_membership_payment_busy(false);
    state.set_membership_payment_message("".into());
    state.set_account_sessions(ModelRc::new(VecModel::from(Vec::<AccountSession>::new())));

    state.set_credit_balance("0".into());
    state.set_credit_reserved("0".into());
    invalidate_credit_account_view(&context.store);
    reset_credit_ledger(app, &context.store, &[], None);
    state.set_credit_packs(ModelRc::new(VecModel::from(Vec::<CreditPackView>::new())));
    state.set_selected_credit_pack_code("".into());
    state.set_selected_credit_amount("".into());
    state.set_selected_credit_price("".into());
    state.set_credit_payment_busy(false);
    state.set_credit_payment_message("".into());
    clear_credit_redemption_state(app);
    context
        .store
        .borrow_mut()
        .pending_credit_redemptions_by_owner
        .clear();
    state.set_credit_insufficient_open(false);
    state.set_credit_insufficient_message("积分不足以支持本次生图，请前往充值".into());

    state.set_email_bound(false);
    state.set_email_bind_open(false);
    state.set_email_bind_email("".into());
    state.set_email_bind_code("".into());
    state.set_email_bind_code_busy(false);
    state.set_email_bind_busy(false);
    state.set_email_bind_countdown(0);
    state.set_email_bind_status("".into());
    state.set_wechat_bound(false);
    state.set_wechat_can_unbind(false);
    state.set_wechat_bound_name("".into());
    state.set_wechat_bind_open(false);
    state.set_wechat_bind_busy(false);
    state.set_wechat_bind_login_id("".into());
    state.set_wechat_bind_qr_ready(false);
    state.set_wechat_bind_scanned(false);
    state.set_wechat_bind_status("".into());
    state.set_wechat_bind_expires_in(0);
    state.set_wechat_bind_poll_elapsed_ms(0);
    state.set_wechat_unbind_confirm_open(false);

    state.set_invitation_code("".into());
    state.set_invitation_code_busy(false);
    state.set_invitation_code_success(false);
    state.set_invitation_code_submitted(false);
    state.set_invitation_code_status("".into());
    state.set_invitation_reward_rate("".into());
    state.set_invitation_count("".into());
    state.set_invitation_history_reward("".into());
    state.set_invitation_own_code("".into());
    state.set_invitation_rule_description("".into());
    state.set_invitation_rewards_status("".into());
    state.set_invitation_users(ModelRc::new(VecModel::from(Vec::<InvitedUserView>::new())));
    state.set_invitation_users_loading(false);
    state.set_invitation_users_has_more(false);
    state.set_invitation_users_next_cursor("".into());
    state.set_invitation_users_message("".into());

    state.set_catalog_models(ModelRc::new(VecModel::from(Vec::<CatalogModelView>::new())));
    state.set_image_model("".into());
    state.set_image_model_name("".into());
    state.set_reasoning_model("".into());
    state.set_reasoning_model_name("".into());
    state.set_image_price_1k(0);
    state.set_image_price_2k(0);
    state.set_image_price_4k(0);
    state.set_image_editor_model("".into());
    state.set_image_editor_model_name("".into());
    state.set_image_editor_price_1k(0);
    state.set_image_editor_price_2k(0);
    state.set_image_editor_price_4k(0);
    state.set_style_analysis_available(false);
    state.set_style_analysis_model_code("".into());
    state.set_style_analysis_display_name("".into());
    state.set_style_analysis_credit_cost("".into());
    state.set_model_catalog_message("".into());
    {
        let mut store = context.store.borrow_mut();
        store.model_groups.clear();
        push_model_groups(app, &store);
    }
}

pub(super) fn account_snapshot_scope_is_current(
    context: &AppContext,
    session_scope: &SessionScope,
) -> bool {
    context
        .account_snapshot_scope
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .as_ref()
        == Some(session_scope)
}

pub(super) fn refresh_backend_snapshot(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let Some(session_scope) = current_auth_session_scope(&context) else {
        return;
    };
    let credit_sync_epoch = begin_credit_sync_epoch(&mut context.store.borrow_mut());
    app.global::<AppState>().set_credit_ledger_loading(true);
    let (sender, receiver) = mpsc::channel();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let _ = sender
            .send(AccountApi::new(backend.api.clone()).snapshot_epoch(worker_scope.auth_epoch));
    });
    poll_backend_snapshot(
        app.as_weak(),
        context,
        session_scope,
        credit_sync_epoch,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_backend_snapshot(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    credit_sync_epoch: u64,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<BackendSnapshot, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = match poll_receiver(&receiver) {
            ReceiverPoll::Pending => {
                poll_backend_snapshot(
                    app_weak,
                    context,
                    session_scope,
                    credit_sync_epoch,
                    receiver,
                );
                return;
            }
            ReceiverPoll::Ready(result) => result,
            ReceiverPoll::Disconnected => {
                if let Some(app) = app_weak.upgrade() {
                    if auth_scope_matches_context(&context, &session_scope)
                        && credit_sync_epoch_is_current(&context.store.borrow(), credit_sync_epoch)
                    {
                        let state = app.global::<AppState>();
                        state.set_credit_ledger_loading(false);
                        state.set_generation_status(
                            "账号数据刷新任务已中断，请稍后重试；支付功能暂不可用".into(),
                        );
                    }
                }
                return;
            }
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let captured_session_ended = result.as_ref().is_err_and(captured_session_error);
        let terminal_context_matches =
            terminal_auth_scope_matches_context(&context, &session_scope);
        let outcome_is_current = if captured_session_ended {
            terminal_context_matches
        } else {
            auth_scope_matches_context(&context, &session_scope)
        };
        if !outcome_is_current {
            return;
        }
        match result {
            Ok(snapshot) => {
                apply_backend_snapshot(&app, &context, snapshot, credit_sync_epoch);
                recover_pending_orders(&app, context.clone());
            }
            Err(error) if captured_session_error(&error) && terminal_context_matches => {
                sign_out_locally(&app, &context, true, Some(session_scope.auth_epoch))
            }
            Err(error) => {
                if credit_sync_epoch_is_current(&context.store.borrow(), credit_sync_epoch) {
                    let state = app.global::<AppState>();
                    state.set_credit_ledger_loading(false);
                    state.set_generation_status(
                        format!("账号数据刷新失败：{}", auth_error_message(&error)).into(),
                    );
                }
            }
        }
    });
}

pub(super) fn apply_backend_snapshot(
    app: &AppWindow,
    context: &AppContext,
    snapshot: BackendSnapshot,
    credit_sync_epoch: u64,
) {
    if !credit_sync_epoch_is_current(&context.store.borrow(), credit_sync_epoch) {
        return;
    }
    // This request now owns the credit view. Clear any loading state left by the older request it
    // invalidated, including paths where the snapshot is rejected before its ledger is applied.
    app.global::<AppState>().set_credit_ledger_loading(false);
    let Some(backend) = context.backend.as_ref() else {
        return;
    };
    let session = backend.api.session();
    if session.bind_user(&snapshot.account.user.id).is_err() {
        return;
    }
    let Some(snapshot_scope) = session.scope_for_user(&snapshot.account.user.id) else {
        return;
    };
    let account_changed = {
        let mut current_user_id = context
            .current_user_id
            .lock()
            .unwrap_or_else(|value| value.into_inner());
        let changed = current_user_id.as_deref() != Some(snapshot.account.user.id.as_str());
        *current_user_id = Some(snapshot.account.user.id.clone());
        changed
    };
    let state = app.global::<AppState>();
    if account_changed {
        clear_credit_redemption_state(app);
        invalidate_credit_account_view(&context.store);
        context
            .store
            .borrow_mut()
            .pending_credit_redemptions_by_owner
            .clear();
    }
    state.set_email_mask(snapshot.account.user.email_masked.clone().into());
    state.set_invitation_code_submitted(snapshot.account.user.invitation_code_submitted);
    if snapshot.account.user.invitation_code_submitted {
        state.set_invitation_code("".into());
        state.set_invitation_code_success(true);
        state.set_invitation_code_status("当前账号已填写过邀请码，每个账号只能填写一次".into());
    } else {
        state.set_invitation_code_success(false);
        state.set_invitation_code_status("".into());
    }
    if let Some(invitation) = snapshot.invitation.as_ref() {
        apply_invitation_dashboard(app, invitation);
    } else {
        state.set_invitation_reward_rate("".into());
        state.set_invitation_count("".into());
        state.set_invitation_history_reward("".into());
        state.set_invitation_own_code("".into());
        state.set_invitation_rule_description("".into());
        state.set_invitation_rewards_status("".into());
        state.set_invitation_users(ModelRc::new(VecModel::from(Vec::<InvitedUserView>::new())));
        state.set_invitation_users_loading(false);
        state.set_invitation_users_has_more(false);
        state.set_invitation_users_next_cursor("".into());
        state.set_invitation_users_message("".into());
    }
    state.set_nickname(
        snapshot
            .account
            .user
            .nickname
            .clone()
            .unwrap_or_default()
            .into(),
    );
    state.set_email_bound(snapshot.account.auth_methods.email.bound);
    state.set_wechat_bound(snapshot.account.auth_methods.wechat.bound);
    state.set_wechat_can_unbind(snapshot.account.auth_methods.wechat.can_unbind);
    state.set_wechat_bound_name(
        snapshot
            .account
            .auth_methods
            .wechat
            .nickname
            .clone()
            .unwrap_or_default()
            .into(),
    );
    if let Some(plan) = snapshot.account.membership.plan.as_ref() {
        state.set_membership_plan_code(plan.code.clone().into());
        state.set_membership_plan_name(plan.name.clone().into());
        state.set_membership_tier_rank(plan.tier_rank);
    } else {
        state.set_membership_plan_code("free".into());
        state.set_membership_plan_name("免费版".into());
        state.set_membership_tier_rank(0);
    }
    let membership_ends_at = snapshot
        .account
        .membership
        .ends_at
        .clone()
        .unwrap_or_default();
    state.set_membership_ends_at(format_membership_ends_at(&membership_ends_at).into());
    state.set_membership_expiry_message(membership_expiry_message(&membership_ends_at).into());
    let credit_snapshot_applied = if let Some(credits) = snapshot.account.credits.as_ref() {
        apply_credit_account_balance_if_fresh(
            app,
            &context.store,
            &credits.available,
            &credits.reserved,
            &credits.version,
            credit_sync_epoch,
        )
    } else {
        invalidate_credit_account_view(&context.store);
        state.set_credit_balance("0".into());
        state.set_credit_reserved("0".into());
        true
    };
    let packs = snapshot
        .packs
        .iter()
        .map(|pack| CreditPackView {
            code: pack.code.clone().into(),
            name: pack.name.clone().into(),
            credits: pack.credits.clone().into(),
            price: format_cents(credit_pack_price_cents(pack)).into(),
            price_cents: credit_pack_price_cents(pack).into(),
            note: credit_pack_note(pack).into(),
        })
        .collect::<Vec<_>>();
    let selected_code = state.get_selected_credit_pack_code().to_string();
    if let Some(selected) = snapshot
        .packs
        .iter()
        .find(|pack| pack.code == selected_code)
        .or_else(|| snapshot.packs.first())
    {
        state.set_selected_credit_pack_code(selected.code.clone().into());
        state.set_selected_credit_amount(selected.credits.clone().into());
        state.set_selected_credit_price(format_cents(credit_pack_price_cents(selected)).into());
    } else {
        state.set_selected_credit_pack_code("".into());
        state.set_selected_credit_amount("".into());
        state.set_selected_credit_price("".into());
    }
    state.set_credit_packs(ModelRc::new(VecModel::from(packs)));
    state.set_membership_plans(ModelRc::new(VecModel::from(
        snapshot
            .plans
            .iter()
            .map(|plan| MembershipPlanView {
                code: plan.code.clone().into(),
                name: plan.name.clone().into(),
                price: format_cents(&plan.price_cents).into(),
                grant_credits: plan.grant_credits.clone().into(),
                period_days: plan.period_days,
                tier_rank: plan.tier_rank,
            })
            .collect::<Vec<_>>(),
    )));
    let catalog_models = snapshot
        .models
        .iter()
        .map(|model| CatalogModelView {
            code: model.code.clone().into(),
            name: model_display_name(model).into(),
            purpose: model.purpose.clone().into(),
            version: model.version.min(i32::MAX as u32) as i32,
            capabilities: model_capabilities_text(model).into(),
            pricing: model
                .prices
                .iter()
                .map(|price| match price.max_long_edge {
                    Some(edge) => format!(
                        "{}：{} 积分（最长边 {}）",
                        price.quality, price.credit_cost, edge
                    ),
                    None => format!("{}：{} 积分", price.quality, price.credit_cost),
                })
                .collect::<Vec<_>>()
                .join(" · ")
                .into(),
            price_1k: model_price(model, "1K"),
            price_2k: model_price(model, "2K"),
            price_4k: model_price(model, "4K"),
            price_standard: model_credit_cost(model, "standard").into(),
            supports_image_edit: model_supports_task_type(model, "image_edit")
                && model_capability_enabled(model, "supports_masks"),
            supports_style_analysis: model_supports_task_type(model, "image_style_analysis")
                && model_capability_enabled(model, "supports_references")
                && model_supports_operation(model, "analyze_style"),
        })
        .collect::<Vec<_>>();
    state.set_catalog_models(ModelRc::new(VecModel::from(catalog_models)));
    let credit_snapshot_is_current =
        credit_sync_epoch_is_current(&context.store.borrow(), credit_sync_epoch);
    if credit_snapshot_applied && credit_snapshot_is_current {
        reset_credit_ledger(
            app,
            &context.store,
            &snapshot.ledger,
            snapshot.ledger_next_cursor.clone(),
        );
    }
    state.set_account_sessions(ModelRc::new(VecModel::from(
        snapshot
            .sessions
            .iter()
            .map(|session| AccountSession {
                id: session.id.clone().into(),
                device_name: session.device_name.clone().into(),
                platform: session.platform.clone().into(),
                app_version: session.app_version.clone().into(),
                last_seen_at: session.last_seen_at.clone().into(),
                is_current: session.is_current,
            })
            .collect::<Vec<_>>(),
    )));

    let image_models = snapshot
        .models
        .iter()
        .filter(|item| item.purpose == "image_generation")
        .map(|item| ModelOptionData {
            code: item.code.clone(),
            name: model_display_name(item),
        })
        .collect::<Vec<_>>();
    let prompt_models = snapshot
        .models
        .iter()
        .filter(|item| item.purpose == "prompt_processing")
        .map(|item| ModelOptionData {
            code: item.code.clone(),
            name: item.name.clone(),
        })
        .collect::<Vec<_>>();
    let selected_image_code = state.get_image_model().to_string();
    let selected_image = snapshot
        .models
        .iter()
        .find(|item| item.purpose == "image_generation" && item.code == selected_image_code)
        .or_else(|| {
            snapshot
                .models
                .iter()
                .find(|item| item.code == "openai_image")
        })
        .or_else(|| {
            snapshot
                .models
                .iter()
                .find(|item| item.purpose == "image_generation")
        });
    let selected_prompt_code = state.get_reasoning_model().to_string();
    let selected_prompt = snapshot
        .models
        .iter()
        .find(|item| item.purpose == "prompt_processing" && item.code == selected_prompt_code)
        .or_else(|| {
            snapshot
                .models
                .iter()
                .find(|item| item.purpose == "prompt_processing")
        });
    let mut model_groups = Vec::new();
    if !image_models.is_empty() {
        model_groups.push(model_group(
            "image",
            "平台图像模型",
            image_models.clone(),
            selected_image
                .map(|model| model.code.as_str())
                .unwrap_or_default(),
        ));
    }
    if !prompt_models.is_empty() {
        model_groups.push(model_group(
            "reasoning",
            "平台提示词模型",
            prompt_models.clone(),
            selected_prompt
                .map(|model| model.code.as_str())
                .unwrap_or_default(),
        ));
    }
    {
        let mut store = context.store.borrow_mut();
        store.model_groups = model_groups;
        push_model_groups(app, &store);
    }
    if let Some(model) = selected_image {
        apply_image_model(&state, model);
    }
    if let Some(model) = selected_prompt {
        state.set_reasoning_model(model.code.clone().into());
        state.set_reasoning_model_name(model.name.clone().into());
    }
    sync_style_analysis_selection(&state);
    *context
        .account_snapshot_scope
        .lock()
        .unwrap_or_else(|value| value.into_inner()) = Some(snapshot_scope);
    save_user_profile(app);
}

fn model_group(
    kind: &str,
    name: &str,
    models: Vec<ModelOptionData>,
    selected_model: &str,
) -> ModelGroupData {
    let model_codes = models
        .iter()
        .map(|model| model.code.clone())
        .collect::<Vec<_>>();
    let selected_model = model_codes
        .iter()
        .find(|code| code.as_str() == selected_model)
        .cloned()
        .or_else(|| model_codes.first().cloned())
        .unwrap_or_default();
    ModelGroupData {
        kind: kind.to_string(),
        name: name.to_string(),
        selected_model,
        used_models: model_codes,
        models,
    }
}

fn model_price(model: &ModelCatalogItem, quality: &str) -> i32 {
    model
        .prices
        .iter()
        .find(|price| price.quality == quality)
        .map(|price| decimal_to_i32(&price.credit_cost))
        .unwrap_or(0)
}

fn model_credit_cost(model: &ModelCatalogItem, quality: &str) -> String {
    model
        .prices
        .iter()
        .find(|price| price.quality == quality)
        .map(|price| price.credit_cost.clone())
        .unwrap_or_default()
}

fn model_supports_task_type(model: &ModelCatalogItem, task_type: &str) -> bool {
    model
        .capabilities
        .get("task_types")
        .and_then(Value::as_array)
        .is_some_and(|task_types| {
            task_types
                .iter()
                .any(|value| value.as_str() == Some(task_type))
        })
}

fn model_capability_enabled(model: &ModelCatalogItem, capability: &str) -> bool {
    model
        .capabilities
        .get(capability)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn model_supports_operation(model: &ModelCatalogItem, operation: &str) -> bool {
    model
        .capabilities
        .get("operations")
        .and_then(Value::as_array)
        .is_some_and(|operations| {
            operations
                .iter()
                .any(|value| value.as_str() == Some(operation))
        })
}

fn apply_image_model(state: &AppState, model: &ModelCatalogItem) {
    state.set_image_model(model.code.clone().into());
    state.set_image_model_name(model_display_name(model).into());
    state.set_image_price_1k(model_price(model, "1K"));
    state.set_image_price_2k(model_price(model, "2K"));
    state.set_image_price_4k(model_price(model, "4K"));
}

fn model_display_name(model: &ModelCatalogItem) -> String {
    if model.code == "nano_banana" {
        "nano-banana-2".to_string()
    } else {
        model.name.clone()
    }
}

fn decimal_to_i32(value: &str) -> i32 {
    value
        .parse::<i64>()
        .unwrap_or(0)
        .clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

fn membership_expiry_message(ends_at: &str) -> String {
    let Ok(ends_at) = chrono::DateTime::parse_from_rfc3339(ends_at) else {
        return String::new();
    };
    let remaining = ends_at.signed_duration_since(Local::now());
    if remaining.num_seconds() <= 0 {
        return "会员已到期，请续费后继续使用会员权益".to_string();
    }
    if remaining.num_days() < 7 {
        return format!("会员将在 {} 天内到期，请及时续费", remaining.num_days() + 1);
    }
    String::new()
}

fn format_membership_ends_at(ends_at: &str) -> String {
    let Ok(ends_at) = chrono::DateTime::parse_from_rfc3339(ends_at) else {
        return ends_at.to_string();
    };
    ends_at
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn format_cents(value: &str) -> String {
    let value = value.trim();
    let (sign, digits) = value
        .strip_prefix('-')
        .map(|digits| ("-", digits))
        .unwrap_or(("", value));
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return format!("¥ {value}");
    }
    let normalized = digits.trim_start_matches('0');
    let normalized = if normalized.is_empty() {
        "0"
    } else {
        normalized
    };
    let padded = format!("{:0>3}", normalized);
    let split = padded.len() - 2;
    format!("¥ {sign}{}.{}", &padded[..split], &padded[split..])
}

fn credit_pack_price_cents(pack: &CreditPack) -> &str {
    pack.payable_price_cents
        .as_deref()
        .unwrap_or(&pack.price_cents)
}

fn credit_pack_note(pack: &CreditPack) -> String {
    let discount_bps = pack.recharge_discount_bps.unwrap_or(10000);
    let discount_amount = pack.discount_amount_cents.as_deref().unwrap_or("0");
    if discount_bps < 10000 && discount_amount != "0" {
        return format!(
            "会员 {} 折 · 已优惠 {}",
            discount_bps / 100,
            format_cents(discount_amount),
        );
    }
    format!("{} 积分 · 服务端实时计价", pack.credits)
}

fn model_capabilities_text(model: &ModelCatalogItem) -> String {
    let mut parts = Vec::new();
    if let Some(ratios) = model
        .capabilities
        .get("aspect_ratios")
        .and_then(Value::as_array)
    {
        let values = ratios
            .iter()
            .filter_map(Value::as_str)
            .map(client_ratio_from_api)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            parts.push(format!("比例：{}", values.join("/")));
        }
    }
    if model
        .capabilities
        .get("supports_references")
        .and_then(Value::as_bool)
        == Some(true)
    {
        parts.push("支持参考图".to_string());
    }
    if let Some(operations) = model
        .capabilities
        .get("operations")
        .and_then(Value::as_array)
    {
        let values = operations
            .iter()
            .filter_map(Value::as_str)
            .map(|operation| match operation {
                "optimize" => "提示词优化",
                "translate" => "提示词翻译",
                value => value,
            })
            .collect::<Vec<_>>();
        if !values.is_empty() {
            parts.push(values.join("/"));
        }
    }
    if parts.is_empty() {
        "服务端模型能力".to_string()
    } else {
        parts.join(" · ")
    }
}

pub(super) fn apply_agreements(app: &AppWindow, agreements: &[AgreementItem]) {
    let state = app.global::<AppState>();
    for agreement in agreements {
        match (
            agreement.required_action.as_str(),
            agreement.agreement_type.as_str(),
        ) {
            ("login", "user_terms") => {
                state.set_auth_user_terms_required(agreement.required);
                state.set_auth_user_terms_title(agreement.title.clone().into());
                state.set_auth_user_terms_version(agreement.version.clone().into());
                state.set_auth_user_terms_url(agreement.content_url.clone().into());
            }
            ("login", "privacy_policy") => {
                state.set_auth_privacy_required(agreement.required);
                state.set_auth_privacy_title(agreement.title.clone().into());
                state.set_auth_privacy_version(agreement.version.clone().into());
                state.set_auth_privacy_url(agreement.content_url.clone().into());
            }
            ("purchase", "membership_service") => {
                state.set_purchase_membership_required(agreement.required);
                state.set_purchase_membership_title(agreement.title.clone().into());
                state.set_purchase_membership_version(agreement.version.clone().into());
                state.set_purchase_membership_url(agreement.content_url.clone().into());
            }
            ("purchase", "credit_rules") => {
                state.set_purchase_credit_rules_required(agreement.required);
                state.set_purchase_credit_rules_title(agreement.title.clone().into());
                state.set_purchase_credit_rules_version(agreement.version.clone().into());
                state.set_purchase_credit_rules_url(agreement.content_url.clone().into());
            }
            _ => {}
        }
    }
    state.set_auth_user_terms_accepted(true);
    state.set_auth_privacy_accepted(true);
}

fn require_updated_agreements(app: &AppWindow) {
    let state = app.global::<AppState>();
    let terms_outdated = state.get_auth_user_terms_required()
        && state.get_accepted_user_terms_version() != state.get_auth_user_terms_version();
    let privacy_outdated = state.get_auth_privacy_required()
        && state.get_accepted_privacy_version() != state.get_auth_privacy_version();
    if terms_outdated || privacy_outdated {
        state.set_auth_user_terms_accepted(!terms_outdated);
        state.set_auth_privacy_accepted(!privacy_outdated);
        state.set_agreement_update_open(true);
        state.set_agreement_update_message("".into());
    }
}

fn apply_auth_error(app: &AppWindow, error: ApiError) {
    let state = app.global::<AppState>();
    if error.is_client_update_required() {
        state.set_session_state("update_required".into());
        show_required_update_prompt(app, minimum_version_from_error(&error));
    }
    let message = if error.is_client_update_required() {
        update_required_message(&error)
    } else {
        auth_error_message(&error)
    };
    state.set_auth_error(message.into());
}

fn apply_email_login_error(app: &AppWindow, login_mode: &str, error: ApiError) {
    if login_mode == "password"
        && matches!(
            &error,
            ApiError::Http {
                status: 404 | 405 | 501,
                ..
            }
        )
    {
        app.global::<AppState>()
            .set_auth_error("密码登录服务暂未开放，请使用验证码登录".into());
        return;
    }
    apply_auth_error(app, error);
}

fn update_required_message(error: &ApiError) -> String {
    format!(
        "当前客户端版本过旧，在线功能要求至少升级到 {}",
        minimum_version_from_error(error)
    )
}

fn minimum_version_from_error(error: &ApiError) -> &str {
    match error {
        ApiError::Http {
            details: Some(details),
            ..
        } => details
            .get("minimum_version")
            .and_then(Value::as_str)
            .unwrap_or("最新版本"),
        _ => "最新版本",
    }
}

fn auth_error_message(error: &ApiError) -> String {
    error.user_message()
}

pub(super) fn sign_out_locally(
    app: &AppWindow,
    context: &AppContext,
    revoked: bool,
    expected_auth_epoch: Option<u64>,
) {
    if revoked {
        if let (Some(backend), Some(auth_epoch)) = (context.backend.as_ref(), expected_auth_epoch) {
            // Compare-and-clear the captured lease. If a new login won the race, clear_epoch
            // rejects the stale epoch and preserves that newer account.
            let _ = backend.api.session().clear_epoch(auth_epoch);
        }
    }
    invalidate_auth_operations(context);
    invalidate_credit_sync_epoch(&mut context.store.borrow_mut());
    *context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner()) = None;
    clear_account_snapshot_state(app, context);
    clear_payment_account_state(app, context);
    clear_notification_account_state(app, context);
    clear_prompt_task_account_state(app);
    clear_prompt_optimization_account_state(app, context);
    clear_generation_account_state(app, context);
    close_agreement_window();
    let state = app.global::<AppState>();
    state.set_auth_busy(false);
    state.set_auth_code_busy(false);
    state.set_auth_wechat_busy(false);
    state.set_auth_countdown(0);
    state.set_auth_wechat_expires_in(0);
    state.set_auth_wechat_poll_elapsed_ms(0);
    state.set_agreement_update_busy(false);
    state.set_agreement_update_open(false);
    state.set_agreement_update_message("".into());
    state.set_credit_ledger_loading(false);
    state.set_logged_in(false);
    state.set_offline_mode(false);
    state.set_session_state("signed_out".into());
    state.set_ever_authenticated(false);
    state.set_offline_available(false);
    state.set_auth_open(true);
    state.set_auth_code("".into());
    state.set_auth_password("".into());
    clear_password_reset_state(&state);
    state.set_auth_wechat_login_id("".into());
    state.set_auth_wechat_qr_ready(false);
    state.set_auth_wechat_scanned(false);
    state.set_auth_wechat_busy(false);
    state.set_auth_wechat_status("".into());
    state.set_wechat_bound(false);
    state.set_wechat_can_unbind(false);
    state.set_wechat_bound_name("".into());
    state.set_wechat_bind_open(false);
    state.set_wechat_bind_busy(false);
    state.set_wechat_bind_login_id("".into());
    state.set_wechat_bind_qr_ready(false);
    state.set_wechat_bind_scanned(false);
    state.set_wechat_bind_status("".into());
    state.set_wechat_bind_expires_in(0);
    state.set_wechat_bind_poll_elapsed_ms(0);
    state.set_wechat_unbind_confirm_open(false);
    state.set_email_bound(false);
    state.set_email_bind_open(false);
    state.set_email_bind_email("".into());
    state.set_email_bind_code("".into());
    state.set_email_bind_code_busy(false);
    state.set_email_bind_busy(false);
    state.set_email_bind_countdown(0);
    state.set_email_bind_status("".into());
    state.set_invitation_code("".into());
    state.set_invitation_code_busy(false);
    state.set_invitation_code_success(false);
    state.set_invitation_code_submitted(false);
    state.set_invitation_code_status("".into());
    state.set_invitation_reward_rate("".into());
    state.set_invitation_count("".into());
    state.set_invitation_history_reward("".into());
    state.set_invitation_own_code("".into());
    state.set_invitation_rule_description("".into());
    state.set_invitation_rewards_status("".into());
    state.set_invitation_users(ModelRc::new(VecModel::from(Vec::<InvitedUserView>::new())));
    state.set_invitation_users_loading(false);
    state.set_invitation_users_has_more(false);
    state.set_invitation_users_next_cursor("".into());
    state.set_invitation_users_message("".into());
    state.set_auth_error(if revoked {
        "登录状态已失效，请重新登录".into()
    } else {
        "".into()
    });
    navigate_to(app, "welcome");
    state.set_profile_open(false);
    state.set_agreement_viewer_open(false);
    save_user_profile(app);
    if state.get_auth_method().as_str() == "wechat" {
        state.invoke_start_wechat_login();
    }
}

pub(super) fn require_online_operation(app: &AppWindow, operation: &str) -> bool {
    let state = app.global::<AppState>();
    if state.get_session_state().as_str() == "online" {
        return true;
    }
    if state.get_session_state().as_str() == "offline" {
        state.set_generation_status(
            format!("离线模式只能浏览本地内容，联网后才能{operation}").into(),
        );
    } else {
        state.set_generation_status(format!("请先登录后再{operation}").into());
        state.set_auth_open(true);
        if state.get_auth_method().as_str() == "wechat"
            && !state.get_auth_wechat_busy()
            && !state.get_auth_wechat_qr_ready()
        {
            state.invoke_start_wechat_login();
        }
    }
    false
}

fn valid_email(email: &str) -> bool {
    let mut parts = email.split('@');
    matches!((parts.next(), parts.next(), parts.next()), (Some(local), Some(domain), None) if !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_worker_is_not_mistaken_for_a_pending_result() {
        let (sender, receiver) = mpsc::channel::<u8>();
        drop(sender);
        let receiver = Rc::new(RefCell::new(Some(receiver)));

        assert_eq!(poll_receiver(&receiver), ReceiverPoll::Disconnected);
        assert_eq!(poll_receiver(&receiver), ReceiverPoll::Disconnected);
    }

    #[test]
    fn paid_actions_require_a_snapshot_for_the_exact_session_scope() {
        let context = AppContext::default();
        let scope_a = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 7,
        };
        let scope_b = SessionScope {
            owner_user_id: "user-b".to_string(),
            auth_epoch: 8,
        };

        assert!(!account_snapshot_scope_is_current(&context, &scope_a));
        *context
            .account_snapshot_scope
            .lock()
            .unwrap_or_else(|value| value.into_inner()) = Some(scope_a.clone());
        assert!(account_snapshot_scope_is_current(&context, &scope_a));
        assert!(!account_snapshot_scope_is_current(&context, &scope_b));
    }

    #[test]
    fn refreshed_qr_completion_cannot_install_the_old_attempt() {
        let context = AppContext::default();
        let old_attempt = begin_auth_operation(&context);
        let _new_attempt = begin_auth_operation(&context);
        let install_called = std::cell::Cell::new(false);

        let installed =
            install_login_if_current(auth_operation_is_current(&context, old_attempt), || {
                install_called.set(true);
                Ok(SessionScope {
                    owner_user_id: "old-wechat-user".to_string(),
                    auth_epoch: 1,
                })
            })
            .unwrap();

        assert!(installed.is_none());
        assert!(!install_called.get());
    }

    #[test]
    fn cancelled_email_attempt_cannot_install_after_the_dialog_closes() {
        let context = AppContext::default();
        let email_attempt = begin_auth_operation(&context);
        invalidate_auth_operations(&context);
        let install_called = std::cell::Cell::new(false);

        let installed =
            install_login_if_current(auth_operation_is_current(&context, email_attempt), || {
                install_called.set(true);
                Ok(SessionScope {
                    owner_user_id: "cancelled-email-user".to_string(),
                    auth_epoch: 1,
                })
            })
            .unwrap();

        assert!(installed.is_none());
        assert!(!install_called.get());
    }

    #[test]
    fn authentication_required_is_a_terminal_captured_session_outcome() {
        assert!(captured_session_error(&ApiError::AuthenticationRequired));
        assert!(!captured_session_error(&ApiError::Network {
            message: "offline".to_string(),
            timeout: false,
        }));
    }

    #[test]
    fn scoped_auth_outcomes_distinguish_current_terminal_and_stale_results() {
        assert_eq!(
            classify_scoped_auth_guards(false, true, false),
            ScopedAuthOutcome::Current
        );
        assert_eq!(
            classify_scoped_auth_guards(true, false, true),
            ScopedAuthOutcome::CapturedTerminal
        );
        assert_eq!(
            classify_scoped_auth_guards(true, false, false),
            ScopedAuthOutcome::Stale
        );
    }

    #[test]
    fn startup_refresh_success_followed_by_terminal_snapshot_ends_the_session() {
        let result = StartupAuthResult {
            auth_epoch: 7,
            credit_sync_epoch: 11,
            agreements: Ok(Vec::new()),
            refresh: Some(Ok("rotated-access".to_string())),
            snapshot: Some(Err(ApiError::Http {
                status: 401,
                code: "session_invalid".to_string(),
                message: "revoked during snapshot".to_string(),
                request_id: None,
                details: None,
            })),
        };

        assert!(startup_result_ended_captured_session(&result));
    }

    #[test]
    fn email_validation_rejects_incomplete_addresses() {
        assert!(valid_email("artist@example.com"));
        assert!(!valid_email("artist"));
        assert!(!valid_email("artist@example"));
        assert!(!valid_email("@example.com"));
    }

    #[test]
    fn membership_expiry_reminder_only_appears_near_expiry() {
        let near = (Local::now() + ChronoDuration::days(2)).to_rfc3339();
        let later = (Local::now() + ChronoDuration::days(10)).to_rfc3339();
        let expired = (Local::now() - ChronoDuration::minutes(1)).to_rfc3339();

        assert!(membership_expiry_message(&near).contains("到期"));
        assert!(membership_expiry_message(&later).is_empty());
        assert!(membership_expiry_message(&expired).contains("已到期"));
        assert!(membership_expiry_message("not-a-date").is_empty());
    }

    #[test]
    fn membership_expiry_is_displayed_in_local_time_without_utc_syntax() {
        let source = "2026-08-16T13:31:07.000Z";
        let expected = chrono::DateTime::parse_from_rfc3339(source)
            .unwrap()
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        let displayed = format_membership_ends_at(source);

        assert_eq!(displayed, expected);
        assert!(!displayed.contains('T'));
        assert!(!displayed.ends_with('Z'));
    }

    #[test]
    fn startup_network_failure_offers_offline_only_to_known_devices() {
        let network = ApiError::Network {
            message: "offline".to_string(),
            timeout: false,
        };
        assert_eq!(
            startup_error_disposition(&network, true),
            StartupErrorDisposition::OfferOffline
        );
        assert_eq!(
            startup_error_disposition(&network, false),
            StartupErrorDisposition::Recoverable
        );
    }

    #[test]
    fn startup_revocation_and_forced_update_never_enter_offline_mode() {
        let error = |code: &str| ApiError::Http {
            status: 401,
            code: code.to_string(),
            message: "test".to_string(),
            request_id: None,
            details: None,
        };
        assert_eq!(
            startup_error_disposition(&error("refresh_token_reused"), true),
            StartupErrorDisposition::TerminalSession
        );
        assert_eq!(
            startup_error_disposition(&error("client_update_required"), true),
            StartupErrorDisposition::UpdateRequired
        );
    }

    #[test]
    fn auth_error_messages_hide_request_ids() {
        let error = ApiError::Http {
            status: 400,
            code: "email_code_invalid".to_string(),
            message: "invalid code".to_string(),
            request_id: Some("94ab68af-e2b5-4a99-877b-b572edbd0e1c".to_string()),
            details: None,
        };
        let message = auth_error_message(&error);
        assert_eq!(message, "验证码不正确或已失效");
        assert!(!message.contains("请求号"));
        assert!(!message.contains("94ab68af"));
        assert!(!message.contains("email_code_invalid"));
    }

    fn credit_pack(payable_price_cents: Option<&str>) -> CreditPack {
        CreditPack {
            code: "pack_1000".to_string(),
            name: "1000 积分".to_string(),
            price_cents: "1000".to_string(),
            payable_price_cents: payable_price_cents.map(ToString::to_string),
            discount_amount_cents: payable_price_cents.map(|_| "50".to_string()),
            recharge_discount_bps: payable_price_cents.map(|_| 9500),
            credits: "1000".to_string(),
        }
    }

    #[test]
    fn credit_pack_price_prefers_membership_discount_quote() {
        let discounted = credit_pack(Some("950"));
        assert_eq!(format_cents(credit_pack_price_cents(&discounted)), "¥ 9.50");
        assert_eq!(credit_pack_note(&discounted), "会员 95 折 · 已优惠 ¥ 0.50");

        let original = credit_pack(None);
        assert_eq!(format_cents(credit_pack_price_cents(&original)), "¥ 10.00");
    }

    #[test]
    fn image_model_prices_follow_the_selected_catalog_model() {
        let model = ModelCatalogItem {
            code: "nano_banana".to_string(),
            version: 1,
            purpose: "image_generation".to_string(),
            name: "nano-banana".to_string(),
            capabilities: serde_json::json!({}),
            prices: vec![
                ModelPrice {
                    quality: "1K".to_string(),
                    max_long_edge: Some(1024),
                    credit_cost: "35".to_string(),
                },
                ModelPrice {
                    quality: "2K".to_string(),
                    max_long_edge: Some(2048),
                    credit_cost: "45".to_string(),
                },
                ModelPrice {
                    quality: "4K".to_string(),
                    max_long_edge: Some(4096),
                    credit_cost: "60".to_string(),
                },
            ],
        };

        assert_eq!(model_price(&model, "1K"), 35);
        assert_eq!(model_price(&model, "2K"), 45);
        assert_eq!(model_price(&model, "4K"), 60);
    }

    #[test]
    fn nano_banana_uses_the_versioned_client_display_name() {
        let model = ModelCatalogItem {
            code: "nano_banana".to_string(),
            version: 1,
            purpose: "image_generation".to_string(),
            name: "nano-banana".to_string(),
            capabilities: serde_json::json!({}),
            prices: vec![],
        };

        assert_eq!(model_display_name(&model), "nano-banana-2");
        assert_eq!(model.code, "nano_banana");

        let pro = ModelCatalogItem {
            code: "nano_banana_pro".to_string(),
            name: "nano-banana-pro".to_string(),
            ..model.clone()
        };
        let fast = ModelCatalogItem {
            code: "nano_banana_fast".to_string(),
            name: "nano-banana".to_string(),
            ..model
        };
        assert_eq!(model_display_name(&pro), "nano-banana-pro");
        assert_eq!(model_display_name(&fast), "nano-banana");
    }
}
