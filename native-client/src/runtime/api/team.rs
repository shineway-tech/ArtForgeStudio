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
#[serde(deny_unknown_fields)]
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginUserWire {
    id: String,
    email_masked: String,
    nickname: Option<String>,
    status: String,
}

impl From<LoginUserWire> for LoginUser {
    fn from(wire: LoginUserWire) -> Self {
        Self {
            id: wire.id,
            email_masked: wire.email_masked,
            nickname: wire.nickname,
            status: wire.status,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum EmailLoginOutcomeWire {
    Authenticated {
        access_token: String,
        access_expires_in_seconds: u64,
        refresh_token: String,
        refresh_expires_at: String,
        token_type: String,
        is_new_user: bool,
        registration_credit_granted: String,
        user: LoginUserWire,
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

pub(crate) enum EmailLoginOutcome {
    Authenticated {
        login: LoginResponse,
    },
    TeamRegistrationRequired {
        registration_continuation: SecretString,
        continuation_expires_at: String,
        invitations: Vec<TeamRegistrationInvitationSummary>,
        pending_invitation_count: u64,
        selection_state: TeamRegistrationSelectionState,
    },
}

impl<'de> Deserialize<'de> for EmailLoginOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match EmailLoginOutcomeWire::deserialize(deserializer)? {
            EmailLoginOutcomeWire::Authenticated {
                access_token,
                access_expires_in_seconds,
                refresh_token,
                refresh_expires_at,
                token_type,
                is_new_user,
                registration_credit_granted,
                user,
            } => Ok(Self::Authenticated {
                login: LoginResponse {
                    tokens: TokenSet {
                        access_token,
                        access_expires_in_seconds,
                        refresh_token,
                        refresh_expires_at,
                        token_type,
                    },
                    is_new_user,
                    registration_credit_granted,
                    user: user.into(),
                },
            }),
            EmailLoginOutcomeWire::TeamRegistrationRequired {
                registration_continuation,
                continuation_expires_at,
                invitations,
                pending_invitation_count,
                selection_state,
            } => {
                if invitations.len() > 50 {
                    return Err(serde::de::Error::custom(
                        "团队邀请摘要超过客户端单页上限",
                    ));
                }
                Ok(Self::TeamRegistrationRequired {
                    registration_continuation,
                    continuation_expires_at,
                    invitations,
                    pending_invitation_count,
                    selection_state,
                })
            }
        }
    }
}

impl std::fmt::Debug for EmailLoginOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authenticated { .. } => {
                formatter.write_str("EmailLoginOutcome::Authenticated([REDACTED])")
            }
            Self::TeamRegistrationRequired {
                registration_continuation,
                continuation_expires_at,
                invitations,
                pending_invitation_count,
                selection_state,
            } => formatter
                .debug_struct("EmailLoginOutcome::TeamRegistrationRequired")
                .field("registration_continuation", registration_continuation)
                .field("continuation_expires_at", continuation_expires_at)
                .field("invitations", invitations)
                .field("pending_invitation_count", pending_invitation_count)
                .field("selection_state", selection_state)
                .finish(),
        }
    }
}

