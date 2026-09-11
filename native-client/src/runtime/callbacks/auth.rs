use super::*;

pub(super) struct PreparedBackendProjection {
    pub(super) ui:PreparedUiProjection,
    pub(super) model_groups:Vec<ModelGroupData>,
    pub(super) pagination:CreditLedgerPagination,
    pub(super) credit_version:Option<String>,
}
pub(super) fn prepare_activation_backend_projection(
    snapshot:&BackendSnapshot,preferred_image:&str,preferred_prompt:&str,preferred_video:&str,preferred_pack:&str,
) -> PreparedBackendProjection {
    let mut ui=PreparedUiProjection::default();
    ui.push(false,|state,value|state.set_credit_ledger_loading(value));
    let projection = project_backend_snapshot(snapshot);
    // Account-center wiring lands in Task 10; reading these explicit optional sections here
    // prevents the current callback from ever substituting synthetic finance data for absence.
    let _account_center_finance = (projection.orders, projection.owner_billing);
    ui.push(snapshot.account.user.email_masked.clone().into(), |state, value| state.set_email_mask(value));
    ui.push(snapshot.account.user.invitation_code_submitted, |state, value| state.set_invitation_code_submitted(value));
    if snapshot.account.user.invitation_code_submitted {
        ui.push("".into(), |state, value| state.set_invitation_code(value));
        ui.push(true, |state, value| state.set_invitation_code_success(value));
        ui.push("当前账号已填写过邀请码，每个账号只能填写一次".into(), |state, value| state.set_invitation_code_status(value));
    } else {
        ui.push(false, |state, value| state.set_invitation_code_success(value));
        ui.push("".into(), |state, value| state.set_invitation_code_status(value));
    }
    if let Some(invitation) = snapshot.invitation.as_ref() {
        ui.append(prepare_activation_invitation_projection(invitation));
    } else {
        ui.push("".into(), |state, value| state.set_invitation_reward_rate(value));
        ui.push("".into(), |state, value| state.set_invitation_count(value));
        ui.push("".into(), |state, value| state.set_invitation_history_reward(value));
        ui.push("".into(), |state, value| state.set_invitation_own_code(value));
        ui.push("".into(), |state, value| state.set_invitation_rule_description(value));
        ui.push("".into(), |state, value| state.set_invitation_rewards_status(value));
        ui.push(ModelRc::new(VecModel::from(Vec::<InvitedUserView>::new())), |state, value| state.set_invitation_users(value));
        ui.push(false, |state, value| state.set_invitation_users_loading(value));
        ui.push(false, |state, value| state.set_invitation_users_has_more(value));
        ui.push("".into(), |state, value| state.set_invitation_users_next_cursor(value));
        ui.push("".into(), |state, value| state.set_invitation_users_message(value));
    }
    ui.push(
        snapshot
            .account
            .user
            .nickname
            .clone()
            .unwrap_or_default()
            .into(),
    |state, value| state.set_nickname(value));
    ui.push(snapshot.account.auth_methods.email.bound, |state, value| state.set_email_bound(value));
    ui.push(snapshot.account.auth_methods.password.set, |state, value| state.set_password_set(value));
    ui.push(snapshot.account.auth_methods.wechat.bound, |state, value| state.set_wechat_bound(value));
    ui.push(snapshot.account.auth_methods.wechat.can_unbind, |state, value| state.set_wechat_can_unbind(value));
    ui.push(
        snapshot
            .account
            .auth_methods
            .wechat
            .nickname
            .clone()
            .unwrap_or_default()
            .into(),
    |state, value| state.set_wechat_bound_name(value));
    if let Some(plan) = projection
        .membership
        .and_then(|membership| membership.plan.as_ref())
    {
        ui.push(plan.code.clone().into(), |state, value| state.set_membership_plan_code(value));
        ui.push(plan.name.clone().into(), |state, value| state.set_membership_plan_name(value));
        ui.push(plan.tier_rank, |state, value| state.set_membership_tier_rank(value));
    } else {
        ui.push("".into(), |state, value| state.set_membership_plan_code(value));
        ui.push("".into(), |state, value| state.set_membership_plan_name(value));
        ui.push(0, |state, value| state.set_membership_tier_rank(value));
    }
    let membership_ends_at = projection
        .membership
        .and_then(|membership| membership.ends_at.clone())
        .unwrap_or_default();
    ui.push(format_membership_ends_at(&membership_ends_at).into(), |state, value| state.set_membership_ends_at(value));
    ui.push(membership_expiry_message(&membership_ends_at).into(), |state, value| state.set_membership_expiry_message(value));
    if let Some(credits)=projection.credits {
        ui.push(credits.available.clone().into(), |state, value| state.set_credit_balance(value));
        ui.push(credits.reserved.clone().into(), |state, value| state.set_credit_reserved(value));
    } else {
        ui.push("".into(), |state, value| state.set_credit_balance(value));
        ui.push("".into(), |state, value| state.set_credit_reserved(value));
    }
    let available_packs = projection.packs.unwrap_or(&[]);
    let packs = available_packs
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
    let selected_code = preferred_pack.to_owned();
    if let Some(selected) = available_packs
        .iter()
        .find(|pack| pack.code == selected_code)
        .or_else(|| available_packs.first())
    {
        ui.push(selected.code.clone().into(), |state, value| state.set_selected_credit_pack_code(value));
        ui.push(selected.credits.clone().into(), |state, value| state.set_selected_credit_amount(value));
        ui.push(format_cents(credit_pack_price_cents(selected)).into(), |state, value| state.set_selected_credit_price(value));
    } else {
        ui.push("".into(), |state, value| state.set_selected_credit_pack_code(value));
        ui.push("".into(), |state, value| state.set_selected_credit_amount(value));
        ui.push("".into(), |state, value| state.set_selected_credit_price(value));
    }
    let prepared_invoice_packs=packs.clone();
    ui.push(ModelRc::new(VecModel::from(packs)), |state, value| state.set_credit_packs(value));
    let available_plans = projection.plans.unwrap_or(&[]);
    ui.push(ModelRc::new(VecModel::from(
        available_plans
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
    )), |state, value| state.set_membership_plans(value));
    let (ledger_ui,pagination)=prepare_activation_credit_ledger(&prepared_invoice_packs,projection.ledger.unwrap_or(&[]),snapshot.ledger_next_cursor.clone());
    ui.append(ledger_ui);
    ui.push(ModelRc::new(VecModel::from(
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
    )), |state, value| state.set_account_sessions(value));

    let model_groups=prepare_activation_catalog_projection(&mut ui,projection.models.unwrap_or(&[]),preferred_image,preferred_prompt,preferred_video);
    PreparedBackendProjection { ui,model_groups,pagination,credit_version:projection.credits.map(|credit|credit.version.clone()) }
}

fn prepare_activation_catalog_projection(ui:&mut PreparedUiProjection,available_models:&[ModelCatalogItem],preferred_image:&str,preferred_prompt:&str,preferred_video:&str) -> Vec<ModelGroupData> {
    let catalog_models = available_models
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
            video_price_480: model_credit_cost(model, "480P").into(),
            video_price_720: model_credit_cost(model, "720P").into(),
            video_price_1080: model_credit_cost(model, "1080P").into(),
            supports_image_edit: model_supports_task_type(model, "image_edit")
                && model_capability_enabled(model, "supports_masks"),
            supports_style_analysis: model_supports_task_type(model, "image_style_analysis")
                && model_capability_enabled(model, "supports_references")
                && model_supports_operation(model, "analyze_style"),
        })
        .collect::<Vec<_>>();
    let video_model_options = catalog_models
        .iter()
        .filter(|model| model.purpose == "video_generation")
        .cloned()
        .collect::<Vec<_>>();
    let style_models=catalog_models.clone();
    ui.push(ModelRc::new(VecModel::from(catalog_models)), |state, value| state.set_catalog_models(value));
    ui.push(ModelRc::new(VecModel::from(video_model_options)), |state, value| state.set_video_model_options(value));

    let image_models = available_models
        .iter()
        .filter(|item| item.purpose == "image_generation")
        .map(|item| ModelOptionData {
            code: item.code.clone(),
            name: model_display_name(item),
        })
        .collect::<Vec<_>>();
    let prompt_models = available_models
        .iter()
        .filter(|item| item.purpose == "prompt_processing")
        .map(|item| ModelOptionData {
            code: item.code.clone(),
            name: item.name.clone(),
        })
        .collect::<Vec<_>>();
    let selected_image_code = preferred_image.to_owned();
    let selected_image = available_models
        .iter()
        .find(|item| item.purpose == "image_generation" && item.code == selected_image_code)
        .or_else(|| {
            available_models
                .iter()
                .find(|item| item.code == "openai_image")
        })
        .or_else(|| {
            available_models
                .iter()
                .find(|item| item.purpose == "image_generation")
        });
    let selected_prompt_code = preferred_prompt.to_owned();
    let selected_prompt = available_models
        .iter()
        .find(|item| item.purpose == "prompt_processing" && item.code == selected_prompt_code)
        .or_else(|| {
            available_models
                .iter()
                .find(|item| item.code == "gpt_5_6_sol")
        })
        .or_else(|| {
            available_models
                .iter()
                .find(|item| item.purpose == "prompt_processing")
        });
    let selected_video_code = preferred_video.to_owned();
    let selected_video = select_video_catalog_model(available_models, &selected_video_code);
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
    ui.append(prepare_model_groups_projection(&model_groups));
    if let Some(model) = selected_image {
        prepare_activation_image_model(ui, model);
    } else {
        prepare_activation_clear_image(ui);
    }
    if let Some(model) = selected_prompt {
        ui.push(model.code.clone().into(), |state, value| state.set_reasoning_model(value));
        ui.push(model.name.clone().into(), |state, value| state.set_reasoning_model_name(value));
    } else {
        prepare_activation_clear_prompt(ui);
    }
    if let Some(model) = selected_video {
        ui.push(model.code.clone().into(), |state, value| state.set_video_model(value));
        ui.push(model_display_name(model).into(), |state, value| state.set_video_model_name(value));
        ui.push(model_capabilities_text(model).into(), |state, value| state.set_video_model_description(value));
        ui.push(true, |state, value| { state.set_video_service_available(value); sync_video_resolutions(state); });
    } else {
        ui.push("".into(), |state, value| state.set_video_model(value));
        ui.push("".into(), |state, value| state.set_video_model_name(value));
        ui.push("".into(), |state, value| state.set_video_model_description(value));
        ui.push(false, |state, value| state.set_video_service_available(value));
    }
    ui.append(prepare_activation_style_projection(style_models,selected_prompt.map(|model|model.code.as_str()).unwrap_or_default()));
    if available_models.is_empty() {
        ui.push("".into(), |state, value| state.set_model_catalog_message(value));
    }
    model_groups
}

