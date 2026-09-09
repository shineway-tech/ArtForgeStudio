fn project_team_registration(state: &AppState, pending: &PendingRegistrationOutcome) {
    state.set_team_registration_open(true);
    state.set_team_registration_busy(false);
    state.set_team_registration_error("".into());
    state.set_team_registration_expires_at(pending.continuation_expires_at.to_rfc3339().into());
    state.set_team_registration_selection_state(match pending.selection_state { TeamRegistrationSelectionState::Unique => "unique", TeamRegistrationSelectionState::Multiple => "multiple" }.into());
    state.set_team_registration_invitations(ModelRc::new(VecModel::from(pending.invitations.iter().map(|invitation| TeamRegistrationInvitationView {
        invitation_id: invitation.invitation_id.clone().into(), group_id: invitation.group_id.clone().into(),
        team_name: invitation.team_name.clone().into(), owner_display_name: invitation.owner_display_name.clone().into(),
        recipient_email_masked: invitation.recipient_email_masked.clone().into(), monthly_limit: invitation.monthly_limit.clone().into(),
        status: invitation.status.clone().into(), expires_at: invitation.expires_at.clone().into(), version: invitation.version.clone().into(),
    }).collect::<Vec<_>>())));
}
fn clear_team_registration(state: &AppState) {
    state.set_team_registration_open(false); state.set_team_registration_busy(false);
    state.set_team_registration_password("".into()); state.set_team_registration_password_confirmation("".into());
    state.set_team_registration_error("".into()); state.set_team_registration_expires_at("".into());
    state.set_team_registration_selection_state("".into());
    state.set_team_registration_invitations(ModelRc::new(VecModel::default()));
}
fn registration_terminal(error: &ApiError) -> bool {
    matches!(error.code(), Some("team_registration_invitation_unavailable" | "team_registration_continuation_invalid"
        | "team_registration_continuation_expired" | "team_registration_device_mismatch" | "registration_already_completed"))
}
struct CompletedTeamRegistration {
    user: LoginUser, tokens: TokenSet, origin: LoginOrigin,
    suggestion: Option<String>, multiple: bool,
}
fn complete_pending_registration(api: &AuthApi, pending: &PendingRegistrationOutcome, cancelled: &std::sync::atomic::AtomicBool) -> Result<CompletedTeamRegistration, ApiError> {
    if cancelled.load(Ordering::SeqCst) { return Err(transition_error("团队注册已取消")); }
    let password = pending.password.as_ref().ok_or_else(|| transition_error("尚未设置密码"))?;
    let result = api.complete_team_registration(&pending.registration_continuation, password.expose(), &pending.agreement_acceptances, &pending.idempotency_key)?;
    if cancelled.load(Ordering::SeqCst) { return Err(transition_error("团队注册已取消")); }
    let (user, tokens, origin) = match result.session {
        TeamRegistrationSessionResult::Authenticated { tokens } => (result.user, tokens, LoginOrigin::TeamInviteFresh),
        TeamRegistrationSessionResult::LoginRequired { session_login_required: true } => {
            let login = api.password_login_response(pending.normalized_email.expose(), password.expose(), &pending.agreement_acceptances)?;
            if login.user.id != result.user.id { return Err(transition_error("团队注册重放身份不一致")); }
            (login.user, login.tokens, LoginOrigin::TeamInviteReplay)
        }
        TeamRegistrationSessionResult::LoginRequired { session_login_required: false } => return Err(transition_error("团队注册重放状态无效")),
    };
    Ok(CompletedTeamRegistration { user, tokens, origin, suggestion: result.suggested_account_group_id, multiple: result.selection_state == TeamRegistrationSelectionState::Multiple })
}
struct RegistrationJob {
    receiver: mpsc::Receiver<(PendingRegistrationOutcome, Result<CompletedTeamRegistration, ApiError>)>,
    worker: Option<std::thread::JoinHandle<()>>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for RegistrationJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}
