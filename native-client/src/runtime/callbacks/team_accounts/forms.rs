//! A confirmation belongs to one visible form, authenticated namespace and billing scope.
use super::*;

#[derive(Clone)]
struct PendingForm {
    capture: TeamCapture,
    token: SharedString,
    action: String,
    id: SharedString,
    version: SharedString,
}

pub(super) struct TeamForms(Rc<RefCell<Option<PendingForm>>>);

impl TeamForms {
    pub(super) fn rename_version(&self, app: &AppWindow, context: &AppContext) -> Option<String> {
        let state = app.global::<AppState>();
        self.0.borrow().as_ref().filter(|form| form.action == "rename"
            && form.capture.current(context) && state.get_team_panel_visible()
            && !state.get_team_form_needs_review() && state.get_team_form_token() == form.token)
            .map(|form| form.version.to_string())
    }
}

// Metadata may be refreshed without replacing billing authority. Any authority
// difference must still go through the existing coordinator and invalidate forms.
pub(super) fn same_billing_authority(choice: &AccountGroupChoice, snapshot: &AccountSnapshot) -> bool {
    let current = &snapshot.billing_group;
    choice.group_id == current.group_id && choice.group_status == current.group_status
        && choice.role == current.role && choice.member_id == current.member_id
        && choice.relationship_status == current.relationship_status
        && choice.readable_context == current.readable_context && choice.selectable == current.selectable
        && choice.membership_version == current.membership_version
        && choice.capabilities == current.capabilities && choice.capabilities == snapshot.capabilities
}

fn prepare(app: &AppWindow, context: &AppContext, action: &str, id: &str, version: &str) -> Result<PendingForm, ApiError> {
    let state = app.global::<AppState>();
    if !state.get_team_panel_visible() || state.get_team_page_loading() || state.get_account_group_switching() {
        return Err(transition_error("请等待当前操作完成"));
    }
    let mut capture = TeamCapture::new(context)?;
    capture.billing = context.billing_context.confirmed_scope();
    let capability = match action {
        "rename" => Some(KnownCapability::ManageGroup),
        "create" | "resend" | "revoke" => Some(KnownCapability::ManageInvitations),
        "set_limit" | "suspend" | "resume" | "remove" => Some(KnownCapability::ManageMembers),
        "leave" => Some(KnownCapability::LeaveTeam),
        "accept" | "decline" => None,
        _ => return Err(transition_error("不支持的团队操作")),
    };
    if let Some(capability) = capability { context.billing_context.current_scope(capability)?; }
    let mut target_id: SharedString = id.into();
    let mut target_version: SharedString = version.into();
    let label;
    let mut limit = None;
    match action {
        "set_limit" | "suspend" | "resume" | "remove" => {
            let member = state.get_team_members().iter().find(|row| row.member_id == id && row.version == version)
                .ok_or_else(|| transition_error("成员状态已变化，请刷新后重试"))?;
            if !matches!(member.status.as_str(), "active" | "suspended")
                || (action == "resume" && member.status != "suspended")
                || (action == "suspend" && member.status != "active") {
                return Err(transition_error("当前成员状态不支持此操作"));
            }
            label = format!("{} · {}", member.display_name, member.email_masked);
            limit = Some(member.monthly_limit);
        }
        "resend" | "revoke" | "accept" | "decline" => {
            let received = matches!(action, "accept" | "decline");
            let rows = if received { state.get_pending_team_invitations() } else { state.get_team_invitations() };
            let invitation = rows.iter().find(|row| row.invitation_id == id && row.version == version && row.actionable)
                .ok_or_else(|| transition_error("邀请状态已变化，请刷新后重试"))?;
            if action == "accept" && state.get_team_joined_slot_occupied() {
                return Err(transition_error("已加入一个团队，请先退出该团队再接受新的邀请"));
            }
            label = format!("{} · {}", invitation.team_name, invitation.recipient_email_masked);
            limit = Some(invitation.monthly_limit);
        }
        "rename" | "leave" => {
            let snapshot = context.billing_context.confirmed_snapshot().ok_or(ApiError::AuthenticationRequired)?;
            if action == "rename" {
                let groups = context.team_groups.borrow();
                let choice = groups.iter().find(|choice| same_billing_authority(choice, &snapshot))
                    .unwrap_or(&snapshot.billing_group);
                label = choice.name.clone();
                target_id = choice.group_id.clone().into();
                target_version = choice.group_version.clone().into();
                state.set_team_name_input(label.clone().into());
            } else {
                label = snapshot.billing_group.name.clone();
                target_id = snapshot.billing_group.member_id.ok_or(ApiError::AuthenticationRequired)?.into();
                target_version = snapshot.billing_group.membership_version.ok_or(ApiError::AuthenticationRequired)?.into();
            }
        }
        "create" => { label = state.get_team_current_name().to_string(); }
        _ => unreachable!(),
    }
    presentation::clear_form(&state);
    if let Some(limit) = limit { state.set_team_limit_input(limit); }
    let token: SharedString = Uuid::new_v4().to_string().into();
    state.set_team_form_id(target_id.clone());
    state.set_team_form_version(target_version.clone());
    state.set_team_form_label(label.into());
    state.set_team_form_token(token.clone());
    state.set_team_form_action(action.into());
    state.set_team_page_error("".into());
    state.set_team_page_status("".into());
    Ok(PendingForm { capture, token, action: action.into(), id: target_id, version: target_version })
}