fn prepare_activation_image_model(ui: &mut PreparedUiProjection, model: &ModelCatalogItem) {
    ui.push(model.code.clone().into(), |state, value| state.set_image_model(value));
    ui.push(model_display_name(model).into(), |state, value| state.set_image_model_name(value));
    ui.push(model_price(model, "1K"), |state, value| state.set_image_price_1k(value));
    ui.push(model_price(model, "2K"), |state, value| state.set_image_price_2k(value));
    ui.push(model_price(model, "4K"), |state, value| state.set_image_price_4k(value));
}

fn prepare_activation_clear_image(ui: &mut PreparedUiProjection) {
    ui.push("".into(), |state, value| state.set_image_model(value));
    ui.push("".into(), |state, value| state.set_image_model_name(value));
    ui.push(0, |state, value| state.set_image_price_1k(value));
    ui.push(0, |state, value| state.set_image_price_2k(value));
    ui.push(0, |state, value| state.set_image_price_4k(value));
    ui.push("".into(), |state, value| state.set_image_editor_model(value));
    ui.push("".into(), |state, value| state.set_image_editor_model_name(value));
    ui.push(0, |state, value| state.set_image_editor_price_1k(value));
    ui.push(0, |state, value| state.set_image_editor_price_2k(value));
    ui.push(0, |state, value| state.set_image_editor_price_4k(value));
}

fn prepare_activation_clear_prompt(ui: &mut PreparedUiProjection) {
    ui.push("".into(), |state, value| state.set_reasoning_model(value));
    ui.push("".into(), |state, value| state.set_reasoning_model_name(value));
}

struct StartupAuthResult {
    auth_epoch: u64,
    credit_sync_epoch: u64,
    agreements: std::result::Result<Vec<AgreementItem>, ApiError>,
    refresh: Option<std::result::Result<String, ApiError>>,
    snapshot: Option<std::result::Result<BackendSnapshot, ApiError>>,
}

type LoginResult = std::result::Result<LoginWorkerResponse, ApiError>;

enum LoginWorkerResponse {
    Email { outcome: EmailLoginOutcome, email: SecretString, acceptances: Vec<AgreementAcceptance> },
    Password(LoginResponse),
}

struct PendingRegistrationOutcome {
    normalized_email: SecretString,
    agreement_acceptances: Vec<AgreementAcceptance>,
    idempotency_key: String,
    password: Option<SecretString>,
    auth_operation_epoch: u64,
    registration_continuation: SecretString,
    continuation_expires_at: chrono::DateTime<chrono::Utc>,
    invitations: Vec<TeamRegistrationInvitationSummary>,
    pending_invitation_count: u64,
    selection_state: TeamRegistrationSelectionState,
}
include!("team_registration.rs");

enum WechatPollOutcome {
    Pending,
    Scanned(String),
    AgreementRequired(String),
    Failed(String),
    Completed(LoginResponse),
}

pub(super) fn begin_auth_operation(context: &AppContext) -> u64 {
    context.auth_operation_epoch.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |epoch| epoch.checked_add(1))
        .map(|previous| previous + 1).unwrap_or(u64::MAX)
}

pub(super) fn auth_operation_is_current(context: &AppContext, operation_epoch: u64) -> bool {
    operation_epoch != u64::MAX && operation_epoch != 0
        && context.auth_operation_epoch.load(Ordering::SeqCst) == operation_epoch
}

fn invalidate_auth_operations(context: &AppContext) {
    let _ = context.auth_operation_epoch.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |epoch| epoch.checked_add(1));
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

fn begin_account_activation(state: &AppState, wechat_status: Option<&str>) {
    state.set_auth_busy(true);
    state.set_session_state("activating".into());
    state.set_auth_error("".into());
    if let Some(status) = wechat_status {
        state.set_auth_wechat_status(status.into());
    }
}

pub(super) fn wire_auth_callbacks(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();
    let pending_registration = Rc::new(RefCell::new(None::<PendingRegistrationOutcome>));
    wire_team_registration_callbacks(app, context.clone(), pending_registration.clone());

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
        let pending_registration = pending_registration.clone();
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
                if let Err(error) = validate_login_password(&credential) {
                    state.set_auth_error(
                        match error {
                            PasswordLoginInputError::Empty => "请输入密码",
                            PasswordLoginInputError::TooManyBytes => {
                                "密码不能超过 512 个 UTF-8 字节"
                            }
                        }
                        .into(),
                    );
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
            pending_registration.borrow_mut().take();
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
                        .map(LoginWorkerResponse::Password)
                } else {
                    api.login_response(&email, &credential, &acceptances)
                        .map(|outcome| LoginWorkerResponse::Email { outcome, email: SecretString::new(email), acceptances })
                };
                let _ = sender.send(result);
            });
            poll_login_result(
                app.as_weak(),
                context,
                auth_operation_epoch,
                login_mode,
                Rc::new(RefCell::new(Some(receiver))),
                pending_registration.clone(),
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
        let context = context.clone();
        state.on_logout(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            if let (Some(coordinator), Some(scope)) = (context.account_transition.clone(), current_auth_session_scope(&context)) {
                coordinator.logout(&app, context.clone(), scope, false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_logout_all(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            if let (Some(coordinator), Some(scope)) = (context.account_transition.clone(), current_auth_session_scope(&context)) {
                coordinator.logout(&app, context.clone(), scope, true);
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
                save_user_profile(&app, &context.store.borrow());
            }
            Err(error) => {
                state.set_agreement_update_message(auth_error_message(&error).into());
            }
        }
    });
}

pub(super) fn initialize_auth(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    state.set_auth_open(true);
    state.set_logged_in(false);
    state.set_session_state("signed_out".into());
    let Some(backend) = context.backend.clone() else { state.set_auth_error("后端客户端初始化失败".into()); return; };
    let weak = app.as_weak();
    let api = AuthApi::new(backend.api.clone());
    std::thread::spawn(move || {
        let result = api.list_agreements();
        let _ = weak.upgrade_in_event_loop(move |app| {
            match result {
                Ok(agreements) => apply_agreements(&app, &agreements),
                Err(error) => app.global::<AppState>().set_auth_error(error.user_message().into()),
            }
        });
    });
    if let Some(coordinator) = context.account_transition.clone() {
        coordinator.resume_persisted_session(app, context.clone());
    }
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

fn network_recovery_allowed(state: &AppState) -> bool {
    !state.get_auth_open()
        && !state.get_auth_busy()
        && !state.get_auth_wechat_busy()
        && state.get_auth_wechat_login_id().is_empty()
        && matches!(state.get_session_state().as_str(), "offline" | "signed_out")
}

fn try_network_recovery(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if !network_recovery_allowed(&state) {
        return;
    }
    if context.active_namespace.lock().unwrap_or_else(|poison| poison.into_inner()).is_some() { return; }
    if let Some(coordinator) = context.account_transition.clone() {
        coordinator.resume_persisted_session(app, context);
    }
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
                if context.backend.is_none() {
                    state.set_auth_busy(false);
                    state.set_session_state("signed_out".into());
                    state.set_auth_error("后端客户端初始化失败".into());
                    return;
                }
                let current = auth_operation_is_current(&context, auth_operation_epoch)
                    && state.get_auth_open()
                    && state.get_auth_method().as_str() == "wechat"
                    && state.get_auth_wechat_login_id().as_str() == login_id;
                if !current { return; }
                state.set_auth_wechat_login_id("".into());
                state.set_auth_wechat_qr_ready(false);
                state.set_auth_wechat_scanned(false);
                begin_account_activation(&state, Some("登录成功，正在加载账号数据"));
                if let Some(coordinator) = context.account_transition.clone() {
                    coordinator.activate_authenticated(&app, context, response, LoginOrigin::Wechat, None);
                } else {
                    state.set_auth_busy(false);
                    state.set_session_state("signed_out".into());
                    state.set_auth_error("账号初始化服务不可用，请重试".into());
                }
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
    pending_registration: Rc<RefCell<Option<PendingRegistrationOutcome>>>,
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
                    pending_registration,
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
        match result {
            Ok(response) => handle_login_success(
                &app,
                &context,
                auth_operation_epoch,
                &login_mode,
                response,
                &pending_registration,
            ),
            Err(error) => {
                state.set_auth_busy(false);
                state.set_session_state("signed_out".into());
                apply_email_login_error(&app, &login_mode, error);
            }
        }
    });
}