fn wire_team_registration_callbacks(app: &AppWindow, context: AppContext, pending: Rc<RefCell<Option<PendingRegistrationOutcome>>>) {
    let state = app.global::<AppState>();
    watch_idle_team_registration(app.as_weak(), context.clone(), pending.clone());
    { let weak = app.as_weak(); let context = context.clone(); let pending = pending.clone();
      state.on_cancel_team_registration(move || {
        pending.borrow_mut().take(); invalidate_auth_operations(&context);
        if let Some(app) = weak.upgrade() { clear_team_registration(&app.global::<AppState>()); app.global::<AppState>().set_auth_busy(false); app.global::<AppState>().set_session_state("signed_out".into()); }
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pending = pending.clone();
      state.on_complete_team_registration(move || {
        let Some(app) = weak.upgrade() else { return; }; let state = app.global::<AppState>();
        if state.get_team_registration_busy() || !state.get_team_registration_open() { return; }
        let Some(mut request) = pending.borrow_mut().take() else { return; };
        if !auth_operation_is_current(&context, request.auth_operation_epoch) { clear_team_registration(&state); return; }
        if request.continuation_expires_at <= chrono::Utc::now() {
            state.set_auth_email(request.normalized_email.expose().into());
            clear_team_registration(&state); state.set_auth_email_mode("code".into()); state.set_auth_code("".into());
            state.set_session_state("signed_out".into()); state.set_auth_error("验证已过期，请重新获取邮箱验证码".into()); return;
        }
        if request.password.is_none() {
            let password = state.get_team_registration_password().to_string();
            let confirmation = state.get_team_registration_password_confirmation().to_string();
            if let Err(message) = validate_team_registration_password(&password, &confirmation) {
                state.set_team_registration_error(message.into()); *pending.borrow_mut() = Some(request); return;
            }
            request.password = Some(SecretString::new(password));
        }
        state.set_team_registration_password("".into()); state.set_team_registration_password_confirmation("".into());
        let Some(backend) = context.backend.as_ref() else { *pending.borrow_mut() = Some(request); return; };
        let api = AuthApi::new(backend.api.clone());
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (sender, receiver) = mpsc::channel();
        let epoch = request.auth_operation_epoch;
        let worker = std::thread::Builder::new().name("team-registration".into()).spawn(move || {
            let result = complete_pending_registration(&api, &request, &worker_cancelled);
            let _ = sender.send((request, result));
        });
        match worker {
            Ok(worker) => {
                state.set_team_registration_busy(true);
                let job = Rc::new(RefCell::new(RegistrationJob { receiver, worker: Some(worker), cancelled }));
                poll_team_registration(app.as_weak(), context.clone(), pending.clone(), epoch, job);
            }
            Err(_) => { clear_team_registration(&state); state.set_auth_error("注册任务无法启动，请重新验证邮箱".into()); }
        }
      }); }
}
fn validate_team_registration_password(password: &str, confirmation: &str) -> Result<(), &'static str> {
    if !(8..=20).contains(&password.chars().count()) || !password.bytes().any(|byte| byte.is_ascii_uppercase())
        || !password.bytes().any(|byte| byte.is_ascii_lowercase()) || !password.bytes().any(|byte| byte.is_ascii_digit()) {
        return Err("密码应为 8–20 个字符，并包含大小写字母和数字");
    }
    if password != confirmation { return Err("两次输入的密码不一致"); }
    Ok(())
}
fn watch_idle_team_registration(weak: Weak<AppWindow>, context: AppContext, pending: Rc<RefCell<Option<PendingRegistrationOutcome>>>) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let Some(app) = weak.upgrade() else { pending.borrow_mut().take(); return; };
        let state = app.global::<AppState>();
        let invalid = pending.borrow().as_ref().is_some_and(|request| {
            !state.get_auth_open() || !state.get_team_registration_open()
                || state.get_auth_method().as_str() != "email" || state.get_auth_email_mode().as_str() != "code"
                || !auth_operation_is_current(&context, request.auth_operation_epoch)
                || context.backend.as_ref().is_some_and(|backend| backend.api.upgrade_latch().is_tripped())
        });
        if invalid {
            pending.borrow_mut().take();
            invalidate_auth_operations(&context);
            clear_team_registration(&state);
        }
        watch_idle_team_registration(weak, context, pending);
    });
}
fn poll_team_registration(weak: Weak<AppWindow>, context: AppContext, pending: Rc<RefCell<Option<PendingRegistrationOutcome>>>, epoch: u64, job: Rc<RefCell<RegistrationJob>>) {
    slint::Timer::single_shot(Duration::from_millis(40), move || {
        let Some(app) = weak.upgrade() else { return; }; let state = app.global::<AppState>();
        if !auth_operation_is_current(&context, epoch) || !state.get_team_registration_open()
            || state.get_auth_method().as_str() != "email" || state.get_auth_email_mode().as_str() != "code" {
            pending.borrow_mut().take(); clear_team_registration(&state); return;
        }
        let result = job.borrow().receiver.try_recv();
        match result {
            Err(TryRecvError::Empty) => poll_team_registration(weak, context, pending, epoch, job),
            Err(TryRecvError::Disconnected) => { clear_team_registration(&state); state.set_auth_error("注册任务已中断，请重新验证邮箱".into()); }
            Ok((request, result)) => {
                if let Some(worker) = job.borrow_mut().worker.take() { let _ = worker.join(); }
                state.set_team_registration_busy(false);
                match result {
                    Ok(completed) => {
                        clear_team_registration(&state);
                        if let Some(coordinator) = context.account_transition.clone() {
                            coordinator.activate_identity(&app, context.clone(), completed.user, completed.tokens, completed.origin, completed.suggestion);
                            if completed.multiple {
                                state.set_account_center_section("accounts-teams".into()); state.set_profile_open(true); state.set_team_tab("pending".into());
                            }
                        }
                    }
                    Err(error) if registration_terminal(&error) => {
                        state.set_auth_email(request.normalized_email.expose().into());
                        clear_team_registration(&state); state.set_auth_email_mode("code".into()); state.set_auth_code("".into());
                        state.set_auth_password("".into()); state.set_auth_open(true); state.set_session_state("signed_out".into());
                        state.set_auth_error(error.user_message().into());
                    }
                    Err(error) => {
                        if let Some(required) = RequiredUpgrade::from_error(&error) {
                            clear_team_registration(&state); show_required_update_prompt(&app, required.minimum_version.as_deref().unwrap_or_default());
                        } else {
                            state.set_team_registration_error(format!("{}；重试将使用相同注册请求与密码", error.user_message()).into());
                            *pending.borrow_mut() = Some(request);
                        }
                    }
                }
            }
        }
    });
}