pub(crate) fn deserialize_email_login_outcome(raw: &str) -> Result<EmailLoginOutcome, ApiError> {
    serde_json::from_str(raw).map_err(|error| ApiError::Protocol {
        message: error.to_string(),
        request_id: None,
    })
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

pub(crate) enum TeamRegistrationSessionResult {
    Authenticated {
        tokens: TokenSet,
    },
    LoginRequired {
        session_login_required: bool,
    },
}

pub(crate) struct TeamRegistrationResult {
    pub(crate) user: LoginUser,
    pub(crate) group_choices: Vec<AccountGroupChoice>,
    pub(crate) selection_state: TeamRegistrationSelectionState,
    pub(crate) suggested_account_group_id: Option<String>,
    pub(crate) session: TeamRegistrationSessionResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamRegistrationAuthenticatedWire {
    user: LoginUserWire,
    group_choices: Vec<AccountGroupChoice>,
    selection_state: TeamRegistrationSelectionState,
    suggested_account_group_id: Option<String>,
    access_token: String,
    access_expires_in_seconds: u64,
    refresh_token: String,
    refresh_expires_at: String,
    token_type: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamRegistrationLoginRequiredWire {
    user: LoginUserWire,
    group_choices: Vec<AccountGroupChoice>,
    selection_state: TeamRegistrationSelectionState,
    suggested_account_group_id: Option<String>,
    session_login_required: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TeamRegistrationResultWire {
    Authenticated(TeamRegistrationAuthenticatedWire),
    LoginRequired(TeamRegistrationLoginRequiredWire),
}

fn has_duplicate_group_choices(choices: &[AccountGroupChoice]) -> bool {
    let unique = choices
        .iter()
        .map(|choice| choice.group_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    unique.len() != choices.len()
}

impl<'de> Deserialize<'de> for TeamRegistrationResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match TeamRegistrationResultWire::deserialize(deserializer)? {
            TeamRegistrationResultWire::Authenticated(wire) => {
                if has_duplicate_group_choices(&wire.group_choices) {
                    return Err(serde::de::Error::custom(
                        "团队注册响应包含重复账号组",
                    ));
                }
                Ok(Self {
                    user: wire.user.into(),
                    group_choices: wire.group_choices,
                    selection_state: wire.selection_state,
                    suggested_account_group_id: wire.suggested_account_group_id,
                    session: TeamRegistrationSessionResult::Authenticated {
                        tokens: TokenSet {
                            access_token: wire.access_token,
                            access_expires_in_seconds: wire.access_expires_in_seconds,
                            refresh_token: wire.refresh_token,
                            refresh_expires_at: wire.refresh_expires_at,
                            token_type: wire.token_type,
                        },
                    },
                })
            }
            TeamRegistrationResultWire::LoginRequired(wire) => {
                if !wire.session_login_required {
                    return Err(serde::de::Error::custom(
                        "团队注册重放响应缺少重新登录标记",
                    ));
                }
                if has_duplicate_group_choices(&wire.group_choices) {
                    return Err(serde::de::Error::custom(
                        "团队注册响应包含重复账号组",
                    ));
                }
                Ok(Self {
                    user: wire.user.into(),
                    group_choices: wire.group_choices,
                    selection_state: wire.selection_state,
                    suggested_account_group_id: wire.suggested_account_group_id,
                    session: TeamRegistrationSessionResult::LoginRequired {
                        session_login_required: true,
                    },
                })
            }
        }
    }
}

impl std::fmt::Debug for TeamRegistrationSessionResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authenticated { .. } => formatter
                .write_str("TeamRegistrationSessionResult::Authenticated([REDACTED])"),
            Self::LoginRequired {
                session_login_required,
            } => formatter
                .debug_struct("TeamRegistrationSessionResult::LoginRequired")
                .field("session_login_required", session_login_required)
                .finish(),
        }
    }
}

impl std::fmt::Debug for TeamRegistrationResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TeamRegistrationResult")
            .field("user", &self.user)
            .field("group_choices", &self.group_choices)
            .field("selection_state", &self.selection_state)
            .field(
                "suggested_account_group_id",
                &self.suggested_account_group_id,
            )
            .field("session", &self.session)
            .finish()
    }
}

