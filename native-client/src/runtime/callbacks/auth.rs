use super::*;

struct StartupAuthResult {
    agreements: std::result::Result<Vec<AgreementItem>, ApiError>,
    refresh: Option<std::result::Result<String, ApiError>>,
    snapshot: Option<std::result::Result<BackendSnapshot, ApiError>>,
}

type LoginResult = std::result::Result<
    (LoginResponse, std::result::Result<BackendSnapshot, ApiError>),
    ApiError,
>;

enum WechatPollOutcome {
    Pending,
    Scanned(String),
    AgreementRequired(String),
    Failed(String),
    Completed(LoginResponse, std::result::Result<BackendSnapshot, ApiError>),
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
            if state.get_auth_busy() || state.get_auth_code_busy() || state.get_auth_countdown() > 0 {
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
                                format!("验证码已发送至 {}，{} 秒内有效", response.email_masked, response.expires_in_seconds).into(),
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
            let Some(app) = app_weak.upgrade() else { return; };
            begin_wechat_login(&app, context.clone(), backend.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        state.on_revoke_session(move |session_id| {
            let Some(app) = app_weak.upgrade() else { return; };
            let session_id = session_id.to_string();
            if session_id.trim().is_empty() { return; }
            let state = app.global::<AppState>();
            state.set_account_sessions(ModelRc::new(VecModel::from(
                state.get_account_sessions().iter()
                    .filter(|session| session.id.as_str() != session_id)
                    .collect::<Vec<_>>(),
            )));
            let api = AccountApi::new(backend.api.clone());
            let weak = app.as_weak();
            std::thread::spawn(move || {
                let result = api.revoke_session(&session_id);
                let _ = weak.upgrade_in_event_loop(move |app| {
                    match result {
                        Ok(()) => app.global::<AppState>().set_generation_status("设备会话已撤销".into()),
                        Err(error) => app.global::<AppState>().set_generation_status(
                            format!("撤销设备失败：{}", error.user_message()).into(),
                        ),
                    }
                });
            });
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
            let code = state.get_auth_code().trim().to_string();
            if !valid_email(&email) {
                state.set_auth_error("请输入正确的邮箱地址".into());
                return;
            }
            if code.len() != 6 || !code.chars().all(|value| value.is_ascii_digit()) {
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
            state.set_auth_busy(true);
            state.set_session_state("authenticating".into());
            state.set_auth_error("".into());
            let api = AuthApi::new(backend.api.clone());
            let account_api = AccountApi::new(backend.api.clone());
            let context = context.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let result = api.login(&email, &code, &acceptances).map(|login| {
                    let snapshot = account_api.snapshot();
                    (login, snapshot)
                });
                let _ = sender.send(result);
            });
            poll_login_result(
                app.as_weak(),
                context,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_enter_offline(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_offline_available() {
                return;
            }
            state.set_logged_in(true);
            state.set_offline_mode(true);
            state.set_session_state("offline".into());
            state.set_auth_open(false);
            state.set_auth_error("".into());
            navigate_to(&app, "assets");
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_open_agreement(move |title, url| {
            let Some(app) = app_weak.upgrade() else { return; };
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
        state.on_accept_current_agreements(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            if state.get_agreement_update_busy() { return; }
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
            state.set_agreement_update_busy(true);
            state.set_agreement_update_message("".into());
            let api = AuthApi::new(backend.api.clone());
            let weak = app.as_weak();
            std::thread::spawn(move || {
                let result = api.accept_agreements(&acceptances);
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<AppState>();
                    state.set_agreement_update_busy(false);
                    match result {
                        Ok(()) => {
                            state.set_accepted_user_terms_version(state.get_auth_user_terms_version());
                            state.set_accepted_privacy_version(state.get_auth_privacy_version());
                            state.set_agreement_update_open(false);
                            state.set_agreement_update_message("".into());
                            save_user_profile(&app);
                        }
                        Err(error) => state.set_agreement_update_message(auth_error_message(&error).into()),
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        state.on_logout(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            sign_out_locally(&app, false);
            let api = AuthApi::new(backend.api.clone());
            let backend = backend.clone();
            std::thread::spawn(move || {
                let _ = api.logout(false);
                let _ = backend.api.session().clear();
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = backend.clone();
        state.on_logout_all(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            sign_out_locally(&app, false);
            let api = AuthApi::new(backend.api.clone());
            let backend = backend.clone();
            std::thread::spawn(move || {
                let _ = api.logout(true);
                let _ = backend.api.session().clear();
            });
        });
    }
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
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let agreements = api.list_agreements();
        let refresh = match backend.api.session().has_refresh_token() {
            Ok(true) => Some(api.refresh()),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        };
        let snapshot = if matches!(refresh, Some(Ok(_))) {
            Some(account_api.snapshot())
        } else {
            None
        };
        let result = StartupAuthResult { agreements, refresh, snapshot };
        let _ = sender.send(result);
    });
    poll_startup_auth_result(
        app.as_weak(),
        context.clone(),
        Rc::new(RefCell::new(Some(receiver))),
    );
    schedule_network_recovery(app.as_weak(), context);
}

fn schedule_network_recovery(app_weak: Weak<AppWindow>, context: AppContext) {
    slint::Timer::single_shot(Duration::from_secs(8), move || {
        let Some(app) = app_weak.upgrade() else { return; };
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
    let Some(backend) = context.backend.clone() else { return; };
    let Ok(true) = backend.api.session().has_refresh_token() else { return; };
    state.set_auth_busy(true);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let auth = AuthApi::new(backend.api.clone());
        let result = auth.refresh().and_then(|_| {
            let snapshot = AccountApi::new(backend.api.clone()).snapshot()?;
            let agreements = auth.list_agreements()?;
            Ok((snapshot, agreements))
        });
        let _ = sender.send(result);
    });
    poll_network_recovery(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_network_recovery(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<(BackendSnapshot, Vec<AgreementItem>), ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = poll_receiver(&receiver);
        let Some(result) = result else {
            poll_network_recovery(app_weak, context, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        state.set_auth_busy(false);
        match result {
            Ok((snapshot, agreements)) => {
                apply_agreements(&app, &agreements);
                apply_backend_snapshot(&app, &context, snapshot);
                state.set_logged_in(true);
                state.set_offline_mode(false);
                state.set_session_state("online".into());
                state.set_auth_open(false);
                state.set_auth_error("".into());
                state.set_generation_status("网络已恢复，账号数据已同步".into());
                require_updated_agreements(&app);
                recover_pending_generations(&app, context.clone());
                recover_pending_orders(&app, context.clone());
                refresh_server_notifications(&app, context);
            }
            Err(error) if error.is_terminal_session_error() => sign_out_locally(&app, true),
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

fn selected_login_agreement_acceptances(state: &AppState) -> Vec<AgreementAcceptance> {
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

pub(super) fn begin_wechat_login(app: &AppWindow, context: AppContext, backend: Arc<BackendRuntime>) {
    let state = app.global::<AppState>();
    if state.get_auth_wechat_busy() || state.get_auth_busy() {
        return;
    }
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
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_wechat_start_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<WechatLoginStartResponse, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = poll_receiver(&receiver);
        let Some(result) = result else {
            poll_wechat_start_result(app_weak, context, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        state.set_auth_wechat_busy(false);
        if !state.get_auth_open() || state.get_auth_method().as_str() != "wechat" {
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
                    let poll_after_ms = response.poll_after_milliseconds
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
    login_id: String,
    delay_milliseconds: u64,
) {
    slint::Timer::single_shot(Duration::from_millis(delay_milliseconds.max(250)), move || {
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if !state.get_auth_open()
            || state.get_auth_method().as_str() != "wechat"
            || state.get_auth_wechat_login_id().as_str() != login_id
        {
            return;
        }
        let Some(backend) = context.backend.clone() else { return; };
        let api = AuthApi::new(backend.api.clone());
        let account_api = AccountApi::new(backend.api.clone());
        let request_login_id = login_id.clone();
        let acceptances = selected_login_agreement_acceptances(&state);
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = api.wechat_login_status(&request_login_id, &acceptances).and_then(|status| {
                match (status.status.as_str(), status.qr_status.as_deref()) {
                    ("pending", Some("scanned")) | ("scanned", _) => Ok(WechatPollOutcome::Scanned(
                        status.message.unwrap_or_else(|| "已扫码，请在手机微信中确认登录".to_string()),
                    )),
                    ("pending", _) => Ok(WechatPollOutcome::Pending),
                    ("agreement_required", _) => Ok(WechatPollOutcome::AgreementRequired(
                        status.message.unwrap_or_else(|| "请先阅读并同意用户协议和隐私政策".to_string()),
                    )),
                    ("failed", _) => Ok(WechatPollOutcome::Failed(
                        status.message.unwrap_or_else(|| "微信登录未完成，请刷新二维码重试".to_string()),
                    )),
                    ("completed", _) => {
                        let login = status.login.ok_or_else(|| ApiError::Protocol {
                            message: "微信登录响应缺少登录信息".to_string(),
                            request_id: None,
                        })?;
                        Ok(WechatPollOutcome::Completed(login, account_api.snapshot()))
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
            login_id,
            Rc::new(RefCell::new(Some(receiver))),
        );
    });
}

fn poll_wechat_status_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    login_id: String,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<WechatPollOutcome, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = poll_receiver(&receiver);
        let Some(result) = result else {
            poll_wechat_status_result(app_weak, context, login_id, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if state.get_auth_wechat_login_id().as_str() != login_id {
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
            Ok(WechatPollOutcome::Completed(response, snapshot)) => {
                state.set_auth_wechat_login_id("".into());
                state.set_auth_wechat_qr_ready(false);
                state.set_auth_wechat_scanned(false);
                state.set_auth_wechat_status("登录成功".into());
                finish_login(&app, &context, response, snapshot);
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
    receiver: Rc<RefCell<Option<mpsc::Receiver<LoginResult>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = poll_receiver(&receiver);
        let Some(result) = result else {
            poll_login_result(app_weak, context, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_auth_busy(false);
        match result {
            Ok((response, snapshot)) => finish_login(&app, &context, response, snapshot),
            Err(error) => {
                state.set_session_state("signed_out".into());
                apply_auth_error(&app, error);
            }
        }
    });
}

fn finish_login(
    app: &AppWindow,
    context: &AppContext,
    response: LoginResponse,
    snapshot: std::result::Result<BackendSnapshot, ApiError>,
) {
    let state = app.global::<AppState>();
    state.set_logged_in(true);
    state.set_offline_mode(false);
    state.set_session_state("online".into());
    state.set_ever_authenticated(true);
    state.set_offline_available(true);
    state.set_email_mask(response.user.email_masked.into());
    state.set_nickname(response.user.nickname.unwrap_or_default().into());
    state.set_auth_code("".into());
    state.set_auth_error("".into());
    state.set_auth_open(false);
    if state.get_auth_user_terms_accepted() {
        state.set_accepted_user_terms_version(state.get_auth_user_terms_version());
    }
    if state.get_auth_privacy_accepted() {
        state.set_accepted_privacy_version(state.get_auth_privacy_version());
    }
    save_user_profile(app);
    match snapshot {
        Ok(snapshot) => apply_backend_snapshot(app, context, snapshot),
        Err(error) => state.set_generation_status(
            format!("账号数据同步失败：{}", auth_error_message(&error)).into(),
        ),
    }
    recover_pending_generations(app, context.clone());
    recover_pending_orders(app, context.clone());
    navigate_to(app, "generation");
}

fn poll_startup_auth_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<RefCell<Option<mpsc::Receiver<StartupAuthResult>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = poll_receiver(&receiver);
        let Some(result) = result else {
            poll_startup_auth_result(app_weak, context, receiver);
            return;
        };
        if let Some(app) = app_weak.upgrade() {
            apply_startup_auth(&app, &context, result);
        }
    });
}

fn poll_receiver<T>(receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>) -> Option<T> {
    let mut slot = receiver.borrow_mut();
    let receiver = slot.as_ref()?;
    match receiver.try_recv() {
        Ok(result) => {
            slot.take();
            Some(result)
        }
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            slot.take();
            None
        }
    }
}

fn apply_startup_auth(app: &AppWindow, context: &AppContext, result: StartupAuthResult) {
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
                    Ok(snapshot) => apply_backend_snapshot(app, context, snapshot),
                    Err(error) => state.set_generation_status(
                        format!("账号数据同步失败：{}", auth_error_message(&error)).into(),
                    ),
                }
            }
            recover_pending_generations(app, context.clone());
            recover_pending_orders(app, context.clone());
            require_updated_agreements(app);
            navigate_to(app, "generation");
        }
        Some(Err(error)) => {
            match startup_error_disposition(&error, state.get_offline_available()) {
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
                    sign_out_locally(app, true);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupErrorDisposition {
    UpdateRequired,
    OfferOffline,
    TerminalSession,
    Recoverable,
}

fn startup_error_disposition(
    error: &ApiError,
    offline_available: bool,
) -> StartupErrorDisposition {
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

pub(super) fn refresh_backend_snapshot(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else { return; };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(AccountApi::new(backend.api.clone()).snapshot());
    });
    poll_backend_snapshot(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_backend_snapshot(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<BackendSnapshot, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = poll_receiver(&receiver);
        let Some(result) = result else {
            poll_backend_snapshot(app_weak, context, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        match result {
            Ok(snapshot) => apply_backend_snapshot(&app, &context, snapshot),
            Err(error) if error.is_terminal_session_error() => sign_out_locally(&app, true),
            Err(error) => app.global::<AppState>().set_generation_status(
                format!("账号数据刷新失败：{}", auth_error_message(&error)).into(),
            ),
        }
    });
}

pub(super) fn apply_backend_snapshot(app: &AppWindow, context: &AppContext, snapshot: BackendSnapshot) {
    let state = app.global::<AppState>();
    state.set_email_mask(snapshot.account.user.email_masked.clone().into());
    state.set_nickname(snapshot.account.user.nickname.clone().unwrap_or_default().into());
    state.set_email_bound(snapshot.account.auth_methods.email.bound);
    state.set_wechat_bound(snapshot.account.auth_methods.wechat.bound);
    state.set_wechat_can_unbind(snapshot.account.auth_methods.wechat.can_unbind);
    state.set_wechat_bound_name(
        snapshot.account.auth_methods.wechat.nickname.clone().unwrap_or_default().into(),
    );
    if let Some(plan) = snapshot.account.membership.plan.as_ref() {
        state.set_membership_plan_code(plan.code.clone().into());
        state.set_membership_plan_name(plan.name.clone().into());
        state.set_membership_max_quality(plan.max_quality.clone().into());
        state.set_membership_tier_rank(plan.tier_rank);
    }
    let membership_ends_at = snapshot.account.membership.ends_at.clone().unwrap_or_default();
    state.set_membership_ends_at(format_membership_ends_at(&membership_ends_at).into());
    state.set_membership_expiry_message(membership_expiry_message(&membership_ends_at).into());
    if let Some(credits) = snapshot.account.credits.as_ref() {
        state.set_credit_balance(credits.available.clone().into());
        state.set_credit_reserved(credits.reserved.clone().into());
    }
    let packs = snapshot.packs.iter().map(|pack| CreditPackView {
        code: pack.code.clone().into(),
        name: pack.name.clone().into(),
        credits: pack.credits.clone().into(),
        price: format_cents(credit_pack_price_cents(pack)).into(),
        price_cents: credit_pack_price_cents(pack).into(),
        note: credit_pack_note(pack).into(),
    }).collect::<Vec<_>>();
    let selected_code = state.get_selected_credit_pack_code().to_string();
    if let Some(selected) = snapshot.packs.iter().find(|pack| pack.code == selected_code)
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
        snapshot.plans.iter().map(|plan| MembershipPlanView {
            code: plan.code.clone().into(),
            name: plan.name.clone().into(),
            price: format_cents(&plan.price_cents).into(),
            grant_credits: plan.grant_credits.clone().into(),
            period_days: plan.period_days,
            tier_rank: plan.tier_rank,
            max_quality: plan.max_quality.clone().into(),
        }).collect::<Vec<_>>(),
    )));
    state.set_catalog_models(ModelRc::new(VecModel::from(
        snapshot.models.iter().map(|model| CatalogModelView {
            code: model.code.clone().into(),
            name: model.name.clone().into(),
            purpose: model.purpose.clone().into(),
            version: model.version.min(i32::MAX as u32) as i32,
            capabilities: model_capabilities_text(model).into(),
            pricing: model.prices.iter().map(|price| {
                match price.max_long_edge {
                    Some(edge) => format!("{}：{} 积分（最长边 {}）", price.quality, price.credit_cost, edge),
                    None => format!("{}：{} 积分", price.quality, price.credit_cost),
                }
            }).collect::<Vec<_>>().join(" · ").into(),
        }).collect::<Vec<_>>(),
    )));
    reset_credit_ledger(
        app,
        &context.store,
        &snapshot.ledger,
        snapshot.ledger_next_cursor.clone(),
    );
    state.set_account_sessions(ModelRc::new(VecModel::from(
        snapshot.sessions.iter().map(|session| AccountSession {
            id: session.id.clone().into(),
            device_name: session.device_name.clone().into(),
            platform: session.platform.clone().into(),
            app_version: session.app_version.clone().into(),
            last_seen_at: session.last_seen_at.clone().into(),
            is_current: session.is_current,
        }).collect::<Vec<_>>(),
    )));

    let image_models = snapshot
        .models
        .iter()
        .filter(|item| item.purpose == "image_generation")
        .map(|item| ModelOptionData {
            code: item.code.clone(),
            name: item.name.clone(),
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
    let mut model_groups = Vec::new();
    if !image_models.is_empty() {
        model_groups.push(model_group("image", "平台图像模型", image_models.clone()));
    }
    if !prompt_models.is_empty() {
        model_groups.push(model_group(
            "reasoning",
            "平台提示词模型",
            prompt_models.clone(),
        ));
    }
    {
        let mut store = context.store.borrow_mut();
        store.model_groups = model_groups;
        push_model_groups(app, &store);
    }
    if let Some(model) = image_models.first() {
        state.set_image_model(model.code.clone().into());
        state.set_image_model_name(model.name.clone().into());
        if let Some(catalog_model) = snapshot.models.iter().find(|item| item.code == model.code) {
            for price in &catalog_model.prices {
                let value = decimal_to_i32(&price.credit_cost);
                match price.quality.as_str() {
                    "1K" => state.set_image_price_1k(value),
                    "2K" => state.set_image_price_2k(value),
                    "4K" => state.set_image_price_4k(value),
                    _ => {}
                }
            }
        }
    }
    if let Some(model) = prompt_models.first() {
        state.set_reasoning_model(model.code.clone().into());
        state.set_reasoning_model_name(model.name.clone().into());
    }
    save_user_profile(app);
}

fn model_group(kind: &str, name: &str, models: Vec<ModelOptionData>) -> ModelGroupData {
    let model_codes = models.iter().map(|model| model.code.clone()).collect::<Vec<_>>();
    ModelGroupData {
        kind: kind.to_string(),
        name: name.to_string(),
        selected_model: model_codes.first().cloned().unwrap_or_default(),
        used_models: model_codes,
        models,
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
    let (sign, digits) = value.strip_prefix('-').map(|digits| ("-", digits)).unwrap_or(("", value));
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return format!("¥ {value}");
    }
    let normalized = digits.trim_start_matches('0');
    let normalized = if normalized.is_empty() { "0" } else { normalized };
    let padded = format!("{:0>3}", normalized);
    let split = padded.len() - 2;
    format!("¥ {sign}{}.{}", &padded[..split], &padded[split..])
}

fn credit_pack_price_cents(pack: &CreditPack) -> &str {
    pack.payable_price_cents.as_deref().unwrap_or(&pack.price_cents)
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
    if let Some(ratios) = model.capabilities.get("aspect_ratios").and_then(Value::as_array) {
        let values = ratios
            .iter()
            .filter_map(Value::as_str)
            .map(client_ratio_from_api)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            parts.push(format!("比例：{}", values.join("/")));
        }
    }
    if model.capabilities.get("supports_references").and_then(Value::as_bool) == Some(true) {
        parts.push("支持参考图".to_string());
    }
    if let Some(operations) = model.capabilities.get("operations").and_then(Value::as_array) {
        let values = operations.iter().filter_map(Value::as_str).map(|operation| match operation {
            "optimize" => "提示词优化",
            "translate" => "提示词翻译",
            value => value,
        }).collect::<Vec<_>>();
        if !values.is_empty() {
            parts.push(values.join("/"));
        }
    }
    if parts.is_empty() { "服务端模型能力".to_string() } else { parts.join(" · ") }
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

fn update_required_message(error: &ApiError) -> String {
    format!(
        "当前客户端版本过旧，在线功能要求至少升级到 {}",
        minimum_version_from_error(error)
    )
}

fn minimum_version_from_error(error: &ApiError) -> &str {
    match error {
        ApiError::Http { details: Some(details), .. } => details
            .get("minimum_version")
            .and_then(Value::as_str)
            .unwrap_or("最新版本"),
        _ => "最新版本",
    }
}

fn auth_error_message(error: &ApiError) -> String {
    error.user_message()
}

fn sign_out_locally(app: &AppWindow, revoked: bool) {
    close_agreement_window();
    let state = app.global::<AppState>();
    state.set_logged_in(false);
    state.set_offline_mode(false);
    state.set_session_state("signed_out".into());
    state.set_ever_authenticated(false);
    state.set_offline_available(false);
    state.set_auth_open(true);
    state.set_auth_code("".into());
    state.set_auth_wechat_login_id("".into());
    state.set_auth_wechat_qr_ready(false);
    state.set_auth_wechat_scanned(false);
    state.set_auth_wechat_busy(false);
    state.set_auth_wechat_status("".into());
    state.set_wechat_bound(false);
    state.set_wechat_can_unbind(false);
    state.set_wechat_bound_name("".into());
    state.set_wechat_bind_open(false);
    state.set_wechat_bind_login_id("".into());
    state.set_wechat_bind_qr_ready(false);
    state.set_wechat_bind_scanned(false);
    state.set_wechat_unbind_confirm_open(false);
    state.set_email_bound(false);
    state.set_email_bind_open(false);
    state.set_email_bind_email("".into());
    state.set_email_bind_code("".into());
    state.set_email_bind_code_busy(false);
    state.set_email_bind_busy(false);
    state.set_email_bind_countdown(0);
    state.set_email_bind_status("".into());
    state.set_auth_error(if revoked { "登录状态已失效，请重新登录".into() } else { "".into() });
    state.set_page("welcome".into());
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
        state.set_generation_status(format!("离线模式只能浏览本地内容，联网后才能{operation}").into());
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
}
