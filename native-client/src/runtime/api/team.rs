//! Canonical wire DTOs for team account operations.

use super::{
    deserialize_secret_string, AgreementAcceptance, ApiError, LoginResponse, LoginUser,
    SecretString, TokenSet,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KnownCapability {
    Bill,
    ReadOwnQuota,
    LeaveTeam,
    ReadGroupFinance,
    Purchase,
    Redeem,
    ManageGroup,
    ManageInvitations,
    ManageMembers,
    ReadGroupUsage,
}

impl KnownCapability {
    pub(crate) fn as_wire(self) -> &'static str {
        match self {
            Self::Bill => "bill",
            Self::ReadOwnQuota => "read_own_quota",
            Self::LeaveTeam => "leave_team",
            Self::ReadGroupFinance => "read_group_finance",
            Self::Purchase => "purchase",
            Self::Redeem => "redeem",
            Self::ManageGroup => "manage_group",
            Self::ManageInvitations => "manage_invitations",
            Self::ManageMembers => "manage_members",
            Self::ReadGroupUsage => "read_group_usage",
        }
    }
}

pub(crate) fn has_known_capability(values: &[String], expected: KnownCapability) -> bool {
    values.iter().any(|value| value == expected.as_wire())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct QuotaSummary {
    pub(crate) period_start: String,
    pub(crate) period_end: String,
    pub(crate) monthly_limit: String,
    pub(crate) settled: String,
    pub(crate) reserved: String,
    pub(crate) remaining: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountGroupChoice {
    pub(crate) group_id: String,
    pub(crate) name: String,
    pub(crate) group_status: String,
    pub(crate) role: String,
    pub(crate) member_id: Option<String>,
    pub(crate) relationship_status: Option<String>,
    pub(crate) readable_context: bool,
    pub(crate) selectable: bool,
    pub(crate) group_version: String,
    pub(crate) membership_version: Option<String>,
    pub(crate) capabilities: Vec<String>,
    pub(crate) quota: Option<QuotaSummary>,
}

impl AccountGroupChoice {
    pub(crate) fn has_capability(&self, expected: KnownCapability) -> bool {
        has_known_capability(&self.capabilities, expected)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemberView {
    pub(crate) member_id: String,
    pub(crate) user_id: String,
    pub(crate) display_name: String,
    pub(crate) email_masked: String,
    pub(crate) status: String,
    pub(crate) monthly_limit: String,
    pub(crate) quota: QuotaSummary,
    pub(crate) joined_at: String,
    pub(crate) version: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InvitationView {
    pub(crate) invitation_id: String,
    pub(crate) group_id: String,
    pub(crate) team_name: String,
    pub(crate) owner_display_name: String,
    pub(crate) recipient_email_masked: String,
    pub(crate) monthly_limit: String,
    pub(crate) status: String,
    pub(crate) expires_at: String,
    pub(crate) version: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerInvitationView {
    pub(crate) invitation_id: String,
    pub(crate) group_id: String,
    pub(crate) team_name: String,
    pub(crate) owner_display_name: String,
    pub(crate) recipient_email_masked: String,
    pub(crate) monthly_limit: String,
    pub(crate) status: String,
    pub(crate) expires_at: String,
    pub(crate) version: String,
    pub(crate) delivery_status: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsageMemberView {
    pub(crate) user_id: String,
    pub(crate) display_name: String,
    pub(crate) email_masked: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsageEventView {
    pub(crate) usage_event_id: String,
    pub(crate) member: UsageMemberView,
    pub(crate) occurred_at: String,
    pub(crate) period_start: String,
    pub(crate) period_end: String,
    pub(crate) operation_kind: String,
    pub(crate) model_label: String,
    pub(crate) credit_amount: String,
    pub(crate) phase: String,
    pub(crate) outcome: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountGroupList {
    pub(crate) items: Vec<AccountGroupChoice>,
    pub(crate) pending_invitation_count: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamItems<T> {
    pub(crate) items: Vec<T>,
}

#[derive(Clone, Debug)]
pub(crate) struct TeamPage<T> {
    pub(crate) items: Vec<T>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ReauthenticationRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    current_password: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email_code: Option<&'a str>,
}

impl<'a> ReauthenticationRequest<'a> {
    pub(crate) fn current_password(value: &'a str) -> Self {
        Self {
            current_password: Some(value),
            email_code: None,
        }
    }

    pub(crate) fn email_code(value: &'a str) -> Self {
        Self {
            current_password: None,
            email_code: Some(value),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReauthenticationCodeResult {
    pub(crate) email_masked: String,
    pub(crate) expires_in_seconds: u64,
    pub(crate) resend_after_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReauthenticationResult {
    pub(crate) user_id: String,
    pub(crate) reauthenticated_at: String,
    pub(crate) expires_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamRegistrationSelectionState {
    Unique,
    Multiple,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TeamRegistrationInvitationSummary {
    pub(crate) invitation_id: String,
    pub(crate) group_id: String,
    pub(crate) team_name: String,
    pub(crate) owner_display_name: String,
    pub(crate) recipient_email_masked: String,
    pub(crate) monthly_limit: String,
    pub(crate) status: String,
    pub(crate) expires_at: String,
    pub(crate) version: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum EmailLoginOutcome {
    Authenticated {
        #[serde(flatten)]
        login: LoginResponse,
    },
    TeamRegistrationRequired {
        #[serde(deserialize_with = "deserialize_secret_string")]
        registration_continuation: SecretString,
        continuation_expires_at: String,
        invitations: Vec<TeamRegistrationInvitationSummary>,
        pending_invitation_count: u64,
        selection_state: TeamRegistrationSelectionState,
    },
}

fn validate_email_login_continuation_keys(value: &serde_json::Value) -> Result<(), ApiError> {
    if value.get("outcome").and_then(serde_json::Value::as_str)
        == Some("team_registration_required")
    {
        const ALLOWED: &[&str] = &[
            "outcome",
            "registration_continuation",
            "continuation_expires_at",
            "invitations",
            "pending_invitation_count",
            "selection_state",
        ];
        let object = value.as_object().ok_or_else(|| ApiError::Protocol {
            message: "团队注册登录响应必须是对象".to_string(),
            request_id: None,
        })?;
        if object
            .keys()
            .any(|key| !ALLOWED.contains(&key.as_str()))
        {
            return Err(ApiError::Protocol {
                message: "团队注册登录响应包含未允许字段".to_string(),
                request_id: None,
            });
        }
    }
    Ok(())
}

pub(crate) fn deserialize_email_login_outcome(
    value: serde_json::Value,
) -> Result<EmailLoginOutcome, ApiError> {
    validate_email_login_continuation_keys(&value)?;
    let outcome: EmailLoginOutcome =
        serde_json::from_value(value).map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
    if let EmailLoginOutcome::TeamRegistrationRequired { invitations, .. } = &outcome {
        if invitations.len() > 50 {
            return Err(ApiError::Protocol {
                message: "团队邀请摘要超过客户端单页上限".to_string(),
                request_id: None,
            });
        }
    }
    Ok(outcome)
}

#[derive(Serialize)]
pub(crate) struct TeamRegistrationRequest<'a> {
    pub(crate) registration_continuation: &'a str,
    pub(crate) password: &'a str,
    pub(crate) agreement_acceptances: &'a [AgreementAcceptance],
    pub(crate) device_id: &'a str,
    pub(crate) device_name: &'a str,
    pub(crate) platform: &'a str,
    pub(crate) app_version: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum TeamRegistrationSessionResult {
    Authenticated {
        #[serde(flatten)]
        tokens: TokenSet,
    },
    LoginRequired {
        session_login_required: bool,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamRegistrationResult {
    pub(crate) user: LoginUser,
    pub(crate) group_choices: Vec<AccountGroupChoice>,
    pub(crate) selection_state: TeamRegistrationSelectionState,
    pub(crate) suggested_account_group_id: Option<String>,
    #[serde(flatten)]
    pub(crate) session: TeamRegistrationSessionResult,
}

fn validate_team_registration_keys(value: &serde_json::Value) -> Result<(), ApiError> {
    const ALLOWED: &[&str] = &[
        "user",
        "group_choices",
        "selection_state",
        "suggested_account_group_id",
        "access_token",
        "access_expires_in_seconds",
        "refresh_token",
        "refresh_expires_at",
        "token_type",
        "session_login_required",
    ];
    let object = value.as_object().ok_or_else(|| ApiError::Protocol {
        message: "团队注册响应必须是对象".to_string(),
        request_id: None,
    })?;
    if object
        .keys()
        .any(|key| !ALLOWED.contains(&key.as_str()))
    {
        return Err(ApiError::Protocol {
            message: "团队注册响应包含未允许字段".to_string(),
            request_id: None,
        });
    }
    Ok(())
}

fn validate_team_registration_session(
    value: &serde_json::Value,
    result: &TeamRegistrationResult,
) -> Result<(), ApiError> {
    let has_token = [
        "access_token",
        "refresh_token",
        "access_expires_in_seconds",
        "refresh_expires_at",
        "token_type",
    ]
    .iter()
    .any(|key| value.get(key).is_some());
    match &result.session {
        TeamRegistrationSessionResult::Authenticated { .. }
            if value.get("session_login_required").is_some() =>
        {
            return Err(ApiError::Protocol {
                message: "团队注册首次响应不得包含重放标记".to_string(),
                request_id: None,
            });
        }
        TeamRegistrationSessionResult::LoginRequired {
            session_login_required: true,
        } if has_token => {
            return Err(ApiError::Protocol {
                message: "团队注册重放响应不得包含会话凭据".to_string(),
                request_id: None,
            });
        }
        TeamRegistrationSessionResult::LoginRequired {
            session_login_required: false,
        } => {
            return Err(ApiError::Protocol {
                message: "团队注册重放响应缺少重新登录标记".to_string(),
                request_id: None,
            });
        }
        _ => {}
    }
    Ok(())
}

fn validate_team_registration_choices(
    result: &TeamRegistrationResult,
) -> Result<(), ApiError> {
    let unique = result
        .group_choices
        .iter()
        .map(|choice| choice.group_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != result.group_choices.len() {
        return Err(ApiError::Protocol {
            message: "团队注册响应包含重复账号组".to_string(),
            request_id: None,
        });
    }
    Ok(())
}

pub(crate) fn deserialize_team_registration_result(
    value: serde_json::Value,
) -> Result<TeamRegistrationResult, ApiError> {
    validate_team_registration_keys(&value)?;
    let result: TeamRegistrationResult =
        serde_json::from_value(value.clone()).map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
    validate_team_registration_session(&value, &result)?;
    validate_team_registration_choices(&result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEST_USER_ID: &str = "33333333-3333-4333-8333-333333333333";

    fn invitation_summary_json() -> serde_json::Value {
        json!({
            "invitation_id": "11111111-1111-4111-8111-111111111111",
            "group_id": "22222222-2222-4222-8222-222222222222",
            "team_name": "Studio Team",
            "owner_display_name": "Owner",
            "recipient_email_masked": "m***@example.com",
            "monthly_limit": "500",
            "status": "pending",
            "expires_at": "2026-09-11T10:00:00Z",
            "version": "1"
        })
    }

    fn invited_login_json(
        invitation_count: usize,
        extra_field: Option<(&str, &str)>,
    ) -> serde_json::Value {
        let mut value = json!({
            "outcome": "team_registration_required",
            "registration_continuation": "opaque-continuation",
            "continuation_expires_at": "2026-09-04T10:05:00Z",
            "invitations": (0..invitation_count)
                .map(|_| invitation_summary_json())
                .collect::<Vec<_>>(),
            "pending_invitation_count": invitation_count,
            "selection_state": "unique"
        });
        if let Some((key, field_value)) = extra_field {
            value
                .as_object_mut()
                .unwrap()
                .insert(key.to_string(), field_value.into());
        }
        value
    }

    fn group_choice_json(capabilities: Vec<&str>) -> serde_json::Value {
        json!({
            "group_id": "22222222-2222-4222-8222-222222222222",
            "name": "Studio Team",
            "group_status": "active",
            "role": "owner",
            "member_id": "44444444-4444-4444-8444-444444444444",
            "relationship_status": "active",
            "readable_context": true,
            "selectable": true,
            "group_version": "1",
            "membership_version": "1",
            "capabilities": capabilities,
            "quota": {
                "period_start": "2026-09-01T00:00:00Z",
                "period_end": "2026-10-01T00:00:00Z",
                "monthly_limit": "500",
                "settled": "12",
                "reserved": "3",
                "remaining": "485"
            }
        })
    }

    fn usage_event_json() -> serde_json::Value {
        json!({
            "usage_event_id": "55555555-5555-4555-8555-555555555555",
            "member": {
                "user_id": TEST_USER_ID,
                "display_name": "Member",
                "email_masked": "m***@example.com"
            },
            "occurred_at": "2026-09-04T10:00:00Z",
            "period_start": "2026-09-01T00:00:00Z",
            "period_end": "2026-10-01T00:00:00Z",
            "operation_kind": "image_generation",
            "model_label": "Model",
            "credit_amount": "4",
            "phase": "settled",
            "outcome": "succeeded"
        })
    }

    #[test]
    fn invited_email_login_has_no_session_tokens() {
        let outcome: EmailLoginOutcome = serde_json::from_value(json!({
            "outcome": "team_registration_required",
            "registration_continuation": "opaque-continuation",
            "continuation_expires_at": "2026-09-04T10:05:00Z",
            "invitations": [invitation_summary_json()],
            "pending_invitation_count": 1,
            "selection_state": "unique"
        }))
        .unwrap();
        assert!(matches!(
            outcome,
            EmailLoginOutcome::TeamRegistrationRequired {
                pending_invitation_count: 1,
                selection_state: TeamRegistrationSelectionState::Unique,
                ..
            }
        ));
    }

    #[test]
    fn invited_email_login_rejects_tokens_and_more_than_fifty_summaries() {
        let with_token = invited_login_json(1, Some(("access_token", "secret")));
        assert!(deserialize_email_login_outcome(with_token).is_err());
        let too_many = invited_login_json(51, None);
        assert!(deserialize_email_login_outcome(too_many).is_err());
    }

    #[test]
    fn usage_projection_rejects_content_fields() {
        let value = usage_event_json();
        let event: UsageEventView = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(event.member.user_id, TEST_USER_ID);
        assert_eq!(event.member.display_name, "Member");
        assert_eq!(event.member.email_masked, "m***@example.com");
        let mut value = value;
        value.as_object_mut().unwrap().insert(
            "task_id".to_string(),
            serde_json::Value::String("private-task".to_string()),
        );
        assert!(serde_json::from_value::<UsageEventView>(value).is_err());
    }

    #[test]
    fn unknown_capability_is_ignored_not_promoted() {
        let choice: AccountGroupChoice = serde_json::from_value(group_choice_json(vec![
            "bill",
            "manage_group",
            "future_capability",
        ]))
        .unwrap();
        assert!(choice.has_capability(KnownCapability::Bill));
        assert!(choice.has_capability(KnownCapability::ManageGroup));
        assert!(!choice.has_capability(KnownCapability::ManageMembers));
    }

    #[test]
    fn reauthentication_body_contains_exactly_one_proof() {
        assert_eq!(
            serde_json::to_value(ReauthenticationRequest::current_password("secret")).unwrap(),
            json!({"current_password": "secret"})
        );
        assert_eq!(
            serde_json::to_value(ReauthenticationRequest::email_code("123456")).unwrap(),
            json!({"email_code": "123456"})
        );
    }

    #[test]
    fn reauthentication_code_result_rejects_session_fields() {
        let exact: ReauthenticationCodeResult = serde_json::from_value(json!({
            "email_masked": "m***@example.com",
            "expires_in_seconds": 300,
            "resend_after_seconds": 60
        }))
        .unwrap();
        assert_eq!(exact.expires_in_seconds, 300);
        let with_token = json!({
            "email_masked": "m***@example.com",
            "expires_in_seconds": 300,
            "resend_after_seconds": 60,
            "access_token": "forbidden"
        });
        assert!(serde_json::from_value::<ReauthenticationCodeResult>(with_token).is_err());
    }
}