pub(crate) fn deserialize_team_registration_result(
    raw: &str,
) -> Result<TeamRegistrationResult, ApiError> {
    serde_json::from_str(raw).map_err(|error| ApiError::Protocol {
        message: error.to_string(),
        request_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
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

    fn quota_json() -> serde_json::Value {
        json!({
            "period_start": "2026-09-01T00:00:00Z",
            "period_end": "2026-10-01T00:00:00Z",
            "monthly_limit": "500",
            "settled": "12",
            "reserved": "3",
            "remaining": "485"
        })
    }

    fn member_json() -> serde_json::Value {
        json!({
            "member_id": "44444444-4444-4444-8444-444444444444",
            "user_id": TEST_USER_ID,
            "display_name": "Member",
            "email_masked": "m***@example.com",
            "status": "active",
            "monthly_limit": "500",
            "quota": quota_json(),
            "joined_at": "2026-09-01T00:00:00Z",
            "version": "1"
        })
    }

    fn owner_invitation_json() -> serde_json::Value {
        let mut value = invitation_summary_json();
        value
            .as_object_mut()
            .unwrap()
            .insert("delivery_status".to_string(), json!("delivered"));
        value
    }

    fn usage_member_json() -> serde_json::Value {
        json!({
            "user_id": TEST_USER_ID,
            "display_name": "Member",
            "email_masked": "m***@example.com"
        })
    }

    fn reauthentication_result_json() -> serde_json::Value {
        json!({
            "user_id": TEST_USER_ID,
            "reauthenticated_at": "2026-09-04T10:00:00Z",
            "expires_at": "2026-09-04T10:05:00Z"
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

    fn login_user_json() -> serde_json::Value {
        json!({
            "id": TEST_USER_ID,
            "email_masked": "m***@example.com",
            "nickname": "Member",
            "status": "active"
        })
    }

    fn authenticated_email_login_json() -> serde_json::Value {
        json!({
            "outcome": "authenticated",
            "access_token": "access-sentinel",
            "access_expires_in_seconds": 900,
            "refresh_token": "refresh-sentinel",
            "refresh_expires_at": "2026-10-04T10:00:00Z",
            "token_type": "Bearer",
            "is_new_user": false,
            "registration_credit_granted": "0",
            "user": login_user_json()
        })
    }

    fn first_registration_json() -> serde_json::Value {
        json!({
            "user": login_user_json(),
            "group_choices": [group_choice_json(vec!["bill", "manage_group"])],
            "selection_state": "unique",
            "suggested_account_group_id": "22222222-2222-4222-8222-222222222222",
            "access_token": "access-sentinel",
            "access_expires_in_seconds": 900,
            "refresh_token": "refresh-sentinel",
            "refresh_expires_at": "2026-10-04T10:00:00Z",
            "token_type": "Bearer"
        })
    }

    fn replay_registration_json() -> serde_json::Value {
        json!({
            "user": login_user_json(),
            "group_choices": [group_choice_json(vec!["read_own_quota"])],
            "selection_state": "unique",
            "suggested_account_group_id": "22222222-2222-4222-8222-222222222222",
            "session_login_required": true
        })
    }

    fn decode_email_value(
        value: serde_json::Value,
    ) -> Result<EmailLoginOutcome, ApiError> {
        let raw = serde_json::to_string(&value).unwrap();
        deserialize_email_login_outcome(&raw)
    }

    fn decode_registration_value(
        value: serde_json::Value,
    ) -> Result<TeamRegistrationResult, ApiError> {
        let raw = serde_json::to_string(&value).unwrap();
        deserialize_team_registration_result(&raw)
    }

    fn prepend_duplicate_field(
        value: serde_json::Value,
        field: &str,
        duplicate_value: &str,
    ) -> String {
        let raw = serde_json::to_string(&value).unwrap();
        assert!(raw.starts_with('{'));
        format!("{{\"{field}\":{duplicate_value},{}", &raw[1..])
    }

    fn assert_dto_rejects_unknown_and_type_mismatch<T>(
        valid: serde_json::Value,
        mismatched_field: &str,
        mismatched_value: serde_json::Value,
    ) where
        T: DeserializeOwned,
    {
        assert!(serde_json::from_value::<T>(valid.clone()).is_ok());

        let mut unknown = valid.clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unknown_field".to_string(), json!(true));
        assert!(serde_json::from_value::<T>(unknown).is_err());

        let mut mismatched = valid;
        mismatched
            .as_object_mut()
            .unwrap()
            .insert(mismatched_field.to_string(), mismatched_value);
        assert!(serde_json::from_value::<T>(mismatched).is_err());
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
        assert!(decode_email_value(with_token).is_err());
        let too_many = invited_login_json(51, None);
        assert!(decode_email_value(too_many).is_err());
    }

    #[test]
    fn email_login_deserialize_enforces_exact_variants_and_bounds() {
        assert!(serde_json::from_value::<EmailLoginOutcome>(authenticated_email_login_json())
            .is_ok());
        assert!(serde_json::from_value::<EmailLoginOutcome>(invited_login_json(50, None)).is_ok());
        assert!(serde_json::from_value::<EmailLoginOutcome>(invited_login_json(51, None)).is_err());

        let continuation_fields = [
            ("registration_continuation", json!("cross-variant")),
            ("continuation_expires_at", json!("2026-09-04T10:05:00Z")),
            ("invitations", json!([])),
            ("pending_invitation_count", json!(0)),
            ("selection_state", json!("unique")),
        ];
        for (field, field_value) in continuation_fields {
            let mut value = authenticated_email_login_json();
            value
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), field_value);
            assert!(
                serde_json::from_value::<EmailLoginOutcome>(value).is_err(),
                "authenticated outcome accepted cross-variant field {field}"
            );
        }

        let authenticated_fields = [
            ("access_token", json!("cross-variant")),
            ("access_expires_in_seconds", json!(900)),
            ("refresh_token", json!("cross-variant")),
            ("refresh_expires_at", json!("2026-10-04T10:00:00Z")),
            ("token_type", json!("Bearer")),
            ("is_new_user", json!(false)),
            ("registration_credit_granted", json!("0")),
            ("user", login_user_json()),
        ];
        for (field, field_value) in authenticated_fields {
            let mut value = invited_login_json(1, None);
            value
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), field_value);
            assert!(
                serde_json::from_value::<EmailLoginOutcome>(value).is_err(),
                "continuation outcome accepted cross-variant field {field}"
            );
        }

        for mut value in [authenticated_email_login_json(), invited_login_json(1, None)] {
            value
                .as_object_mut()
                .unwrap()
                .insert("unknown_field".to_string(), json!(true));
            assert!(serde_json::from_value::<EmailLoginOutcome>(value).is_err());
        }

        for (field, field_value) in [("unknown_field", json!(true)), ("id", json!(1))] {
            let mut value = authenticated_email_login_json();
            value["user"]
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), field_value);
            assert!(serde_json::from_value::<EmailLoginOutcome>(value).is_err());
        }
    }

    #[test]
    fn team_registration_deserialize_enforces_exact_session_variants() {
        assert!(serde_json::from_value::<TeamRegistrationResult>(first_registration_json()).is_ok());
        assert!(serde_json::from_value::<TeamRegistrationResult>(replay_registration_json()).is_ok());

        for flag in [true, false] {
            let mut value = first_registration_json();
            value
                .as_object_mut()
                .unwrap()
                .insert("session_login_required".to_string(), json!(flag));
            assert!(serde_json::from_value::<TeamRegistrationResult>(value).is_err());
        }

        let token_fields = [
            ("access_token", json!("cross-variant")),
            ("access_expires_in_seconds", json!(900)),
            ("refresh_token", json!("cross-variant")),
            ("refresh_expires_at", json!("2026-10-04T10:00:00Z")),
            ("token_type", json!("Bearer")),
        ];
        for (field, field_value) in token_fields {
            let mut value = replay_registration_json();
            value
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), field_value);
            assert!(
                serde_json::from_value::<TeamRegistrationResult>(value).is_err(),
                "registration replay accepted token field {field}"
            );
        }

        let mut false_replay = replay_registration_json();
        false_replay["session_login_required"] = json!(false);
        assert!(serde_json::from_value::<TeamRegistrationResult>(false_replay).is_err());

        for field in [
            "access_token",
            "access_expires_in_seconds",
            "refresh_token",
            "refresh_expires_at",
            "token_type",
        ] {
            let mut partial = first_registration_json();
            partial.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<TeamRegistrationResult>(partial).is_err(),
                "registration accepted partial token set missing {field}"
            );
        }

        for mut value in [first_registration_json(), replay_registration_json()] {
            value
                .as_object_mut()
                .unwrap()
                .insert("unknown_field".to_string(), json!(true));
            assert!(serde_json::from_value::<TeamRegistrationResult>(value).is_err());
        }

        for mut value in [first_registration_json(), replay_registration_json()] {
            value["user"]
                .as_object_mut()
                .unwrap()
                .insert("unknown_field".to_string(), json!(true));
            assert!(serde_json::from_value::<TeamRegistrationResult>(value).is_err());
        }

        for mut duplicate_choice in [first_registration_json(), replay_registration_json()] {
            let duplicate = duplicate_choice["group_choices"][0].clone();
            duplicate_choice["group_choices"]
                .as_array_mut()
                .unwrap()
                .push(duplicate);
            assert!(serde_json::from_value::<TeamRegistrationResult>(duplicate_choice).is_err());
        }
    }

    #[test]
    fn raw_decoders_reject_duplicate_security_fields() {
        let email_cases = [
            (
                "discriminator",
                prepend_duplicate_field(
                    authenticated_email_login_json(),
                    "outcome",
                    "\"authenticated\"",
                ),
            ),
            (
                "continuation",
                prepend_duplicate_field(
                    invited_login_json(1, None),
                    "registration_continuation",
                    "\"duplicate\"",
                ),
            ),
            (
                "email access token",
                prepend_duplicate_field(
                    authenticated_email_login_json(),
                    "access_token",
                    "\"duplicate\"",
                ),
            ),
        ];
        for (label, raw) in email_cases {
            assert!(
                deserialize_email_login_outcome(&raw).is_err(),
                "accepted duplicate email-login {label}"
            );
        }

        let registration_cases = [
            (
                "registration access token",
                prepend_duplicate_field(
                    first_registration_json(),
                    "access_token",
                    "\"duplicate\"",
                ),
            ),
            (
                "registration session flag",
                prepend_duplicate_field(
                    replay_registration_json(),
                    "session_login_required",
                    "true",
                ),
            ),
        ];
        for (label, raw) in registration_cases {
            assert!(
                deserialize_team_registration_result(&raw).is_err(),
                "accepted duplicate {label}"
            );
        }
    }

    #[test]
    fn authenticated_result_debug_redacts_session_tokens() {
        let outcome = decode_email_value(authenticated_email_login_json()).unwrap();
        let registration = decode_registration_value(first_registration_json()).unwrap();
        let outputs = [
            format!("{outcome:?}"),
            format!("{registration:?}"),
            format!("{:?}", registration.session),
        ];
        for output in outputs {
            assert!(!output.contains("access-sentinel"));
            assert!(!output.contains("refresh-sentinel"));
        }
    }

    #[test]
    fn team_items_rejects_non_item_fields() {
        let exact: TeamItems<UsageEventView> =
            serde_json::from_value(json!({"items": []})).unwrap();
        assert!(exact.items.is_empty());
        for field in ["pending_invitation_count", "next_cursor"] {
            let mut value = json!({"items": []});
            value
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), json!("forbidden"));
            assert!(
                serde_json::from_value::<TeamItems<UsageEventView>>(value).is_err(),
                "team page data accepted {field}"
            );
        }
    }

    #[test]
    fn canonical_team_dtos_reject_unknown_and_type_mismatched_fields() {
        assert_dto_rejects_unknown_and_type_mismatch::<QuotaSummary>(
            quota_json(),
            "monthly_limit",
            json!(500),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<AccountGroupChoice>(
            group_choice_json(vec!["bill"]),
            "group_version",
            json!(1),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<MemberView>(
            member_json(),
            "monthly_limit",
            json!(500),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<InvitationView>(
            invitation_summary_json(),
            "monthly_limit",
            json!(500),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<OwnerInvitationView>(
            owner_invitation_json(),
            "delivery_status",
            json!(1),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<UsageMemberView>(
            usage_member_json(),
            "user_id",
            json!(1),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<UsageEventView>(
            usage_event_json(),
            "credit_amount",
            json!(4),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<TeamRegistrationInvitationSummary>(
            invitation_summary_json(),
            "version",
            json!(1),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<AccountGroupList>(
            json!({
                "items": [group_choice_json(vec!["bill"])],
                "pending_invitation_count": 1
            }),
            "pending_invitation_count",
            json!("1"),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<ReauthenticationCodeResult>(
            json!({
                "email_masked": "m***@example.com",
                "expires_in_seconds": 300,
                "resend_after_seconds": 60
            }),
            "expires_in_seconds",
            json!("300"),
        );
        assert_dto_rejects_unknown_and_type_mismatch::<ReauthenticationResult>(
            reauthentication_result_json(),
            "user_id",
            json!(1),
        );
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
