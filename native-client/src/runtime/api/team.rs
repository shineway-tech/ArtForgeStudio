//! Canonical wire DTOs for team account operations.

use super::{
    deserialize_secret_string, AccountMembership, AgreementAcceptance, ApiClient, ApiError,
    ApiResponse, CreditAccount, LoginResponse, LoginUser, SecretString, SessionScope, TokenSet,
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

const TEAM_PAGE_SIZE: &str = "50";

pub(crate) fn page_path(path: &str, cursor: Option<&str>) -> Result<String, ApiError> {
    let mut url = reqwest::Url::parse(&format!("http://desktop.invalid{path}")).map_err(
        |error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        },
    )?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("page_size", TEAM_PAGE_SIZE);
        if let Some(cursor) = cursor {
            query.append_pair("cursor", cursor);
        }
    }
    Ok(format!(
        "{}?{}",
        url.path(),
        url.query().unwrap_or_default()
    ))
}

pub(crate) fn uuid_path_segment(value: &str) -> Result<String, ApiError> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::Protocol {
            message: "团队路由标识必须是规范 UUID".to_string(),
            request_id: None,
        })
}

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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BillingSummary {
    pub(crate) group_id: String,
    pub(crate) group_version: String,
    pub(crate) credits: CreditAccount,
    pub(crate) membership: AccountMembership,
}

pub(crate) enum MemberPolicyAction<'a> {
    SetLimit { monthly_credit_limit: &'a str },
    Suspend,
    Resume,
}

fn policy_body(
    action: MemberPolicyAction<'_>,
    expected_version: &str,
) -> Result<serde_json::Value, ApiError> {
    let mut body = match action {
        MemberPolicyAction::SetLimit {
            monthly_credit_limit,
        } => json!({
            "action": "set_limit",
            "monthly_credit_limit": monthly_credit_limit
        }),
        MemberPolicyAction::Suspend => json!({"action": "suspend"}),
        MemberPolicyAction::Resume => json!({"action": "resume"}),
    };
    body["expected_version"] = serde_json::Value::String(expected_version.to_string());
    Ok(body)
}

fn team_page<T>(response: ApiResponse<TeamItems<T>>) -> Result<TeamPage<T>, ApiError> {
    let ApiResponse {
        request_id,
        data,
        meta,
    } = response;
    let meta = meta.ok_or_else(|| ApiError::Protocol {
        message: "团队分页响应缺少 meta.next_cursor".to_string(),
        request_id: Some(request_id.clone()),
    })?;
    if data.items.len() > 50 {
        return Err(ApiError::Protocol {
            message: "团队分页响应超过请求的 50 条上限".to_string(),
            request_id: Some(request_id),
        });
    }
    Ok(TeamPage {
        items: data.items,
        next_cursor: meta.next_cursor,
    })
}

#[derive(Clone)]
pub(crate) struct TeamApi {
    client: ApiClient,
}

impl TeamApi {
    pub(crate) fn new(client: ApiClient) -> Self {
        Self { client }
    }

