//! Privacy-preserving account-center presentation, independent of transport.
use super::*;

pub(super) fn own_quota<'a>(choice: &'a AccountGroupChoice, snapshot: Option<&'a AccountSnapshot>) -> Option<&'a api::QuotaSummary> {
    if !choice.has_capability(KnownCapability::ReadOwnQuota) { return None; }
    snapshot.filter(|value| value.billing_group.group_id == choice.group_id)
        .and_then(|value| value.quota.as_ref()).or(choice.quota.as_ref())
}

pub(super) fn prepare_summary(ui: &mut PreparedUiProjection, groups: &[AccountGroupChoice], snapshot: Option<&AccountSnapshot>) {
    let choice = snapshot.map(|value| &value.billing_group);
    let quota = choice.and_then(|choice| own_quota(choice, snapshot));
    ui.push(choice.map(|value| value.name.clone()).unwrap_or_default().into(), |state, value| state.set_team_current_name(value));
    ui.push(choice.map(|value| value.role.clone()).unwrap_or_default(), |state, role| {
        state.set_team_role_label(match (role.as_str(), state.get_en()) {
            ("owner", true) => "Owner", ("owner", false) => "主账号", ("member", true) => "Member", ("member", false) => "成员", _ => "",
        }.into());
    });
    ui.push(choice.is_some_and(|value| value.has_capability(KnownCapability::ReadOwnQuota)), |state, value| state.set_team_has_own_quota(value));
    ui.push(quota.map(|value| value.monthly_limit.clone()).unwrap_or_default().into(), |state, value| state.set_team_quota_monthly(value));
    ui.push(quota.map(|value| value.settled.clone()).unwrap_or_default().into(), |state, value| state.set_team_quota_settled(value));
    ui.push(quota.map(|value| value.reserved.clone()).unwrap_or_default().into(), |state, value| state.set_team_quota_reserved(value));
    ui.push(quota.map(|value| value.remaining.clone()).unwrap_or_default().into(), |state, value| state.set_team_quota_remaining(value));
    ui.push(groups.iter().any(|value| value.role == "member" && value.member_id.is_some()), |state, value| state.set_team_joined_slot_occupied(value));
}

pub(super) fn clear_form(state: &AppState) {
    state.set_team_form_needs_review(false);
    state.set_team_form_token("".into());
    state.set_team_form_action("".into());
    state.set_team_form_id("".into());
    state.set_team_form_version("".into());
    state.set_team_form_label("".into());
    state.set_team_limit_input("0".into());
}

pub(super) fn clear_forms_and_feedback(state: &AppState) {
    clear_form(state);
    state.set_team_invite_email("".into());
    state.set_team_invite_limit_input("0".into());
    state.set_team_page_error("".into());
    state.set_team_page_status("".into());
}

pub(super) fn clear_pages(state: &AppState) {
    state.set_team_members(ModelRc::new(VecModel::default()));
    state.set_team_invitations(ModelRc::new(VecModel::default()));
    state.set_pending_team_invitations(ModelRc::new(VecModel::default()));
    state.set_team_usage(ModelRc::new(VecModel::default()));
    state.set_team_members_next_cursor("".into());
    state.set_team_invitations_next_cursor("".into());
    state.set_pending_team_invitations_next_cursor("".into());
    state.set_team_usage_next_cursor("".into());
    state.set_team_tab("accounts".into());
}

pub(super) fn invitation_actionable(status: &str, expiry: &str) -> bool {
    status == "pending" && chrono::DateTime::parse_from_rfc3339(expiry)
        .is_ok_and(|value| value > chrono::Utc::now())
}

pub(super) fn display_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| "时间待确认".into())
}

pub(super) fn operation_label(value: &str) -> &'static str {
    match value {
        "image_generation" => "图片生成", "video_generation" => "视频生成",
        "prompt_optimization" | "prompt_enhancement" => "提示词优化",
        "image_enhancement" | "image_upscale" => "图片增强", "image_edit" | "image_editing" => "图片编辑",
        "image_cutout" => "智能抠图", "background_removal" => "移除背景",
        _ => "创作任务",
    }
}
pub(super) fn phase_label(value: &str) -> &'static str {
    match value { "reserved" | "reservation" => "预留", "settled" | "settlement" => "已结算", "released" | "release" => "已释放", "refunded" => "已退回", _ => "处理中" }
}
pub(super) fn outcome_label(value: &str) -> &'static str {
    match value { "succeeded" | "success" => "成功", "failed" | "failure" => "失败", "cancelled" | "canceled" => "已取消", "pending" | "processing" => "处理中", "expired" => "已过期", _ => "待确认" }
}
pub(super) fn success_label(action: &str) -> &'static str {
    match action {
        "rename" => "团队名称已更新", "create" => "邀请已发送", "resend" => "邀请已重新发送",
        "revoke" => "邀请已撤销", "accept" => "已加入团队，可在上方账号卡片中切换", "decline" => "已拒绝邀请",
        "set_limit" => "成员额度已更新", "suspend" => "成员已暂停", "resume" => "成员已恢复",
        "remove" => "成员已移除", "leave" => "已退出团队", _ => "操作已完成",
    }
}