pub(super) fn wire(app: &AppWindow, context: AppContext) -> TeamForms {
    let pending: Rc<RefCell<Option<PendingForm>>> = Rc::new(RefCell::new(None));
    let state = app.global::<AppState>();
    let was_switching = Rc::new(Cell::new(state.get_account_group_switching()));
    { let weak = app.as_weak(); let context = context.clone(); let pending = pending.clone();
      state.on_open_team_form(move |action, id, version| {
        let Some(app) = weak.upgrade() else { return; };
        match prepare(&app, &context, &action, &id, &version) {
            Ok(form) => *pending.borrow_mut() = Some(form),
            Err(error) => team_entry_error(&app, &context, error.user_message()),
        }
      }); }
    { let weak = app.as_weak(); let pending = pending.clone();
      state.on_cancel_team_form(move || {
        pending.borrow_mut().take();
        if let Some(app) = weak.upgrade() {
            let state = app.global::<AppState>();
            presentation::clear_form(&state);
            state.set_team_page_error("".into());
        }
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pending = pending.clone();
      state.on_team_form_lifecycle_changed(move || {
        let Some(app) = weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        let finished_switch = was_switching.replace(state.get_account_group_switching()) && !state.get_account_group_switching();
        let invalid = !state.get_team_panel_visible() || state.get_account_group_switching()
            || pending.borrow().as_ref().is_some_and(|form| !form.capture.current(&context)
                || form.capture.billing.as_ref().is_some_and(|billing| billing.request.account_group_id != state.get_selected_account_group_id().as_str()));
        if invalid {
            pending.borrow_mut().take();
            presentation::clear_forms_and_feedback(&state);
        }
        if finished_switch && state.get_team_panel_visible() && state.get_team_can_manage_members() {
            let weak = weak.clone(); let context = context.clone();
            let expected = context.billing_context.confirmed_scope();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(app) = weak.upgrade() {
                    let state = app.global::<AppState>();
                    if state.get_team_panel_visible() && state.get_team_tab() == "accounts"
                        && expected.as_ref().is_some_and(|scope| context.billing_context.is_current(scope)) {
                        state.invoke_load_team_members("".into());
                    }
                }
            });
        }
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pending = pending.clone();
      state.on_review_team_form(move || {
        let Some(app) = weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if state.get_team_page_loading() || state.get_account_group_switching() || !state.get_team_form_needs_review() { return; }
        let Some(form) = pending.borrow().clone() else { return; };
        if !state.get_team_panel_visible() || !form.capture.current(&context) || state.get_team_form_token() != form.token {
            pending.borrow_mut().take(); presentation::clear_form(&state); return;
        }
        let latest_version = match form.action.as_str() {
            "set_limit" | "suspend" | "resume" | "remove" => state.get_team_members().iter()
                .find(|row| row.member_id == form.id).map(|row| row.version),
            "resend" | "revoke" => state.get_team_invitations().iter()
                .find(|row| row.invitation_id == form.id).map(|row| row.version),
            "accept" | "decline" => state.get_pending_team_invitations().iter()
                .find(|row| row.invitation_id == form.id).map(|row| row.version),
            "rename" => context.billing_context.confirmed_snapshot().and_then(|snapshot|
                context.team_groups.borrow().iter().find(|choice| choice.group_id == form.id.as_str()
                    && same_billing_authority(choice, &snapshot)).map(|choice| choice.group_version.clone().into())),
            "create" => Some("".into()),
            _ => None,
        };
        let Some(version) = latest_version else {
            state.set_team_page_error(if state.get_en() { "This target is no longer available. Cancel and reload the page." }
                else { "当前目标已不可用，请取消后重新加载页面。" }.into());
            return;
        };
        if version == form.version && form.action != "create" {
            state.set_team_page_error(if state.get_en() { "Reloading the latest state. Review again when loading finishes." }
                else { "正在重新加载最新状态，完成后请再次查看。" }.into());
            match form.action.as_str() {
                "set_limit" | "suspend" | "resume" | "remove" => state.invoke_load_team_members("".into()),
                "resend" | "revoke" => state.invoke_load_team_invitations("".into()),
                "accept" | "decline" => state.invoke_load_pending_team_invitations("".into()),
                "rename" => state.invoke_refresh_account_groups(),
                _ => (),
            }
            return;
        }
        let limit = state.get_team_limit_input();
        let name = state.get_team_name_input();
        match prepare(&app, &context, &form.action, &form.id, &version) {
            Ok(refreshed) => {
                state.set_team_limit_input(limit);
                state.set_team_name_input(name);
                *pending.borrow_mut() = Some(refreshed);
                state.set_team_page_status(if state.get_en() { "Latest version loaded. Check the target and confirm again." }
                    else { "已载入最新版本，请核对目标后再次确认。" }.into());
            }
            Err(error) => team_entry_error(&app, &context, error.user_message()),
        }
      }); }
    { let weak = app.as_weak(); let context = context.clone(); let pending = pending.clone();
      state.on_confirm_team_form(move || {
        let Some(app) = weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if state.get_team_page_loading() || state.get_account_group_switching() || state.get_team_form_needs_review() { return; }
        let Some(form) = pending.borrow().clone() else { return; };
        if !state.get_team_panel_visible() || !form.capture.current(&context) || state.get_team_form_token() != form.token {
            pending.borrow_mut().take(); presentation::clear_form(&state); return;
        }
        match form.action.as_str() {
            "rename" => state.invoke_rename_team(),
            "create" => state.invoke_create_team_invitation(),
            "resend" => state.invoke_resend_team_invitation(form.id, form.version),
            "revoke" => state.invoke_revoke_team_invitation(form.id, form.version),
            "accept" if !state.get_team_joined_slot_occupied() => state.invoke_accept_team_invitation(form.id, form.version),
            "decline" => state.invoke_decline_team_invitation(form.id, form.version),
            "leave" => state.invoke_leave_team(),
            "set_limit" | "suspend" | "resume" | "remove" => state.invoke_update_team_member(form.id, form.version, form.action.into()),
            _ => team_entry_error(&app, &context, "当前状态不支持此操作，请刷新后重试".into()),
        }
      }); }
    TeamForms(pending)
}