    pub(crate) fn list_groups(
        &self,
        scope: &SessionScope,
    ) -> Result<AccountGroupList, ApiError> {
        self.client
            .identity_json_scoped::<AccountGroupList>(
                Method::GET,
                "/v1/account-groups",
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn list_pending_invitations(
        &self,
        cursor: Option<&str>,
        scope: &SessionScope,
    ) -> Result<TeamPage<InvitationView>, ApiError> {
        let path = page_path("/v1/account-group-invitations/pending", cursor)?;
        self.client
            .identity_json_scoped(Method::GET, &path, None, None, scope)
            .and_then(team_page)
    }

    pub(crate) fn accept_invitation(
        &self,
        invitation_id: &str,
        expected_version: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<MemberView, ApiError> {
        let invitation_id = uuid_path_segment(invitation_id)?;
        let path = format!(
            "/v1/account-group-invitations/{invitation_id}/accept"
        );
        self.client
            .identity_json_scoped::<MemberView>(
                Method::POST,
                &path,
                Some(json!({"expected_version": expected_version})),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn decline_invitation(
        &self,
        invitation_id: &str,
        expected_version: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<InvitationView, ApiError> {
        let invitation_id = uuid_path_segment(invitation_id)?;
        let path = format!(
            "/v1/account-group-invitations/{invitation_id}/decline"
        );
        self.client
            .identity_json_scoped::<InvitationView>(
                Method::POST,
                &path,
                Some(json!({"expected_version": expected_version})),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn rename_group(
        &self,
        group_id: &str,
        name: &str,
        expected_version: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<AccountGroupChoice, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let path = format!("/v1/account-groups/{group_id}");
        self.client
            .identity_json_scoped::<AccountGroupChoice>(
                Method::PATCH,
                &path,
                Some(json!({
                    "name": name,
                    "expected_version": expected_version
                })),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn list_members(
        &self,
        group_id: &str,
        cursor: Option<&str>,
        scope: &SessionScope,
    ) -> Result<TeamPage<MemberView>, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let path = page_path(
            &format!("/v1/account-groups/{group_id}/members"),
            cursor,
        )?;
        self.client
            .identity_json_scoped(Method::GET, &path, None, None, scope)
            .and_then(team_page)
    }

    pub(crate) fn create_invitation(
        &self,
        group_id: &str,
        email: &str,
        monthly_credit_limit: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<OwnerInvitationView, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let path = format!("/v1/account-groups/{group_id}/invitations");
        self.client
            .identity_json_scoped::<OwnerInvitationView>(
                Method::POST,
                &path,
                Some(json!({
                    "email": email,
                    "monthly_credit_limit": monthly_credit_limit
                })),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn list_invitations(
        &self,
        group_id: &str,
        cursor: Option<&str>,
        scope: &SessionScope,
    ) -> Result<TeamPage<OwnerInvitationView>, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let path = page_path(
            &format!("/v1/account-groups/{group_id}/invitations"),
            cursor,
        )?;
        self.client
            .identity_json_scoped(Method::GET, &path, None, None, scope)
            .and_then(team_page)
    }

    pub(crate) fn resend_invitation(
        &self,
        group_id: &str,
        invitation_id: &str,
        monthly_credit_limit: &str,
        expected_version: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<OwnerInvitationView, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let invitation_id = uuid_path_segment(invitation_id)?;
        let path = format!(
            "/v1/account-groups/{group_id}/invitations/{invitation_id}/resend"
        );
        self.client
            .identity_json_scoped::<OwnerInvitationView>(
                Method::POST,
                &path,
                Some(json!({
                    "monthly_credit_limit": monthly_credit_limit,
                    "expected_version": expected_version
                })),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn revoke_invitation(
        &self,
        group_id: &str,
        invitation_id: &str,
        expected_version: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<OwnerInvitationView, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let invitation_id = uuid_path_segment(invitation_id)?;
        let path = format!(
            "/v1/account-groups/{group_id}/invitations/{invitation_id}/revoke"
        );
        self.client
            .identity_json_scoped::<OwnerInvitationView>(
                Method::POST,
                &path,
                Some(json!({"expected_version": expected_version})),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn update_member(
        &self,
        group_id: &str,
        member_id: &str,
        action: MemberPolicyAction<'_>,
        expected_version: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<MemberView, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let member_id = uuid_path_segment(member_id)?;
        let path = format!(
            "/v1/account-groups/{group_id}/members/{member_id}"
        );
        self.client
            .identity_json_scoped::<MemberView>(
                Method::PATCH,
                &path,
                Some(policy_body(action, expected_version)?),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn remove_member(
        &self,
        group_id: &str,
        member_id: &str,
        expected_version: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<MemberView, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let member_id = uuid_path_segment(member_id)?;
        let path = format!(
            "/v1/account-groups/{group_id}/members/{member_id}/remove"
        );
        self.client
            .identity_json_scoped::<MemberView>(
                Method::POST,
                &path,
                Some(json!({"expected_version": expected_version})),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn leave_group(
        &self,
        group_id: &str,
        member_id: &str,
        expected_version: &str,
        idempotency_key: &str,
        scope: &SessionScope,
    ) -> Result<MemberView, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let member_id = uuid_path_segment(member_id)?;
        let path = format!("/v1/account-groups/{group_id}/leave");
        self.client
            .identity_json_scoped::<MemberView>(
                Method::POST,
                &path,
                Some(json!({
                    "member_id": member_id,
                    "expected_version": expected_version
                })),
                Some(idempotency_key),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn own_membership(
        &self,
        group_id: &str,
        scope: &SessionScope,
    ) -> Result<MemberView, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let path = format!("/v1/account-groups/{group_id}/membership");
        self.client
            .identity_json_scoped::<MemberView>(Method::GET, &path, None, None, scope)
            .map(|response| response.data)
    }

    pub(crate) fn billing_summary(
        &self,
        group_id: &str,
        scope: &SessionScope,
    ) -> Result<BillingSummary, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let path = format!("/v1/account-groups/{group_id}/billing-summary");
        self.client
            .identity_json_scoped::<BillingSummary>(Method::GET, &path, None, None, scope)
            .map(|response| response.data)
    }

    pub(crate) fn usage_page(
        &self,
        group_id: &str,
        cursor: Option<&str>,
        scope: &SessionScope,
    ) -> Result<TeamPage<UsageEventView>, ApiError> {
        let group_id = uuid_path_segment(group_id)?;
        let path = page_path(
            &format!("/v1/account-groups/{group_id}/usage"),
            cursor,
        )?;
        self.client
            .identity_json_scoped(Method::GET, &path, None, None, scope)
            .and_then(team_page)
    }

    pub(crate) fn request_reauthentication_code(
        &self,
        scope: &SessionScope,
    ) -> Result<ReauthenticationCodeResult, ApiError> {
        self.client
            .identity_json_scoped::<ReauthenticationCodeResult>(
                Method::POST,
                "/v1/account/reauth/code",
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn reauthenticate(
        &self,
        proof: ReauthenticationRequest<'_>,
        scope: &SessionScope,
    ) -> Result<ReauthenticationResult, ApiError> {
        let body = serde_json::to_value(proof).map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .identity_json_scoped::<ReauthenticationResult>(
                Method::POST,
                "/v1/account/reauth",
                Some(body),
                None,
                scope,
            )
            .map(|response| response.data)
    }
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
    use crate::runtime::api::session::test_support::MemoryRefreshTokenStore;
    use crate::runtime::api::{
        ApiClient, ApiClientConfig, ApiMeta, DeviceIdentity, SessionManager, SessionScope,
    };
    use reqwest::Url;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;
    use uuid::Uuid;

    const TEST_USER_ID: &str = "33333333-3333-4333-8333-333333333333";
    const TEST_GROUP_ID: &str = "22222222-2222-4222-8222-222222222222";
    const TEST_INVITATION_ID: &str = "11111111-1111-4111-8111-111111111111";
    const TEST_MEMBER_ID: &str = "44444444-4444-4444-8444-444444444444";

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        target: String,
        headers: HashMap<String, String>,
        body: Option<serde_json::Value>,
    }

    impl CapturedRequest {
        fn parse(raw: &[u8]) -> Self {
            let request = String::from_utf8_lossy(raw);
            let (head, body) = request.split_once("\r\n\r\n").unwrap_or((&request, ""));
            let mut lines = head.lines();
            let mut request_line = lines.next().unwrap_or_default().split_whitespace();
            let method = request_line.next().unwrap_or_default().to_string();
            let target = request_line.next().unwrap_or_default().to_string();
            let path = target.split('?').next().unwrap_or_default().to_string();
            let headers = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| {
                    (name.trim().to_ascii_lowercase(), value.trim().to_string())
                })
                .collect();
            let body = (!body.trim().is_empty())
                .then(|| serde_json::from_str(body).expect("request body is JSON"));
            Self {
                method,
                path,
                target,
                headers,
                body,
            }
        }

        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .get(&name.to_ascii_lowercase())
                .map(String::as_str)
        }
    }

    struct CapturedRequests {
        base_url: String,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        worker: Option<JoinHandle<()>>,
    }

    impl CapturedRequests {
        fn serve_json(count: usize, body: serde_json::Value) -> Self {
            Self::serve_sequence(vec![body; count])
        }

        fn serve_sequence(responses: Vec<serde_json::Value>) -> Self {
            Self::serve_raw_sequence(
                responses
                    .into_iter()
                    .map(|body| serde_json::to_string(&body).unwrap())
                    .collect(),
            )
        }

        fn serve_raw(body: String) -> Self {
            Self::serve_raw_sequence(vec![body])
        }

        fn serve_raw_sequence(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::with_capacity(responses.len())));
            let captured = requests.clone();
            let worker = thread::spawn(move || {
                for body in responses {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_request(&mut stream);
                    captured
                        .lock()
                        .unwrap()
                        .push(CapturedRequest::parse(&request));
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len(),
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                }
            });
            Self {
                base_url: format!("http://{address}/"),
                requests,
                worker: Some(worker),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn finish(mut self) -> Vec<CapturedRequest> {
            self.worker.take().unwrap().join().unwrap();
            Arc::try_unwrap(self.requests)
                .unwrap()
                .into_inner()
                .unwrap()
        }
    }

    fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let received = stream.read(&mut buffer).unwrap();
            if received == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..received]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        request
    }

    fn test_client(base_url: String) -> ApiClient {
        ApiClient::new(
            ApiClientConfig {
                base_url: Url::parse(&base_url).unwrap(),
                app_version: "1.2.3".to_string(),
                timeout: Duration::from_secs(1),
            },
            DeviceIdentity {
                id: Uuid::new_v4().to_string(),
                name: "team-api-test".to_string(),
                platform: "macos".to_string(),
            },
            Arc::new(SessionManager::new(Arc::new(
                MemoryRefreshTokenStore::default(),
            ))),
        )
        .unwrap()
    }

    fn authenticated_test_client(base_url: String) -> ApiClient {
        let client = test_client(base_url);
        client
            .session()
            .install_tokens_for_user(
                &TokenSet {
                    access_token: "access-token".to_string(),
                    access_expires_in_seconds: 900,
                    refresh_token: "refresh-token".to_string(),
                    refresh_expires_at: "2099-01-01T00:00:00Z".to_string(),
                    token_type: "X-Token".to_string(),
                },
                TEST_USER_ID,
            )
            .unwrap();
        client
    }

    fn test_session() -> SessionScope {
        SessionScope {
            owner_user_id: TEST_USER_ID.to_string(),
            auth_epoch: 1,
        }
    }

    fn api_envelope_with_data(data: serde_json::Value) -> serde_json::Value {
        json!({
            "request_id": "team-api-test",
            "data": data,
            "error": null
        })
    }

    fn raw_api_envelope(data: &str) -> String {
        format!(r#"{{"request_id":"team-api-test","data":{data},"error":null}}"#)
    }

    fn invitation_page_envelope(next_cursor: &str) -> serde_json::Value {
        json!({
            "request_id": "invitation-page-test",
            "data": { "items": [invitation_summary_json()] },
            "error": null,
            "meta": { "next_cursor": next_cursor }
        })
    }

    fn invitation_page_envelope_with_count(count: usize) -> serde_json::Value {
        json!({
            "request_id": "invitation-page-bound-test",
            "data": {
                "items": (0..count)
                    .map(|_| invitation_summary_json())
                    .collect::<Vec<_>>()
            },
            "error": null,
            "meta": { "next_cursor": null }
        })
    }

    fn page_envelope(items: Vec<serde_json::Value>) -> serde_json::Value {
        json!({
            "request_id": "team-page-test",
            "data": { "items": items },
            "error": null,
            "meta": { "next_cursor": null }
        })
    }

    fn assert_request(
        request: &CapturedRequest,
        method: &str,
        target: &str,
        idempotency_key: Option<&str>,
        body: Option<serde_json::Value>,
    ) {
        assert_eq!(request.method, method);
        assert_eq!(request.target, target);
        assert_eq!(request.header("idempotency-key"), idempotency_key);
        assert_eq!(request.body, body);
        assert_eq!(request.header("x-account-group-id"), None);
    }

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

    fn billing_summary_json() -> serde_json::Value {
        json!({
            "group_id": TEST_GROUP_ID,
            "group_version": "9",
            "credits": {
                "available": "500",
                "reserved": "12",
                "lifetime_granted": "900",
                "lifetime_spent": "400",
                "version": "8"
            },
            "membership": {
                "revision": "3",
                "period_id": "2026-09",
                "starts_at": "2026-09-01T00:00:00Z",
                "ends_at": "2026-10-01T00:00:00Z",
                "plan": {
                    "code": "pro",
                    "name": "Pro",
                    "tier_rank": 2,
                    "recharge_discount_bps": 9000,
                    "max_quality": "4K"
                }
            }
        })
    }

    fn invitation_mutation_envelopes() -> Vec<serde_json::Value> {
        vec![
            api_envelope_with_data(member_json()),
            api_envelope_with_data(invitation_summary_json()),
        ]
    }

    fn owner_invitation_envelopes() -> Vec<serde_json::Value> {
        vec![
            api_envelope_with_data(owner_invitation_json()),
            page_envelope(vec![owner_invitation_json()]),
            api_envelope_with_data(owner_invitation_json()),
            api_envelope_with_data(owner_invitation_json()),
        ]
    }

    fn member_route_envelopes() -> Vec<serde_json::Value> {
        vec![
            page_envelope(vec![member_json()]),
            api_envelope_with_data(member_json()),
            api_envelope_with_data(member_json()),
            api_envelope_with_data(member_json()),
            api_envelope_with_data(member_json()),
        ]
    }

    fn owner_read_envelopes() -> Vec<serde_json::Value> {
        vec![
            api_envelope_with_data(group_choice_json(vec!["manage_group"])),
            api_envelope_with_data(billing_summary_json()),
            page_envelope(vec![usage_event_json()]),
        ]
    }

    fn reauthentication_envelopes() -> Vec<serde_json::Value> {
        vec![
            api_envelope_with_data(json!({
                "email_masked": "m***@example.com",
                "expires_in_seconds": 300,
                "resend_after_seconds": 60
            })),
            api_envelope_with_data(reauthentication_result_json()),
        ]
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
    fn authenticated_email_outcome_flattens_existing_login_fields() {
        let outcome: EmailLoginOutcome = serde_json::from_value(json!({
            "outcome": "authenticated",
            "access_token": "access",
            "access_expires_in_seconds": 900,
            "refresh_token": "refresh",
            "refresh_expires_at": "2026-10-04T10:00:00Z",
            "token_type": "Bearer",
            "is_new_user": false,
            "registration_credit_granted": "0",
            "user": {
                "id": "11111111-1111-4111-8111-111111111111",
                "email_masked": "m***@example.com",
                "nickname": null,
                "status": "active"
            }
        }))
        .unwrap();

        assert!(matches!(
            outcome,
            EmailLoginOutcome::Authenticated { .. }
        ));
    }

    #[test]
    fn email_login_response_stream_rejects_duplicate_security_fields() {
        let raw_data = prepend_duplicate_field(
            authenticated_email_login_json(),
            "outcome",
            "\"authenticated\"",
        );
        let capture = CapturedRequests::serve_raw(raw_api_envelope(&raw_data));
        let client = test_client(capture.base_url());

        let result = crate::runtime::api::AuthApi::new(client).login_response(
            "member@example.com",
            "123456",
            &[],
        );

        assert!(result.is_err());
        assert_eq!(capture.finish().len(), 1);
    }

    #[test]
    fn team_registration_uses_continuation_and_idempotency_without_auth() {
        let capture = CapturedRequests::serve_json(
            1,
            api_envelope_with_data(first_registration_json()),
        );
        let client = test_client(capture.base_url());
        let session_epoch = client.session().auth_epoch();
        let api = crate::runtime::api::AuthApi::new(client.clone());

        api.complete_team_registration(
            &SecretString::new("opaque-continuation".to_string()),
            "new-password",
            &[AgreementAcceptance {
                agreement_type: "user_terms".to_string(),
                version: "2026-09-01".to_string(),
            }],
            "registration-key",
        )
        .unwrap();

        let request = capture.finish().remove(0);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/auth/team-registration");
        assert_eq!(request.header("idempotency-key"), Some("registration-key"));
        let body = request.body.as_ref().expect("registration JSON body");
        assert_eq!(
            body["registration_continuation"],
            "opaque-continuation"
        );
        assert_eq!(
            body["agreement_acceptances"],
            json!([{
                "type": "user_terms",
                "version": "2026-09-01"
            }])
        );
        assert!(request.header("authorization").is_none());
        assert!(request.header("x-token").is_none());
        assert!(request.header("x-account-group-id").is_none());
        assert_eq!(client.session().auth_epoch(), session_epoch);
        assert!(client.session().access().is_none());
    }

    #[test]
    fn continuation_outcome_does_not_change_the_session() {
        let capture = CapturedRequests::serve_json(
            1,
            api_envelope_with_data(invited_login_json(1, None)),
        );
        let client = test_client(capture.base_url());
        let epoch = client.session().auth_epoch();

        let outcome: EmailLoginOutcome =
            crate::runtime::api::AuthApi::new(client.clone())
                .login_response("member@example.com", "123456", &[])
                .unwrap();

        assert!(matches!(
            outcome,
            EmailLoginOutcome::TeamRegistrationRequired { .. }
        ));
        assert_eq!(client.session().auth_epoch(), epoch);
        assert!(client.session().access().is_none());
        assert_eq!(capture.finish().len(), 1);
    }

    #[test]
    fn registration_replay_never_reuses_session_secrets() {
        let first_value = first_registration_json();
        assert!(first_value.get("user").is_some());
        assert!(first_value.get("group_choices").is_some());
        assert_eq!(first_value["selection_state"], "unique");
        for obsolete in ["user_id", "owned_account_group_id", "account_groups"] {
            assert!(first_value.get(obsolete).is_none());
            let mut drifted = first_registration_json();
            drifted.as_object_mut().unwrap().insert(
                obsolete.to_string(),
                serde_json::Value::String("forbidden".to_string()),
            );
            assert!(serde_json::from_value::<TeamRegistrationResult>(drifted).is_err());
        }
        let first: TeamRegistrationResult = serde_json::from_value(first_value).unwrap();
        assert!(matches!(
            first.session,
            TeamRegistrationSessionResult::Authenticated { .. }
        ));

        let replay_value = replay_registration_json();
        assert_eq!(replay_value["session_login_required"], true);
        for secret in ["access_token", "refresh_token", "token_type"] {
            assert!(replay_value.get(secret).is_none());
        }
        let replay: TeamRegistrationResult = serde_json::from_value(replay_value).unwrap();
        assert!(matches!(
            replay.session,
            TeamRegistrationSessionResult::LoginRequired {
                session_login_required: true
            }
        ));
    }

    #[test]
    fn registration_response_stream_rejects_duplicate_session_fields() {
        let raw_data = prepend_duplicate_field(
            replay_registration_json(),
            "session_login_required",
            "true",
        );
        let capture = CapturedRequests::serve_raw(raw_api_envelope(&raw_data));
        let client = test_client(capture.base_url());
        let result = crate::runtime::api::AuthApi::new(client).complete_team_registration(
            &SecretString::new("opaque-continuation".to_string()),
            "new-password",
            &[],
            "registration-key",
        );

        assert!(result.is_err());
        assert_eq!(capture.finish().len(), 1);
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

    #[test]
    fn pending_invitation_cursor_is_encoded_and_page_size_is_fifty() {
        let capture = CapturedRequests::serve_json(
            1,
            invitation_page_envelope("next/cursor+2"),
        );
        let api = TeamApi::new(authenticated_test_client(capture.base_url()));
        let page = api
            .list_pending_invitations(Some("opaque/+ cursor"), &test_session())
            .unwrap();
        let request = capture.finish().remove(0);
        assert_eq!(
            request.target,
            "/v1/account-group-invitations/pending?page_size=50&cursor=opaque%2F%2B+cursor"
        );
        assert_eq!(page.next_cursor.as_deref(), Some("next/cursor+2"));
    }

    #[test]
    fn team_page_requires_meta_and_honors_the_requested_bound() {
        let missing_meta = CapturedRequests::serve_json(
            1,
            api_envelope_with_data(json!({"items": []})),
        );
        let api = TeamApi::new(authenticated_test_client(missing_meta.base_url()));
        assert!(api
            .list_pending_invitations(None, &test_session())
            .is_err());
        assert_eq!(missing_meta.finish().len(), 1);

        let missing_cursor = CapturedRequests::serve_json(
            1,
            json!({
                "request_id": "missing-cursor-test",
                "data": { "items": [] },
                "error": null,
                "meta": {}
            }),
        );
        let api = TeamApi::new(authenticated_test_client(missing_cursor.base_url()));
        assert!(
            api.list_pending_invitations(None, &test_session())
                .is_err(),
            "team page accepted meta without required next_cursor"
        );
        assert_eq!(missing_cursor.finish().len(), 1);

        let unknown_meta = CapturedRequests::serve_json(
            1,
            json!({
                "request_id": "unknown-meta-test",
                "data": { "items": [] },
                "error": null,
                "meta": { "next_cursor": null, "unexpected": true }
            }),
        );
        let api = TeamApi::new(authenticated_test_client(unknown_meta.base_url()));
        assert!(
            api.list_pending_invitations(None, &test_session())
                .is_err(),
            "team page accepted unknown metadata field"
        );
        assert_eq!(unknown_meta.finish().len(), 1);

        let wrong_cursor = CapturedRequests::serve_json(
            1,
            json!({
                "request_id": "wrong-cursor-test",
                "data": { "items": [] },
                "error": null,
                "meta": { "next_cursor": 7 }
            }),
        );
        let api = TeamApi::new(authenticated_test_client(wrong_cursor.base_url()));
        assert!(
            api.list_pending_invitations(None, &test_session())
                .is_err(),
            "team page accepted a non-string, non-null cursor"
        );
        assert_eq!(wrong_cursor.finish().len(), 1);

        assert!(serde_json::from_str::<ApiMeta>(
            r#"{"next_cursor":null,"next_cursor":"duplicate"}"#
        )
        .is_err());

        let oversized = CapturedRequests::serve_json(
            1,
            invitation_page_envelope_with_count(51),
        );
        let api = TeamApi::new(authenticated_test_client(oversized.base_url()));
        assert!(api
            .list_pending_invitations(None, &test_session())
            .is_err());
        assert_eq!(oversized.finish().len(), 1);
    }

    #[test]
    fn account_groups_decode_items_and_pending_count_without_cursor_meta() {
        let capture = CapturedRequests::serve_json(
            1,
            json!({
                "request_id": "account-group-list-test",
                "data": {
                    "items": [group_choice_json(vec!["bill"])],
                    "pending_invitation_count": 2
                }
            }),
        );
        let api = TeamApi::new(authenticated_test_client(capture.base_url()));
        let result = api.list_groups(&test_session()).unwrap();
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.pending_invitation_count, 2);
        assert_eq!(capture.finish()[0].target, "/v1/account-groups");
    }

    #[test]
    fn recipient_mutations_use_invitation_id_version_and_idempotency() {
        let capture = CapturedRequests::serve_sequence(invitation_mutation_envelopes());
        let api = TeamApi::new(authenticated_test_client(capture.base_url()));
        api.accept_invitation(
            TEST_INVITATION_ID,
            "4",
            "accept-key",
            &test_session(),
        )
        .unwrap();
        api.decline_invitation(
            TEST_INVITATION_ID,
            "5",
            "decline-key",
            &test_session(),
        )
        .unwrap();
        let requests = capture.finish();
        assert_request(
            &requests[0],
            "POST",
            "/v1/account-group-invitations/11111111-1111-4111-8111-111111111111/accept",
            Some("accept-key"),
            Some(json!({"expected_version": "4"})),
        );
        assert_request(
            &requests[1],
            "POST",
            "/v1/account-group-invitations/11111111-1111-4111-8111-111111111111/decline",
            Some("decline-key"),
            Some(json!({"expected_version": "5"})),
        );
    }

    #[test]
    fn owner_invitation_routes_keep_the_pending_invitation_id() {
        let capture = CapturedRequests::serve_sequence(owner_invitation_envelopes());
        let api = TeamApi::new(authenticated_test_client(capture.base_url()));
        api.create_invitation(
            TEST_GROUP_ID,
            "member@example.com",
            "500",
            "create-key",
            &test_session(),
        )
        .unwrap();
        api.list_invitations(TEST_GROUP_ID, None, &test_session())
            .unwrap();
        let resent = api
            .resend_invitation(
                TEST_GROUP_ID,
                TEST_INVITATION_ID,
                "750",
                "3",
                "resend-key",
                &test_session(),
            )
            .unwrap();
        api.revoke_invitation(
            TEST_GROUP_ID,
            TEST_INVITATION_ID,
            "4",
            "revoke-key",
            &test_session(),
        )
        .unwrap();
        assert_eq!(resent.invitation_id, TEST_INVITATION_ID);
        let requests = capture.finish();
        assert_request(
            &requests[0],
            "POST",
            &format!("/v1/account-groups/{TEST_GROUP_ID}/invitations"),
            Some("create-key"),
            Some(json!({
                "email": "member@example.com",
                "monthly_credit_limit": "500"
            })),
        );
        assert_request(
            &requests[1],
            "GET",
            &format!(
                "/v1/account-groups/{TEST_GROUP_ID}/invitations?page_size=50"
            ),
            None,
            None,
        );
        assert_request(
            &requests[2],
            "POST",
            &format!(
                "/v1/account-groups/{TEST_GROUP_ID}/invitations/{TEST_INVITATION_ID}/resend"
            ),
            Some("resend-key"),
            Some(json!({
                "monthly_credit_limit": "750",
                "expected_version": "3"
            })),
        );
        assert_request(
            &requests[3],
            "POST",
            &format!(
                "/v1/account-groups/{TEST_GROUP_ID}/invitations/{TEST_INVITATION_ID}/revoke"
            ),
            Some("revoke-key"),
            Some(json!({"expected_version": "4"})),
        );
    }

    #[test]
    fn member_policy_serializes_one_action_and_expected_version() {
        assert_eq!(
            policy_body(
                MemberPolicyAction::SetLimit {
                    monthly_credit_limit: "900"
                },
                "7"
            )
            .unwrap(),
            json!({
                "action": "set_limit",
                "monthly_credit_limit": "900",
                "expected_version": "7"
            })
        );
        assert_eq!(
            policy_body(MemberPolicyAction::Suspend, "8").unwrap(),
            json!({"action": "suspend", "expected_version": "8"})
        );
        assert_eq!(
            policy_body(MemberPolicyAction::Resume, "9").unwrap(),
            json!({"action": "resume", "expected_version": "9"})
        );
    }

    #[test]
    fn member_routes_are_path_authorized_without_group_header() {
        let capture = CapturedRequests::serve_sequence(member_route_envelopes());
        let api = TeamApi::new(authenticated_test_client(capture.base_url()));
        api.list_members(TEST_GROUP_ID, None, &test_session())
            .unwrap();
        api.update_member(
            TEST_GROUP_ID,
            TEST_MEMBER_ID,
            MemberPolicyAction::Suspend,
            "3",
            "suspend-key",
            &test_session(),
        )
        .unwrap();
        api.remove_member(
            TEST_GROUP_ID,
            TEST_MEMBER_ID,
            "4",
            "remove-key",
            &test_session(),
        )
        .unwrap();
        api.leave_group(
            TEST_GROUP_ID,
            TEST_MEMBER_ID,
            "5",
            "leave-key",
            &test_session(),
        )
        .unwrap();
        api.own_membership(TEST_GROUP_ID, &test_session()).unwrap();
        let requests = capture.finish();
        assert_request(
            &requests[0],
            "GET",
            &format!("/v1/account-groups/{TEST_GROUP_ID}/members?page_size=50"),
            None,
            None,
        );
        assert_request(
            &requests[1],
            "PATCH",
            &format!(
                "/v1/account-groups/{TEST_GROUP_ID}/members/{TEST_MEMBER_ID}"
            ),
            Some("suspend-key"),
            Some(json!({"action": "suspend", "expected_version": "3"})),
        );
        assert_request(
            &requests[2],
            "POST",
            &format!(
                "/v1/account-groups/{TEST_GROUP_ID}/members/{TEST_MEMBER_ID}/remove"
            ),
            Some("remove-key"),
            Some(json!({"expected_version": "4"})),
        );
        assert_request(
            &requests[3],
            "POST",
            &format!("/v1/account-groups/{TEST_GROUP_ID}/leave"),
            Some("leave-key"),
            Some(json!({
                "member_id": TEST_MEMBER_ID,
                "expected_version": "5"
            })),
        );
        assert_request(
            &requests[4],
            "GET",
            &format!("/v1/account-groups/{TEST_GROUP_ID}/membership"),
            None,
            None,
        );
    }

    #[test]
    fn owner_reads_and_rename_use_path_group_authority() {
        let capture = CapturedRequests::serve_sequence(owner_read_envelopes());
        let api = TeamApi::new(authenticated_test_client(capture.base_url()));
        api.rename_group(
            TEST_GROUP_ID,
            "New Team",
            "8",
            "rename-key",
            &test_session(),
        )
        .unwrap();
        let summary = api.billing_summary(TEST_GROUP_ID, &test_session()).unwrap();
        assert_eq!(summary.group_id, TEST_GROUP_ID);
        assert_eq!(summary.group_version, "9");
        assert_eq!(summary.credits.available, "500");
        assert_eq!(summary.membership.revision, "3");
        api.usage_page(TEST_GROUP_ID, None, &test_session())
            .unwrap();
        let requests = capture.finish();
        assert_request(
            &requests[0],
            "PATCH",
            &format!("/v1/account-groups/{TEST_GROUP_ID}"),
            Some("rename-key"),
            Some(json!({"name": "New Team", "expected_version": "8"})),
        );
        assert_request(
            &requests[1],
            "GET",
            &format!("/v1/account-groups/{TEST_GROUP_ID}/billing-summary"),
            None,
            None,
        );
        assert_request(
            &requests[2],
            "GET",
            &format!("/v1/account-groups/{TEST_GROUP_ID}/usage?page_size=50"),
            None,
            None,
        );
    }

    #[test]
    fn billing_summary_rejects_invented_flattened_finance_keys() {
        let mut value = billing_summary_json();
        value
            .as_object_mut()
            .unwrap()
            .insert("available_credits".to_string(), json!("500"));
        assert!(serde_json::from_value::<BillingSummary>(value).is_err());

        let mut missing_credit_version = billing_summary_json();
        missing_credit_version["credits"]
            .as_object_mut()
            .unwrap()
            .remove("version");
        assert!(serde_json::from_value::<BillingSummary>(missing_credit_version).is_err());

        let mut missing_max_quality = billing_summary_json();
        missing_max_quality["membership"]["plan"]
            .as_object_mut()
            .unwrap()
            .remove("max_quality");
        assert!(serde_json::from_value::<BillingSummary>(missing_max_quality).is_err());
    }

    #[test]
    fn reauthentication_uses_current_session_without_billing_or_token_install() {
        let capture = CapturedRequests::serve_sequence(reauthentication_envelopes());
        let client = authenticated_test_client(capture.base_url());
        let auth_epoch = client.session().auth_epoch();
        let api = TeamApi::new(client.clone());
        let code = api
            .request_reauthentication_code(&test_session())
            .unwrap();
        assert_eq!(code.email_masked, "m***@example.com");
        assert_eq!(code.expires_in_seconds, 300);
        assert_eq!(code.resend_after_seconds, 60);
        let result = api
            .reauthenticate(
                ReauthenticationRequest::email_code("123456"),
                &test_session(),
            )
            .unwrap();
        assert_eq!(result.user_id, TEST_USER_ID);
        assert_eq!(client.session().auth_epoch(), auth_epoch);
        let requests = capture.finish();
        assert_request(
            &requests[0],
            "POST",
            "/v1/account/reauth/code",
            None,
            None,
        );
        assert_request(
            &requests[1],
            "POST",
            "/v1/account/reauth",
            None,
            Some(json!({"email_code": "123456"})),
        );
    }

    #[test]
    fn hostile_team_path_identifier_is_rejected_before_any_request() {
        let capture = CapturedRequests::serve_sequence(Vec::new());
        let api = TeamApi::new(authenticated_test_client(capture.base_url()));
        let invitation_error = api
            .accept_invitation(
                "../other-invitation",
                "1",
                "hostile-invitation",
                &test_session(),
            )
            .unwrap_err();
        let group_error = api
            .list_members("../other-group", None, &test_session())
            .unwrap_err();
        let member_error = api
            .remove_member(
                TEST_GROUP_ID,
                "../other-member",
                "1",
                "hostile-member",
                &test_session(),
            )
            .unwrap_err();
        for error in [invitation_error, group_error, member_error] {
            assert!(matches!(
                error,
                ApiError::Protocol {
                    request_id: None,
                    ..
                }
            ));
        }
        assert!(capture.finish().is_empty());
    }

}