fn handle_login_success(
    app: &AppWindow,
    context: &AppContext,
    auth_operation_epoch: u64,
    login_mode: &str,
    response: LoginWorkerResponse,
    pending_registration: &Rc<RefCell<Option<PendingRegistrationOutcome>>>,
) {
    let state = app.global::<AppState>();
    match response {
        LoginWorkerResponse::Password(response)
        | LoginWorkerResponse::Email { outcome: EmailLoginOutcome::Authenticated { login: response }, .. } => {
            pending_registration.borrow_mut().take();
            if context.backend.is_none() {
                state.set_auth_busy(false);
                state.set_session_state("signed_out".into());
                state.set_auth_error("后端客户端初始化失败".into());
                return;
            }
            let current = auth_operation_is_current(context, auth_operation_epoch)
                && state.get_auth_open()
                && state.get_auth_method().as_str() == "email"
                && state.get_auth_email_mode().as_str() == login_mode;
            if current {
                begin_account_activation(&state, None);
                if let Some(coordinator) = context.account_transition.clone() {
                    let origin = if login_mode == "password" { LoginOrigin::Password } else { LoginOrigin::Email };
                    coordinator.activate_authenticated(app, context.clone(), response, origin, None);
                } else {
                    state.set_auth_busy(false);
                    state.set_session_state("signed_out".into());
                    state.set_auth_error("账号初始化服务不可用，请重试".into());
                }
            }
        }
        LoginWorkerResponse::Email { outcome: EmailLoginOutcome::TeamRegistrationRequired {
            registration_continuation,
            continuation_expires_at,
            invitations,
            pending_invitation_count,
            selection_state,
        }, email, acceptances } => {
            state.set_auth_busy(false);
            let Ok(continuation_expires_at) = chrono::DateTime::parse_from_rfc3339(&continuation_expires_at).map(|value| value.with_timezone(&chrono::Utc)) else {
                state.set_auth_error("团队注册验证有效期无效，请重新验证邮箱".into());
                state.set_session_state("signed_out".into());
                return;
            };
            *pending_registration.borrow_mut() = Some(PendingRegistrationOutcome {
                normalized_email: email,
                agreement_acceptances: acceptances,
                idempotency_key: Uuid::new_v4().to_string(),
                password: None,
                auth_operation_epoch,
                registration_continuation,
                continuation_expires_at,
                invitations,
                pending_invitation_count,
                selection_state,
            });
            state.set_session_state("team_registration_required".into());
            state.set_auth_open(true);
            state.set_auth_error("请设置密码以完成团队邀请注册".into());
            project_team_registration(&state, pending_registration.borrow().as_ref().unwrap());
        }
    }
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
    save_user_profile(app, &context.store.borrow());
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
pub(super) enum ReceiverPoll<T> {
    Pending,
    Ready(T),
    Disconnected,
}

pub(super) fn poll_receiver<T>(receiver: &Rc<RefCell<Option<mpsc::Receiver<T>>>>) -> ReceiverPoll<T> {
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
            save_user_profile(app, &context.store.borrow());
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

pub(super) fn clear_billing_snapshot_state(app: &AppWindow, context: &AppContext) {
    *context.account_snapshot_scope.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
    let state = app.global::<AppState>();
    state.set_membership_plan_code("".into()); state.set_membership_plan_name("".into());
    state.set_membership_ends_at("".into()); state.set_membership_expiry_message("".into()); state.set_membership_tier_rank(0);
    state.set_membership_plans(ModelRc::new(VecModel::default())); state.set_membership_open(false);
    state.set_credit_balance("".into()); state.set_credit_reserved("".into());
    invalidate_credit_account_view(&context.store); reset_credit_ledger(app, &context.store, &[], None);
    state.set_credit_packs(ModelRc::new(VecModel::default())); state.set_invoice_orders(ModelRc::new(VecModel::default()));
    state.set_selected_credit_pack_code("".into()); state.set_selected_credit_amount("".into()); state.set_selected_credit_price("".into());
    state.set_payment_active(false); state.set_payment_dialog_open(false); state.set_payment_browser_ready(false);
    state.set_payment_status_message("".into()); state.set_payment_waiting_message("".into());
    state.set_payment_success_message("".into()); state.set_payment_success_detail("".into());
    state.set_credit_payment_busy(false); state.set_membership_payment_busy(false);
    state.set_credit_payment_message("".into()); state.set_membership_payment_message("".into());
    state.set_credit_insufficient_open(false); clear_credit_redemption_state(app);
    state.set_catalog_models(ModelRc::new(VecModel::default())); state.set_video_model_options(ModelRc::new(VecModel::default()));
    clear_image_model_authority(&state); clear_reasoning_model_authority(&state);
    state.set_video_service_available(false); state.set_style_analysis_available(false);
    state.set_style_analysis_model_code("".into()); state.set_style_analysis_display_name("".into()); state.set_style_analysis_credit_cost("".into());
    let mut store = context.store.borrow_mut(); store.model_groups.clear(); push_model_groups(app, &store);
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
    state.set_password_set(false);
    clear_password_management_state(&state);
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
    state.set_video_model_options(ModelRc::new(VecModel::from(
        Vec::<CatalogModelView>::new(),
    )));
    clear_image_model_authority(&state);
    clear_reasoning_model_authority(&state);
    state.set_video_model("".into());
    state.set_video_model_name("".into());
    state.set_video_model_description("".into());
    state.set_video_service_available(false);
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

struct CapturedBackendRefresh {
    context: AppContext,
    persistence: PrivatePersistence,
    billing: BillingScope,
    epoch: u64,
    nickname: String,
    status: String,
}
impl CapturedBackendRefresh {
    fn binding_matches(&self) -> bool {
        self.context.store.borrow().private_persistence.as_ref()
            .is_some_and(|current| current.same_binding_metadata(&self.persistence))
    }
    // Pure metadata only; safe to repeat inside the original short completion.
    fn metadata_matches(&self) -> bool {
        self.binding_matches()
            && self.context.billing_context.is_current(&self.billing)
            && auth_scope_matches_context(&self.context, &self.billing.request.session)
            && credit_sync_epoch_is_current(&self.context.store.borrow(), self.epoch)
    }
    fn current(&self) -> bool { self.persistence.is_current() && self.metadata_matches() }
}
pub(super) fn refresh_backend_snapshot_captured(
    app: &AppWindow, context: AppContext, persistence: PrivatePersistence,
) {
    let Some(backend) = context.backend.clone() else { return };
    let Some(billing) = context.billing_context.confirmed_scope() else { return };
    if !persistence.is_current()
        || persistence.lease().namespace.user_public_id() != billing.request.session.owner_user_id
        || persistence.lease().auth_epoch != billing.request.session.auth_epoch
        || !auth_scope_matches_context(&context, &billing.request.session)
        || !context.store.borrow().private_persistence.as_ref()
            .is_some_and(|current| current.same_binding_metadata(&persistence)) { return; }
    let epoch = context.apply_user_completion(persistence.lease(), || {
        if !context.billing_context.is_current(&billing)
            || !context.store.borrow().private_persistence.as_ref()
                .is_some_and(|current| current.same_binding_metadata(&persistence)) { return None; }
        let epoch = {
            let mut store = context.store.borrow_mut();
            let epoch = store.credit_sync_epoch.checked_add(1)?;
            store.credit_sync_epoch = epoch;
            epoch
        };
        app.global::<AppState>().set_credit_ledger_loading(true);
        Some(epoch)
    }).ok().flatten();
    let Some(epoch) = epoch else { return };
    let captured = Rc::new(CapturedBackendRefresh {
        nickname: app.global::<AppState>().get_nickname().to_string(),
        status: app.global::<AppState>().get_generation_status().to_string(),
        context, persistence, billing: billing.clone(), epoch,
    });
    let work = spawn_delivery_preparation(&captured.persistence, move |_, activity, cancel| {
        if cancel.load(Ordering::SeqCst) || activity.is_quiescing() {
            return Err(DeliveryRetryError::AuthenticationRequired);
        }
        let snapshot = AccountApi::new(backend.api.clone()).snapshot_billing(&billing)?;
        if cancel.load(Ordering::SeqCst) || activity.is_quiescing() {
            return Err(DeliveryRetryError::AuthenticationRequired);
        }
        Ok(snapshot)
    });
    match work {
        Ok((cancel, receiver)) => poll_captured_backend_snapshot(app.as_weak(), captured, cancel, receiver),
        Err(error) => captured_backend_refresh_error(app, &captured, error.into(), false),
    }
}
fn captured_backend_refresh_error(
    app: &AppWindow, captured: &CapturedBackendRefresh, error: DeliveryRetryError, profile: bool,
) {
    if matches!(&error, DeliveryRetryError::Api(api) if api.is_terminal_session_error()) {
        let session = &captured.billing.request.session;
        if captured.binding_matches() && terminal_auth_scope_matches_context(&captured.context, session) {
            drop(error);
            sign_out_locally(app, &captured.context, true, Some(session.auth_epoch));
        }
        return;
    }
    if !captured.current() { return; }
    let _ = captured.context.apply_user_completion(captured.persistence.lease(), || {
        if !captured.metadata_matches() { return; }
        let state = app.global::<AppState>();
        state.set_credit_ledger_loading(false);
        if state.get_generation_status().as_str() == captured.status {
            state.set_generation_status(if profile {
                "账号数据已刷新，个人设置未能安全保存，请重试"
            } else {
                "账号数据刷新未确认，请稍后重试"
            }.into());
        }
    });
}
fn poll_captured_backend_snapshot(
    weak: Weak<AppWindow>, captured: Rc<CapturedBackendRefresh>, cancel: Arc<std::sync::atomic::AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<BackendSnapshot, DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let app = weak.upgrade();
        if app.is_none() || !captured.current() { cancel.store(true, Ordering::SeqCst); }
        let joined = finish_delivery_preparation(&cancel);
        if matches!(joined, Ok(true)) {
            poll_captured_backend_snapshot(weak, captured, cancel, receiver); return;
        }
        let result = match joined {
            Err(error) => Err(error.into()),
            Ok(false) => receiver.try_recv().unwrap_or_else(|_| Err(anyhow!("snapshot worker disconnected").into())),
            Ok(true) => unreachable!(),
        };
        let Some(app) = app else { return };
        match result {
            Err(error) => captured_backend_refresh_error(&app, &captured, error, false),
            Ok(snapshot) => publish_captured_backend_snapshot(&app, captured, snapshot),
        }
    });
}
fn publish_captured_backend_snapshot(
    app: &AppWindow, captured: Rc<CapturedBackendRefresh>, snapshot: BackendSnapshot,
) {
    if !captured.current() { return; }
    if snapshot.account.user.id != captured.billing.request.session.owner_user_id
        || snapshot.account.billing_group.group_id != captured.billing.request.account_group_id {
        captured_backend_refresh_error(app, &captured, anyhow!("snapshot identity mismatch").into(), false); return;
    }
    let state = app.global::<AppState>();
    let projection = prepare_activation_backend_projection(&snapshot,
        state.get_image_model().as_str(), state.get_reasoning_model().as_str(),
        state.get_video_model().as_str(), state.get_selected_credit_pack_code().as_str());
    let current_nickname = state.get_nickname().to_string();
    let keep_nickname = current_nickname != captured.nickname;
    let mut write = match captured.persistence.prepare_ordered_save() {
        Ok(write) => Some(write),
        Err(error) => { captured_backend_refresh_error(app, &captured, error.into(), true); return; }
    };
    let queued = captured.context.apply_user_completion(captured.persistence.lease(), || {
        if !captured.metadata_matches() { return None; }
        if let Some(credit) = snapshot.account.credits.as_ref() {
            if !apply_credit_account_balance_if_fresh(app, &captured.context.store,
                &credit.available, &credit.reserved, &credit.version, captured.epoch) {
                state.set_credit_ledger_loading(false); return None;
            }
        }
        let PreparedBackendProjection { ui, model_groups, pagination, credit_version: _ } = projection;
        if !captured.context.billing_context.refresh_financial_snapshot(&captured.billing, &snapshot.account) {
            state.set_credit_ledger_loading(false);
            return None;
        }
        {
            let mut store = captured.context.store.borrow_mut();
            store.model_groups = model_groups;
            store.credit_ledger_pagination = pagination;
            if snapshot.account.credits.is_none() { store.credit_account_version = None; }
        }
        *captured.context.account_snapshot_scope.lock().unwrap_or_else(|error| error.into_inner())
            = Some(captured.billing.request.session.clone());
        ui.publish(&state);
        render_team_context(app, &captured.context);
        if keep_nickname { state.set_nickname(current_nickname.into()); }
        // Projection is staged, not a durable-success receipt. Keep loading until
        // the exact queued profile write and its registered waiter have completed.
        state.set_credit_ledger_loading(true);
        Some(write.take().unwrap().enqueue_profile(user_profile_data(app)))
    });
    // Even the error owns admission: never convert/drop it under the latch.
    drop(write);
    let receiver = match queued.ok().flatten() {
        Some(Ok(receiver)) => receiver,
        Some(Err(error)) => {
            drop(error);
            captured_backend_refresh_error(app, &captured, anyhow!("profile enqueue refused").into(), true); return;
        }
        None => return,
    };
    let work = spawn_delivery_preparation(&captured.persistence, move |_, activity, cancel| {
        loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => { result.map_err(anyhow::Error::from)?; return Ok(()); }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(anyhow!("profile acknowledgment disconnected").into()),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if cancel.load(Ordering::SeqCst) || activity.is_quiescing() {
                        // The queued writer, not this waiter, owns the durable guard.
                        return Err(DeliveryRetryError::AuthenticationRequired);
                    }
                }
            }
        }
    });
    match work {
        Ok((cancel, receiver)) => poll_captured_backend_profile(app.as_weak(), captured, cancel, receiver),
        Err(error) => captured_backend_refresh_error(app, &captured, error.into(), true),
    }
}
fn poll_captured_backend_profile(
    weak: Weak<AppWindow>, captured: Rc<CapturedBackendRefresh>, cancel: Arc<std::sync::atomic::AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<(), DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let app = weak.upgrade();
        if app.is_none() || !captured.current() { cancel.store(true, Ordering::SeqCst); }
        let joined = finish_delivery_preparation(&cancel);
        if matches!(joined, Ok(true)) {
            poll_captured_backend_profile(weak, captured, cancel, receiver); return;
        }
        let result = match joined {
            Err(error) => Err(error.into()),
            Ok(false) => receiver.try_recv().unwrap_or_else(|_| Err(anyhow!("profile waiter disconnected").into())),
            Ok(true) => unreachable!(),
        };
        let Some(app) = app else { return };
        match result {
            Err(error) => captured_backend_refresh_error(&app, &captured, error, true),
            Ok(()) if captured.current() => {
                let _ = captured.context.apply_user_completion(captured.persistence.lease(), || {
                    if captured.metadata_matches() { app.global::<AppState>().set_credit_ledger_loading(false); }
                });
            }
            Ok(()) => {}
        }
    });
}

pub(super) fn refresh_backend_snapshot(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let Some(billing_scope) = context.billing_context.confirmed_scope() else {
        return;
    };
    let Ok(activity) = backend.api.begin_user_work(&billing_scope.request.session) else { return; };
    let credit_sync_epoch = begin_credit_sync_epoch(&mut context.store.borrow_mut());
    app.global::<AppState>().set_credit_ledger_loading(true);
    let (sender, receiver) = mpsc::channel();
    let worker_scope = billing_scope.clone();
    std::thread::spawn(move || {
        let result = if activity.is_quiescing() { Err(ApiError::AuthenticationRequired) }
            else { AccountApi::new(backend.api.clone()).snapshot_billing(&worker_scope) };
        drop(activity);
        let _ = sender.send(result);
    });
    poll_backend_snapshot(
        app.as_weak(),
        context,
        billing_scope,
        credit_sync_epoch,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_backend_snapshot(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    billing_scope: BillingScope,
    credit_sync_epoch: u64,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<BackendSnapshot, ApiError>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let session_scope = billing_scope.request.session.clone();
        if !context.billing_context.is_current(&billing_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let result = match poll_receiver(&receiver) {
            ReceiverPoll::Pending => {
                poll_backend_snapshot(
                    app_weak,
                    context,
                    billing_scope,
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
        if !outcome_is_current || !context.billing_context.is_current(&billing_scope) {
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

struct BackendSnapshotProjection<'a> {
    membership: Option<&'a AccountMembership>,
    credits: Option<&'a CreditAccount>,
    plans: Option<&'a [MembershipPlan]>,
    packs: Option<&'a [CreditPack]>,
    models: Option<&'a [ModelCatalogItem]>,
    ledger: Option<&'a [CreditLedgerItem]>,
    orders: Option<&'a TeamPage<OrderDetail>>,
    owner_billing: Option<&'a BillingSummary>,
}

fn project_backend_snapshot(snapshot: &BackendSnapshot) -> BackendSnapshotProjection<'_> {
    BackendSnapshotProjection {
        membership: snapshot.account.membership.as_ref(),
        credits: snapshot.account.credits.as_ref(),
        plans: snapshot.plans.as_deref(),
        packs: snapshot.packs.as_deref(),
        models: snapshot.models.as_deref(),
        ledger: snapshot.ledger.as_deref(),
        orders: snapshot.orders.as_ref(),
        owner_billing: snapshot.owner_billing.as_ref(),
    }
}

pub(super) fn apply_backend_snapshot(
    app: &AppWindow,
    context: &AppContext,
    snapshot: BackendSnapshot,
    credit_sync_epoch: u64,
) {
    let Some(billing_scope) = context.billing_context.confirmed_scope() else { return; };
    if snapshot.account.user.id != billing_scope.request.session.owner_user_id
        || snapshot.account.billing_group.group_id != billing_scope.request.account_group_id { return; }
    let Ok(lease) = context.namespace_for(&billing_scope.request.session) else { return; };
    let applied = context.apply_user_completion(&lease, || {
        if !context.billing_context.is_current(&billing_scope) { return false; }
        apply_backend_snapshot_projection(app, context, snapshot, credit_sync_epoch);
        true
    }).unwrap_or(false);
    if applied { save_user_profile(app, &context.store.borrow()); }
}

fn apply_backend_snapshot_projection(
    app: &AppWindow, context: &AppContext, snapshot: BackendSnapshot, credit_sync_epoch: u64,
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
    let Some(snapshot_scope) = session.scope_for_user(&snapshot.account.user.id) else {
        return;
    };
    let state = app.global::<AppState>();
    let projection = project_backend_snapshot(&snapshot);
    // Account-center wiring lands in Task 10; reading these explicit optional sections here
    // prevents the current callback from ever substituting synthetic finance data for absence.
    let _account_center_finance = (projection.orders, projection.owner_billing);
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
    state.set_password_set(snapshot.account.auth_methods.password.set);
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
    if let Some(plan) = projection
        .membership
        .and_then(|membership| membership.plan.as_ref())
    {
        state.set_membership_plan_code(plan.code.clone().into());
        state.set_membership_plan_name(plan.name.clone().into());
        state.set_membership_tier_rank(plan.tier_rank);
    } else {
        state.set_membership_plan_code("".into());
        state.set_membership_plan_name("".into());
        state.set_membership_tier_rank(0);
    }
    let membership_ends_at = projection
        .membership
        .and_then(|membership| membership.ends_at.clone())
        .unwrap_or_default();
    state.set_membership_ends_at(format_membership_ends_at(&membership_ends_at).into());
    state.set_membership_expiry_message(membership_expiry_message(&membership_ends_at).into());
    let credit_snapshot_applied = if let Some(credits) = projection.credits {
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
        state.set_credit_balance("".into());
        state.set_credit_reserved("".into());
        true
    };
    let available_packs = projection.packs.unwrap_or(&[]);
    let packs = available_packs
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
    if let Some(selected) = available_packs
        .iter()
        .find(|pack| pack.code == selected_code)
        .or_else(|| available_packs.first())
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
    let available_plans = projection.plans.unwrap_or(&[]);
    state.set_membership_plans(ModelRc::new(VecModel::from(
        available_plans
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
    let credit_snapshot_is_current =
        credit_sync_epoch_is_current(&context.store.borrow(), credit_sync_epoch);
    if credit_snapshot_applied && credit_snapshot_is_current {
        match projection.ledger {
            Some(ledger) => reset_credit_ledger(
                app,
                &context.store,
                ledger,
                snapshot.ledger_next_cursor.clone(),
            ),
            None => reset_credit_ledger(app, &context.store, &[], None),
        }
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

    apply_model_catalog_projection(app, context, projection.models.unwrap_or(&[]));
    *context
        .account_snapshot_scope
        .lock()
        .unwrap_or_else(|value| value.into_inner()) = Some(snapshot_scope);
}

pub(super) fn apply_model_catalog_projection(
    app: &AppWindow,
    context: &AppContext,
    available_models: &[ModelCatalogItem],
) {
    let state = app.global::<AppState>();
    let catalog_models = available_models
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
            video_price_480: model_credit_cost(model, "480P").into(),
            video_price_720: model_credit_cost(model, "720P").into(),
            video_price_1080: model_credit_cost(model, "1080P").into(),
            supports_image_edit: model_supports_task_type(model, "image_edit")
                && model_capability_enabled(model, "supports_masks"),
            supports_style_analysis: model_supports_task_type(model, "image_style_analysis")
                && model_capability_enabled(model, "supports_references")
                && model_supports_operation(model, "analyze_style"),
        })
        .collect::<Vec<_>>();
    let video_model_options = catalog_models
        .iter()
        .filter(|model| model.purpose == "video_generation")
        .cloned()
        .collect::<Vec<_>>();
    state.set_catalog_models(ModelRc::new(VecModel::from(catalog_models)));
    state.set_video_model_options(ModelRc::new(VecModel::from(video_model_options)));

    let image_models = available_models
        .iter()
        .filter(|item| item.purpose == "image_generation")
        .map(|item| ModelOptionData {
            code: item.code.clone(),
            name: model_display_name(item),
        })
        .collect::<Vec<_>>();
    let prompt_models = available_models
        .iter()
        .filter(|item| item.purpose == "prompt_processing")
        .map(|item| ModelOptionData {
            code: item.code.clone(),
            name: item.name.clone(),
        })
        .collect::<Vec<_>>();
    let selected_image_code = state.get_image_model().to_string();
    let selected_image = available_models
        .iter()
        .find(|item| item.purpose == "image_generation" && item.code == selected_image_code)
        .or_else(|| {
            available_models
                .iter()
                .find(|item| item.code == "openai_image")
        })
        .or_else(|| {
            available_models
                .iter()
                .find(|item| item.purpose == "image_generation")
        });
    let selected_prompt_code = state.get_reasoning_model().to_string();
    let selected_prompt = available_models
        .iter()
        .find(|item| item.purpose == "prompt_processing" && item.code == selected_prompt_code)
        .or_else(|| {
            available_models
                .iter()
                .find(|item| item.code == "gpt_5_6_sol")
        })
        .or_else(|| {
            available_models
                .iter()
                .find(|item| item.purpose == "prompt_processing")
        });
    let selected_video_code = state.get_video_model().to_string();
    let selected_video = select_video_catalog_model(available_models, &selected_video_code);
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
    } else {
        clear_image_model_authority(&state);
    }
    if let Some(model) = selected_prompt {
        state.set_reasoning_model(model.code.clone().into());
        state.set_reasoning_model_name(model.name.clone().into());
    } else {
        clear_reasoning_model_authority(&state);
    }
    if let Some(model) = selected_video {
        state.set_video_model(model.code.clone().into());
        state.set_video_model_name(model_display_name(model).into());
        state.set_video_model_description(model_capabilities_text(model).into());
        state.set_video_service_available(true);
        sync_video_resolutions(&state);
    } else {
        state.set_video_model("".into());
        state.set_video_model_name("".into());
        state.set_video_model_description("".into());
        state.set_video_service_available(false);
    }
    sync_style_analysis_selection(&state);
    if available_models.is_empty() {
        state.set_model_catalog_message("".into());
    }
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

fn select_video_catalog_model<'a>(
    models: &'a [ModelCatalogItem],
    preferred: &str,
) -> Option<&'a ModelCatalogItem> {
    models
        .iter()
        .find(|item| item.purpose == "video_generation" && item.code == preferred)
        .or_else(|| {
            models
                .iter()
                .find(|item| item.purpose == "video_generation")
        })
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

fn clear_image_model_authority(state: &AppState) {
    state.set_image_model("".into());
    state.set_image_model_name("".into());
    state.set_image_price_1k(0);
    state.set_image_price_2k(0);
    state.set_image_price_4k(0);
    state.set_image_editor_model("".into());
    state.set_image_editor_model_name("".into());
    state.set_image_editor_price_1k(0);
    state.set_image_editor_price_2k(0);
    state.set_image_editor_price_4k(0);
}

fn clear_reasoning_model_authority(state: &AppState) {
    state.set_reasoning_model("".into());
    state.set_reasoning_model_name("".into());
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
    if let Some(coordinator) = context.account_transition.clone() {
        let active = context.active_namespace.lock().unwrap_or_else(|poison| poison.into_inner()).clone();
        if let Some(lease) = active.filter(|lease| expected_auth_epoch.map(|epoch| epoch == lease.auth_epoch).unwrap_or(true)) {
            let scope = SessionScope { owner_user_id: lease.namespace.user_public_id().into(), auth_epoch: lease.auth_epoch };
            coordinator.logout(app, context.clone(), scope, false);
        }
        return;
    }
    let generation_owner_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone()
        .filter(|value| !value.trim().is_empty());
    let generation_teardown_scope = expected_auth_epoch
        .and_then(|auth_epoch| {
            generation_owner_user_id
                .clone()
                .map(|owner_user_id| SessionScope {
                    owner_user_id,
                    auth_epoch,
                })
        })
        .or_else(|| current_generation_session_scope(context));
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
    clear_generation_account_state(app, context, generation_teardown_scope.as_ref());
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
    state.set_auth_email("".into());
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
    save_user_profile(app, &context.store.borrow());
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

pub(super) fn valid_email(email: &str) -> bool {
    let mut parts = email.split('@');
    matches!((parts.next(), parts.next(), parts.next()), (Some(local), Some(domain), None) if !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_transport_keeps_login_blocked_until_account_activation_finishes() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_auth_busy(false);
        state.set_session_state("authenticating".into());

        begin_account_activation(&state, Some("登录成功，正在加载账号数据"));

        assert!(state.get_auth_busy());
        assert_eq!(state.get_session_state().as_str(), "activating");
        assert_eq!(
            state.get_auth_wechat_status().as_str(),
            "登录成功，正在加载账号数据"
        );
    }

    #[test]
    fn network_recovery_stays_idle_while_login_dialog_is_open() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_session_state("signed_out".into());
        state.set_auth_open(true);

        assert!(!network_recovery_allowed(&state));

        state.set_auth_open(false);
        assert!(network_recovery_allowed(&state));
    }
    use crate::runtime::test_support::MemoryRefreshTokenStore;
    use reqwest::Url;

    fn unavailable_finance_snapshot(role: &str, read_only: bool) -> BackendSnapshot {
        BackendSnapshot {
            account: AccountSnapshot {
                read_only,
                capabilities: Vec::new(),
                user: AccountUser {
                    id: "11111111-1111-4111-8111-111111111111".to_string(),
                    email_masked: "u***@example.com".to_string(),
                    nickname: None,
                    status: "active".to_string(),
                    registered_at: "2026-09-05T00:00:00Z".to_string(),
                    invitation_code_submitted: false,
                },
                auth_methods: AccountAuthMethods::default(),
                membership: None,
                billing_group: AccountGroupChoice {
                    group_id: "22222222-2222-4222-8222-222222222222".to_string(),
                    name: "Studio".to_string(),
                    group_status: if read_only { "frozen" } else { "active" }.to_string(),
                    role: role.to_string(),
                    member_id: (role == "member")
                        .then(|| "33333333-3333-4333-8333-333333333333".to_string()),
                    relationship_status: (role == "member").then(|| "active".to_string()),
                    readable_context: true,
                    selectable: !read_only,
                    group_version: "4".to_string(),
                    membership_version: (role == "member").then(|| "8".to_string()),
                    capabilities: Vec::new(),
                    quota: None,
                },
                entitlement: Value::Null,
                credits: None,
                quota: None,
            },
            models: None,
            plans: None,
            packs: None,
            ledger: None,
            ledger_next_cursor: None,
            orders: None,
            owner_billing: None,
            sessions: Vec::new(),
            invitation: None,
        }
    }

    fn owner_catalog_models() -> Vec<ModelCatalogItem> {
        vec![
            ModelCatalogItem {
                code: "openai_image".to_string(),
                version: 3,
                purpose: "image_generation".to_string(),
                name: "OpenAI Image".to_string(),
                capabilities: serde_json::json!({
                    "task_types": ["image_generation", "image_edit"],
                    "supports_masks": true
                }),
                prices: vec![
                    ModelPrice {
                        quality: "1K".to_string(),
                        max_long_edge: Some(1024),
                        credit_cost: "11".to_string(),
                    },
                    ModelPrice {
                        quality: "2K".to_string(),
                        max_long_edge: Some(2048),
                        credit_cost: "22".to_string(),
                    },
                    ModelPrice {
                        quality: "4K".to_string(),
                        max_long_edge: Some(4096),
                        credit_cost: "33".to_string(),
                    },
                ],
            },
            ModelCatalogItem {
                code: "openai_prompt".to_string(),
                version: 2,
                purpose: "prompt_processing".to_string(),
                name: "OpenAI Prompt".to_string(),
                capabilities: serde_json::json!({
                    "task_types": ["image_style_analysis"],
                    "supports_references": true,
                    "operations": ["analyze_style"]
                }),
                prices: vec![ModelPrice {
                    quality: "standard".to_string(),
                    max_long_edge: None,
                    credit_cost: "7".to_string(),
                }],
            },
            ModelCatalogItem {
                code: "seedance".to_string(),
                version: 1,
                purpose: "video_generation".to_string(),
                name: "Seedance".to_string(),
                capabilities: serde_json::json!({"summary": "Video model"}),
                prices: Vec::new(),
            },
        ]
    }

    #[test]
    fn core_prompt_catalog_projection_preserves_saved_gpt_5_6_first_fallback() {
        i_slint_backend_testing::init_no_event_loop();
        // Deliberately place the generic model before both preferred candidates.
        // Literal expectations catch saved-selection loss and wrong fallback priority.
        for (saved, codes, expected) in [
            ("saved_prompt", vec!["first_prompt", "gpt_5_6_sol", "saved_prompt"], "saved_prompt"),
            ("missing_prompt", vec!["first_prompt", "gpt_5_6_sol"], "gpt_5_6_sol"),
            ("missing_prompt", vec!["first_prompt", "second_prompt"], "first_prompt"),
        ] {
            let models = codes.into_iter().map(|code| ModelCatalogItem {
                code: code.into(),
                version: 1,
                purpose: "prompt_processing".into(),
                name: format!("Display {code}"),
                capabilities: serde_json::json!({}),
                prices: Vec::new(),
            }).collect::<Vec<_>>();
            let app = AppWindow::new().expect("create projection fixture");
            let context = AppContext::default();
            let state = app.global::<AppState>();

            let mut prepared = PreparedUiProjection::default();
            prepare_activation_catalog_projection(&mut prepared, &models, "", saved, "");
            state.set_reasoning_model("previous_projection".into());
            prepared.publish(&state);
            assert_eq!(state.get_reasoning_model().as_str(), expected, "prepared / {saved}");
            assert_eq!(state.get_reasoning_model_name().as_str(), format!("Display {expected}"));

            state.set_reasoning_model(saved.into());
            state.set_reasoning_model_name("stale display".into());
            apply_model_catalog_projection(&app, &context, &models);
            assert_eq!(state.get_reasoning_model().as_str(), expected, "refresh / {saved}");
            assert_eq!(state.get_reasoning_model_name().as_str(), format!("Display {expected}"));
        }
    }

    #[test]
    fn member_and_frozen_snapshot_projection_keeps_finance_unavailable() {
        i_slint_backend_testing::init_no_event_loop();
        for snapshot in [
            unavailable_finance_snapshot("member", false),
            unavailable_finance_snapshot("owner", true),
        ] {
            let app = AppWindow::new().expect("create app window");
            let context = AppContext::default();
            let state = app.global::<AppState>();
            let mut owner_snapshot = unavailable_finance_snapshot("owner", false);
            owner_snapshot.models = Some(owner_catalog_models());
            assert_eq!(owner_snapshot.account.user.id, snapshot.account.user.id);
            let owner_projection = project_backend_snapshot(&owner_snapshot);
            apply_model_catalog_projection(
                &app,
                &context,
                owner_projection.models.expect("owner model catalog"),
            );
            state.set_image_editor_model("openai_image".into());
            state.set_image_editor_model_name("OpenAI Image".into());
            state.set_image_editor_price_1k(11);
            state.set_image_editor_price_2k(22);
            state.set_image_editor_price_4k(33);
            state.set_model_catalog_message("owner catalog loaded".into());
            assert_eq!(state.get_image_model().as_str(), "openai_image");
            assert_eq!(state.get_reasoning_model().as_str(), "openai_prompt");
            assert_eq!(state.get_video_model().as_str(), "seedance");
            assert!(state.get_style_analysis_available());
            assert_eq!(state.get_catalog_models().row_count(), 3);
            assert_eq!(context.store.borrow().model_groups.len(), 2);

            let projection = project_backend_snapshot(&snapshot);
            let wallet_balance = projection.credits.map(|credits| credits.available.as_str());
            let wallet_reserved = projection.credits.map(|credits| credits.reserved.as_str());
            assert_eq!(wallet_balance, None);
            assert_eq!(wallet_reserved, None);
            assert_ne!(wallet_balance, Some("0"));
            assert_ne!(wallet_reserved, Some("0"));
            assert!(projection.membership.is_none());
            assert!(projection.plans.is_none());
            assert!(projection.packs.is_none());
            assert!(projection.models.is_none());
            assert!(projection.ledger.is_none());
            assert!(projection.orders.is_none());
            assert!(projection.owner_billing.is_none());

            apply_model_catalog_projection(&app, &context, projection.models.unwrap_or(&[]));

            assert_eq!(state.get_image_model().as_str(), "");
            assert_eq!(state.get_image_model_name().as_str(), "");
            assert_eq!(state.get_image_price_1k(), 0);
            assert_eq!(state.get_image_price_2k(), 0);
            assert_eq!(state.get_image_price_4k(), 0);
            assert_eq!(state.get_image_editor_model().as_str(), "");
            assert_eq!(state.get_image_editor_model_name().as_str(), "");
            assert_eq!(state.get_image_editor_price_1k(), 0);
            assert_eq!(state.get_image_editor_price_2k(), 0);
            assert_eq!(state.get_image_editor_price_4k(), 0);
            assert_eq!(state.get_reasoning_model().as_str(), "");
            assert_eq!(state.get_reasoning_model_name().as_str(), "");
            assert_eq!(state.get_video_model().as_str(), "");
            assert_eq!(state.get_video_model_name().as_str(), "");
            assert_eq!(state.get_video_model_description().as_str(), "");
            assert!(!state.get_video_service_available());
            assert!(!state.get_style_analysis_available());
            assert_eq!(state.get_style_analysis_model_code().as_str(), "");
            assert_eq!(state.get_style_analysis_display_name().as_str(), "");
            assert_eq!(state.get_style_analysis_credit_cost().as_str(), "");
            assert_eq!(state.get_catalog_models().row_count(), 0);
            assert_eq!(state.get_video_model_options().row_count(), 0);
            assert_eq!(state.get_model_groups().row_count(), 0);
            assert_eq!(state.get_model_image_options().row_count(), 0);
            assert_eq!(state.get_model_reasoning_options().row_count(), 0);
            assert!(context.store.borrow().model_groups.is_empty());
            assert_eq!(state.get_model_catalog_message().as_str(), "");
        }
    }

    #[test]
    fn signing_out_removes_credentials_from_the_previous_account() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_auth_email("previous-account@example.com".into());
        state.set_auth_password("PreviousPass1!".into());

        sign_out_locally(&app, &AppContext::default(), false, None);

        assert_eq!(state.get_auth_email().as_str(), "");
        assert_eq!(state.get_auth_password().as_str(), "");
    }

    #[test]
    fn registration_continuation_callback_does_not_finish_login() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let api = ApiClient::new(
            ApiClientConfig {
                base_url: Url::parse("http://127.0.0.1:9/").unwrap(),
                app_version: "1.2.3".to_string(),
                timeout: Duration::from_secs(1),
            },
            DeviceIdentity {
                id: "callback-device".to_string(),
                name: "callback-test".to_string(),
                platform: "macos".to_string(),
            },
            Arc::new(SessionManager::new(Arc::new(
                MemoryRefreshTokenStore::default(),
            ))),
        )
        .unwrap();
        let context = AppContext {
            backend: Some(Arc::new(BackendRuntime { api: api.clone() })),
            ..AppContext::default()
        };
        let operation_epoch = begin_auth_operation(&context);
        let state = app.global::<AppState>();
        state.set_auth_open(true);
        state.set_auth_method("email".into());
        state.set_auth_email_mode("code".into());
        state.set_session_state("authenticating".into());
        let pending_registration = Rc::new(RefCell::new(None));
        let session_epoch = api.session().auth_epoch();

        handle_login_success(
            &app,
            &context,
            operation_epoch,
            "code",
            LoginWorkerResponse::Email { outcome: EmailLoginOutcome::TeamRegistrationRequired {
                registration_continuation: SecretString::new(
                    "opaque-continuation".to_string(),
                ),
                continuation_expires_at: "2026-09-05T10:05:00Z".to_string(),
                invitations: vec![TeamRegistrationInvitationSummary {
                    invitation_id: "11111111-1111-4111-8111-111111111111".to_string(),
                    group_id: "22222222-2222-4222-8222-222222222222".to_string(),
                    team_name: "Studio Team".to_string(),
                    owner_display_name: "Owner".to_string(),
                    recipient_email_masked: "m***@example.com".to_string(),
                    monthly_limit: "500".to_string(),
                    status: "pending".to_string(),
                    expires_at: "2026-09-11T10:00:00Z".to_string(),
                    version: "1".to_string(),
                }],
                pending_invitation_count: 1,
                selection_state: TeamRegistrationSelectionState::Unique,
            }, email: SecretString::new("verified@example.com".into()), acceptances: Vec::new() },
            &pending_registration,
        );

        assert_eq!(api.session().auth_epoch(), session_epoch);
        assert!(api.session().access().is_none());
        assert!(!state.get_logged_in());
        assert!(context
            .current_user_id
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .is_none());
        assert_eq!(
            state.get_session_state().as_str(),
            "team_registration_required"
        );
        assert!(state.get_auth_open());
        assert_eq!(
            pending_registration
                .borrow()
                .as_ref()
                .expect("pending registration")
                .registration_continuation
                .expose(),
            "opaque-continuation"
        );
    }

    #[test]
    fn disconnected_worker_is_not_mistaken_for_a_pending_result() {
        let (sender, receiver) = mpsc::channel::<u8>();
        drop(sender);
        let receiver = Rc::new(RefCell::new(Some(receiver)));

        assert_eq!(poll_receiver(&receiver), ReceiverPoll::Disconnected);
        assert_eq!(poll_receiver(&receiver), ReceiverPoll::Disconnected);
    }
    #[test]
    fn core_idle_registration_mode_change_disposes_retained_secrets_without_submission() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let context = AppContext::default();
        let epoch = begin_auth_operation(&context);
        let pending = Rc::new(RefCell::new(Some(PendingRegistrationOutcome {
            normalized_email: SecretString::new("fixture@example.invalid".into()),
            agreement_acceptances: vec![], idempotency_key: "exact-registration-key".into(),
            password: Some(SecretString::new("FixturePassword1".into())), auth_operation_epoch: epoch,
            registration_continuation: SecretString::new("fixture-continuation".into()),
            continuation_expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            invitations: vec![], pending_invitation_count: 1, selection_state: TeamRegistrationSelectionState::Unique,
        })));
        let state = app.global::<AppState>();
        state.set_auth_open(true); state.set_auth_method("email".into()); state.set_auth_email_mode("code".into());
        state.set_team_registration_open(true); state.set_team_registration_password("not retained".into());
        wire_team_registration_callbacks(&app, context.clone(), pending.clone());
        state.set_auth_method("wechat".into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        slint::platform::update_timers_and_animations();
        assert!(pending.borrow().is_none());
        assert!(!state.get_team_registration_open());
        assert!(state.get_team_registration_password().is_empty());
        assert!(!auth_operation_is_current(&context, epoch));
    }
    #[test]
    fn core_auth_operation_counter_exhaustion_never_reuses_completion_authority() {
        let context = AppContext::default();
        context.auth_operation_epoch.store(u64::MAX - 1, Ordering::SeqCst);
        let first = begin_auth_operation(&context);
        assert!(!auth_operation_is_current(&context, first));
        invalidate_auth_operations(&context);
        let next = begin_auth_operation(&context);
        assert!(!auth_operation_is_current(&context, next));
        assert_eq!(context.auth_operation_epoch.load(Ordering::SeqCst), u64::MAX);
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
        let error = |status, code: &str| ApiError::Http {
            status,
            code: code.to_string(),
            message: "test".to_string(),
            request_id: None,
            details: None,
        };
        assert_eq!(
            startup_error_disposition(&error(401, "refresh_token_reused"), true),
            StartupErrorDisposition::TerminalSession
        );
        for offline_available in [false, true] {
            assert_eq!(
                startup_error_disposition(&error(426, "client_upgrade_required"), offline_available),
                StartupErrorDisposition::UpdateRequired
            );
            assert_eq!(
                startup_error_disposition(&error(401, "client_upgrade_required"), offline_available),
                StartupErrorDisposition::Recoverable
            );
        }
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

    #[test]
    fn video_model_selector_preserves_an_available_preference_then_falls_back() {
        let models = vec![
            ModelCatalogItem {
                code: "seedance-lite".to_string(),
                version: 1,
                purpose: "video_generation".to_string(),
                name: "Seedance Lite".to_string(),
                capabilities: serde_json::json!({}),
                prices: vec![],
            },
            ModelCatalogItem {
                code: "seedance-pro".to_string(),
                version: 1,
                purpose: "video_generation".to_string(),
                name: "Seedance Pro".to_string(),
                capabilities: serde_json::json!({}),
                prices: vec![],
            },
            ModelCatalogItem {
                code: "image-only".to_string(),
                version: 1,
                purpose: "image_generation".to_string(),
                name: "Image Only".to_string(),
                capabilities: serde_json::json!({}),
                prices: vec![],
            },
        ];

        assert_eq!(
            select_video_catalog_model(&models, "seedance-pro").map(|model| model.code.as_str()),
            Some("seedance-pro")
        );
        assert_eq!(
            select_video_catalog_model(&models, "missing").map(|model| model.code.as_str()),
            Some("seedance-lite")
        );
    }
}

#[cfg(test)]
mod core_captured_snapshot_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};

    const OWNER: &str = "11111111-1111-4111-8111-111111111111";
    const GROUP: &str = "22222222-2222-4222-8222-222222222222";
    const OTHER: &str = "33333333-3333-4333-8333-333333333333";

    fn account(group: &str) -> Value {
        serde_json::json!({
            "user":{"id":OWNER,"email_masked":"s***@example.com","nickname":"Server user",
                "status":"active","registered_at":"2026-09-01T00:00:00Z","invitation_code_submitted":false},
            "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},
            "credits":null,"quota":null,
            "billing_group":{"group_id":group,"name":"Selected group","group_status":"active","role":"member",
                "member_id":OTHER,"relationship_status":"active","readable_context":true,"selectable":true,
                "group_version":"1","membership_version":"1","capabilities":["bill"],"quota":null}
        })
    }
    struct Server {
        url: String,
        release: Option<mpsc::Sender<()>>,
        seen: mpsc::Receiver<()>,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Server {
        fn new(code: Option<&'static str>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let (release, held) = mpsc::channel();
            let (observed, seen) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let worker = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(15);
                let mut account_seen = false;
                while !stopping.load(Ordering::SeqCst) && Instant::now() < deadline {
                    let (mut stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2)); continue;
                        }
                        Err(error) => panic!("fixture accept: {error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let request = read_headers(&mut stream);
                    let Some(request) = request else { continue };
                    let path = request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                    assert!(request.starts_with("GET "));
                    let (status, body) = match path {
                        "/v1/account" => {
                            assert!(!account_seen); account_seen = true;
                            assert!(request.to_lowercase().contains(&format!("x-account-group-id: {GROUP}")));
                            let _ = observed.send(());
                            held.recv_timeout(Duration::from_secs(6)).expect("fixture release");
                            match code {
                                Some(code) => (
                                    if code == "client_upgrade_required" { 426 } else { 401 },
                                    serde_json::json!({"request_id":"fixture","data":null,"error":{
                                        "code":code,"message":"private fixture body","details":{"minimum_version":"9999.0.0"}},"meta":null}),
                                ),
                                None => (200, envelope(account(GROUP))),
                            }
                        }
                        "/v1/models" => {
                            assert!(request.to_lowercase().contains(&format!("x-account-group-id: {GROUP}")));
                            (200, envelope(serde_json::json!({"items":[
                                {"code":"first","version":1,"purpose":"prompt_processing","name":"First","capabilities":{},"prices":[]},
                                {"code":"later","version":1,"purpose":"prompt_processing","name":"Later","capabilities":{},"prices":[]}
                            ]})))
                        }
                        "/v1/account/sessions" => {
                            assert!(!request.to_lowercase().contains("x-account-group-id:"));
                            (200, envelope(serde_json::json!({"items":[]})))
                        }
                        "/v1/account/invitation" => {
                            assert!(!request.to_lowercase().contains("x-account-group-id:"));
                            (403, serde_json::json!({"request_id":"fixture","data":null,"error":{
                                "code":"permission_denied","message":"invitation unavailable","details":null},"meta":null}))
                        }
                        _ => panic!("unexpected captured snapshot route: {path}"),
                    };
                    let body = body.to_string();
                    // The client may have cancelled an obsolete request.
                    let _ = write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }
            });
            Self { url, release: Some(release), seen, stop, worker: Some(worker) }
        }
        fn release(&mut self) { if let Some(tx) = self.release.take() { let _ = tx.send(()); } }
        fn finish(&mut self) -> std::thread::Result<()> {
            self.release(); self.stop.store(true, Ordering::SeqCst);
            self.worker.take().map(|worker| worker.join()).unwrap_or(Ok(()))
        }
    }
    impl Drop for Server { fn drop(&mut self) { let result = self.finish(); if !std::thread::panicking() { result.unwrap(); } } }
    fn envelope(data: Value) -> Value {
        serde_json::json!({"request_id":"fixture","data":data,"error":null,"meta":null})
    }
    fn read_headers(stream: &mut TcpStream) -> Option<String> {
        let mut bytes = Vec::new();
        while bytes.len() <= 16 * 1024 {
            let mut chunk = [0u8; 2048];
            let n = match stream.read(&mut chunk) {
                Ok(0) if bytes.is_empty() => return None,
                Ok(n) => n,
                Err(error) if bytes.is_empty() && matches!(error.kind(),std::io::ErrorKind::TimedOut|std::io::ErrorKind::WouldBlock) => return None,
                Err(error) => panic!("fixture read: {error}"),
            };
            assert!(n > 0); bytes.extend_from_slice(&chunk[..n]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                return Some(String::from_utf8(bytes).unwrap());
            }
        }
        panic!("fixture headers exceed bound");
    }

    struct Fixture {
        inner: video_image_callbacks::tests::scoped_inputs::Fixture,
        server: Server,
    }
    impl std::ops::Deref for Fixture {
        type Target = video_image_callbacks::tests::scoped_inputs::Fixture;
        fn deref(&self) -> &Self::Target { &self.inner }
    }
    impl Fixture {
        fn new(code: Option<&'static str>) -> (Self, AppWindow) {
            let server = Server::new(code);
            let mut inner = video_image_callbacks::tests::scoped_inputs::Fixture::new();
            let backend = Arc::new(BackendRuntime { api: ApiClient::new(ApiClientConfig {
                base_url:reqwest::Url::parse(&server.url).unwrap(),app_version:"999.0.0".into(),timeout:Duration::from_secs(2),
            },DeviceIdentity{id:OTHER.into(),name:"snapshot fixture".into(),platform:"macos".into()},
                inner.context.backend.as_ref().unwrap().api.session().clone()).unwrap() });
            backend.api.bind_user_work(UserWorkAdmission::new(inner.context.active_namespace.clone(),inner.context.user_activity.clone())).unwrap();
            let p = PrivatePersistence::for_test_with_storage((*inner.writer).clone(),inner.persistence.lease().clone(),
                inner.context.user_activity.clone(),backend.api.upgrade_latch().clone(),inner.context.data_root_capability.clone().unwrap(),
                backend.api.clone(),inner.context.file_index.clone().unwrap());
            inner.context.backend=Some(backend); inner.persistence=p.clone(); inner.context.store.borrow_mut().private_persistence=Some(p);
            inner.authority=inner.persistence.storage_authority().unwrap();
            let transition=inner.context.namespace_operations.try_begin_transition().unwrap();
            let phase=transition.begin_prepublication_recovery(inner.persistence.lease()).unwrap();
            phase.verify_no_unsupported_imports(&inner.authority).unwrap();
            let recovered=phase.finish().unwrap();
            transition.prepare_publication(inner.persistence.lease(),recovered).unwrap().publish();
            let f=Self{inner,server}; let app=AppWindow::new().unwrap();
            let state=app.global::<AppState>(); state.set_logged_in(true); state.set_session_state("online".into());
            state.set_auth_method("email".into()); state.set_page("generation".into()); state.set_asset_type("character".into());
            state.set_nickname("Initial user".into()); state.set_reasoning_model("first".into()); state.set_generation_status("Existing result".into());
            f.persistence.save_profile(user_profile_data(&app)).unwrap(); f.publish_group(GROUP);
            (f,app)
        }
        fn publish_group(&self, group: &str) {
            let manager=&self.context.billing_context;
            let session=self.context.current_account_session_scope().unwrap();
            if manager.confirmed_scope().is_none(){manager.bind_authenticated_session(session.clone()).unwrap();}
            let ticket=manager.begin_switch(&session,"snapshot-device",group,PreviousBillingAuthority::StillValid).unwrap();
            let snapshot:AccountSnapshot=serde_json::from_value(account(group)).unwrap();
            let staged=manager.stage_confirmation(&ticket,snapshot.billing_group.clone(),snapshot).unwrap();
            self.writer.save_selected_group(OWNER,"snapshot-device",group).unwrap();
            manager.publish_persisted(ticket,staged);
        }
        fn profile(&self) -> UserProfileData {
            self.writer.load_client_user_profile_for_namespace(self.persistence.lease()).unwrap().unwrap()
        }
        fn reject_profile(&self) {
            let root=self.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
            let db=root.join("fixture.sqlite3"); assert!(db.is_file());
            // Explicit fixture-only SQLite connection: never a production data-root lookup.
            rusqlite::Connection::open(db).unwrap().execute_batch(
                "CREATE TRIGGER snapshot_reject_profile BEFORE INSERT ON user_settings WHEN NEW.key='user_profile' BEGIN SELECT RAISE(ABORT,'fixture profile rejection'); END;"
            ).unwrap();
        }
        fn release_and_pump(&mut self, predicate: impl FnMut()->bool) {
            self.server.release(); video_image_callbacks::tests::scoped_inputs::pump(predicate);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.release();
            let delivery=drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
            let transport=self.server.finish();
            let quiesce=self.context.user_activity.begin_quiesce(self.persistence.lease()).map(|guard|guard.retire());
            if !std::thread::panicking(){delivery.unwrap();transport.unwrap();quiesce.unwrap();}
        }
    }
    fn tick_for(duration: Duration) {
        let end=Instant::now()+duration;
        while Instant::now()<end {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations(); std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn core_captured_snapshot_current_profile_ack_preserves_later_private_edits() {
        i_slint_backend_testing::init_no_event_loop();
        let(mut f,app)=Fixture::new(None);
        refresh_backend_snapshot_captured(&app,f.context.clone(),f.persistence.clone());
        f.server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
        app.global::<AppState>().set_nickname("Later private edit".into());
        app.global::<AppState>().set_reasoning_model("later".into());
        app.global::<AppState>().set_asset_type("scene".into());
        f.release_and_pump(||!app.global::<AppState>().get_credit_ledger_loading());
        let saved=f.profile();
        assert_eq!(saved.nickname,"Later private edit");
        assert_eq!(saved.asset_type,"scene");
        assert_eq!(saved.email_mask,"s***@example.com");
        assert_eq!(app.global::<AppState>().get_reasoning_model().as_str(),"later");
        assert_eq!(app.global::<AppState>().get_generation_status().as_str(),"Existing result");
    }
    #[test]
    fn core_captured_snapshot_late_binding_billing_and_epoch_cannot_publish() {
        i_slint_backend_testing::init_no_event_loop();
        for case in 0..3 {
            let(mut f,app)=Fixture::new(None);
            refresh_backend_snapshot_captured(&app,f.context.clone(),f.persistence.clone());
            f.server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
            match case {
                0 => {
                    let p=PrivatePersistence::for_test((*f.writer).clone(),f.persistence.lease().clone(),
                        f.context.user_activity.clone(),f.context.backend.as_ref().unwrap().api.upgrade_latch().clone());
                    f.context.store.borrow_mut().private_persistence=Some(p);
                },
                1 => f.publish_group(OTHER),
                _ => { let mut store=f.context.store.borrow_mut(); store.credit_sync_epoch=store.credit_sync_epoch.checked_add(1).unwrap(); },
            }
            app.global::<AppState>().set_nickname("Replacement projection".into());
            app.global::<AppState>().set_generation_status("Newer status".into());
            f.server.release(); tick_for(Duration::from_millis(300));
            assert_eq!(app.global::<AppState>().get_nickname().as_str(),"Replacement projection","case {case}");
            assert_eq!(app.global::<AppState>().get_generation_status().as_str(),"Newer status","case {case}");
            assert_eq!(f.profile().nickname,"Initial user","case {case}");
        }
    }
    #[test]
    fn core_captured_snapshot_terminal_and_exact_upgrade_keep_original_authority() {
        i_slint_backend_testing::init_no_event_loop();
        for code in ["session_invalid","client_upgrade_required"] {
            let(mut f,app)=Fixture::new(Some(code));
            refresh_backend_snapshot_captured(&app,f.context.clone(),f.persistence.clone());
            f.server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
            f.server.release();
            if code=="client_upgrade_required" {
                video_image_callbacks::tests::scoped_inputs::pump(||f.context.backend.as_ref().unwrap().api.upgrade_latch().is_tripped());
                tick_for(Duration::from_millis(150));
                assert_eq!(app.global::<AppState>().get_generation_status().as_str(),"Existing result");
                assert_eq!(f.profile().nickname,"Initial user");
            } else {
                video_image_callbacks::tests::scoped_inputs::pump(||!app.global::<AppState>().get_logged_in());
                assert!(f.context.current_user_id.lock().unwrap().is_none());
            }
        }
    }
    #[test]
    fn core_captured_snapshot_failed_profile_ack_does_not_report_durable_success() {
        i_slint_backend_testing::init_no_event_loop();
        let(mut f,app)=Fixture::new(None); f.reject_profile();
        refresh_backend_snapshot_captured(&app,f.context.clone(),f.persistence.clone());
        f.server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
        f.release_and_pump(||!app.global::<AppState>().get_credit_ledger_loading());
        assert_eq!(f.profile().nickname,"Initial user");
        assert!(app.global::<AppState>().get_generation_status().as_str().contains("未能安全保存"));
        assert!(!app.global::<AppState>().get_generation_status().as_str().contains("private fixture"));
    }

    #[test]
    fn core_captured_snapshot_sent_payload_and_profile_ack_wait_for_real_worker_exit() {
        i_slint_backend_testing::init_no_event_loop();
        for hold_profile in [false, true] {
            let(mut f,app)=Fixture::new(None);
            let (release, waiting)=mpsc::channel();
            let (entered, observed)=mpsc::channel();
            struct Release(Option<mpsc::Sender<()>>);
            impl Drop for Release { fn drop(&mut self){if let Some(tx)=self.0.take(){let _=tx.send(());}} }
            let mut release=Release(Some(release));
            let hook=move||{let _=entered.send(());let _=waiting.recv_timeout(Duration::from_secs(6));};
            if !hold_profile {
                set_delivery_preparation_after_send_for_test(hook);
                refresh_backend_snapshot_captured(&app,f.context.clone(),f.persistence.clone());
            } else {
                refresh_backend_snapshot_captured(&app,f.context.clone(),f.persistence.clone());
                // The original GET worker already consumed its one-shot slot;
                // this hook belongs to the real next profile-ack waiter.
                set_delivery_preparation_after_send_for_test(hook);
            }
            f.server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
            f.server.release();
            let reached=Cell::new(false);
            video_image_callbacks::tests::scoped_inputs::pump(||{
                if observed.try_recv().is_ok(){reached.set(true);}
                reached.get()
            });
            tick_for(Duration::from_millis(100));
            assert!(app.global::<AppState>().get_credit_ledger_loading(),"a sent payload is not a joined worker");
            if hold_profile {
                assert_eq!(f.profile().nickname,"Server user","the real writer acknowledged before the hook");
                app.global::<AppState>().set_nickname("Edited after actual ack".into());
                app.global::<AppState>().set_generation_status("Later generation".into());
            } else {
                assert_eq!(app.global::<AppState>().get_nickname().as_str(),"Initial user");
                assert_eq!(f.profile().nickname,"Initial user","GET payload must not stage before join");
            }
            release.0.take().unwrap().send(()).unwrap();
            video_image_callbacks::tests::scoped_inputs::pump(||!app.global::<AppState>().get_credit_ledger_loading());
            if hold_profile {
                assert_eq!(app.global::<AppState>().get_nickname().as_str(),"Edited after actual ack");
                assert_eq!(app.global::<AppState>().get_generation_status().as_str(),"Later generation");
            } else {
                assert_eq!(f.profile().nickname,"Server user");
            }
        }
    }

    #[test]
    fn core_captured_snapshot_after_send_stale_binding_and_epoch_reject_after_join() {
        i_slint_backend_testing::init_no_event_loop();
        // Keep the original test name; cover each original authority boundary
        // after a real payload send and actual registered handle join.
        for case in 0..4 {
            let(mut f,app)=Fixture::new((case == 3).then_some("client_upgrade_required"));
            let (release,waiting)=mpsc::channel();
            let (entered,observed)=mpsc::channel();
            struct Release(Option<mpsc::Sender<()>>);
            impl Drop for Release { fn drop(&mut self){if let Some(tx)=self.0.take(){let _=tx.send(());}} }
            // Declared after the fixture: unwind releases the actual worker before drain.
            let mut release=Release(Some(release));
            set_delivery_preparation_after_send_for_test(move||{
                let _=entered.send(());
                let _=waiting.recv_timeout(Duration::from_secs(6));
            });
            refresh_backend_snapshot_captured(&app,f.context.clone(),f.persistence.clone());
            f.server.seen.recv_timeout(Duration::from_secs(3)).unwrap();
            f.server.release();
            let reached=Cell::new(false);
            video_image_callbacks::tests::scoped_inputs::pump(||{
                if observed.try_recv().is_ok(){reached.set(true);}
                reached.get()
            });
            // The real GET result is already in its channel, but its handle is held.
            match case {
                0 => {
                    let replacement=PrivatePersistence::for_test((*f.writer).clone(),f.persistence.lease().clone(),
                        f.context.user_activity.clone(),f.context.backend.as_ref().unwrap().api.upgrade_latch().clone());
                    f.context.store.borrow_mut().private_persistence=Some(replacement);
                },
                1 => {
                    let mut store=f.context.store.borrow_mut();
                    store.credit_sync_epoch=store.credit_sync_epoch.checked_add(1).unwrap();
                },
                2 => f.publish_group(OTHER),
                _ => assert!(f.context.backend.as_ref().unwrap().api.upgrade_latch().is_tripped()),
            }
            app.global::<AppState>().set_nickname("Replacement stays current".into());
            app.global::<AppState>().set_generation_status("Newer operation stays current".into());
            release.0.take().unwrap().send(()).unwrap();
            // Join before pumping the old real timer; never restore the old target.
            drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease()).unwrap();
            tick_for(Duration::from_millis(150));
            assert_eq!(app.global::<AppState>().get_nickname().as_str(),"Replacement stays current");
            assert_eq!(app.global::<AppState>().get_generation_status().as_str(),"Newer operation stays current");
            assert_eq!(f.profile().nickname,"Initial user","stale sent payload must not enqueue profile");
            if case == 2 {
                assert_eq!(f.context.billing_context.confirmed_scope().unwrap().request.account_group_id,OTHER);
            }
            if case == 3 {
                assert!(f.context.backend.as_ref().unwrap().api.upgrade_latch().is_tripped());
            }
        }
    }

}
