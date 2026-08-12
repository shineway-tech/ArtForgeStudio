use super::{
    ApiClient, ApiError, ApiResponse, CreditPack, PaymentApi, SessionManager, SessionScope,
};
use reqwest::Method;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AccountUser {
    pub(crate) id: String,
    pub(crate) email_masked: String,
    pub(crate) nickname: Option<String>,
    pub(crate) status: String,
    pub(crate) registered_at: String,
    #[serde(default)]
    pub(crate) invitation_code_submitted: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MembershipPlanSummary {
    pub(crate) code: String,
    pub(crate) name: String,
    pub(crate) tier_rank: i32,
    pub(crate) recharge_discount_bps: i32,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MembershipPlan {
    pub(crate) code: String,
    pub(crate) version: u32,
    pub(crate) name: String,
    pub(crate) tier_rank: i32,
    pub(crate) price_cents: String,
    pub(crate) period_days: i32,
    pub(crate) grant_credits: String,
    pub(crate) recharge_discount_bps: i32,
    pub(crate) entitlements: Value,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AccountMembership {
    pub(crate) revision: String,
    pub(crate) period_id: Option<String>,
    pub(crate) starts_at: Option<String>,
    pub(crate) ends_at: Option<String>,
    pub(crate) plan: Option<MembershipPlanSummary>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreditAccount {
    pub(crate) available: String,
    pub(crate) reserved: String,
    pub(crate) lifetime_granted: String,
    pub(crate) lifetime_spent: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AccountSnapshot {
    pub(crate) user: AccountUser,
    #[serde(default)]
    pub(crate) auth_methods: AccountAuthMethods,
    pub(crate) membership: AccountMembership,
    pub(crate) credits: Option<CreditAccount>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct AccountAuthMethods {
    #[serde(default)]
    pub(crate) email: AccountAuthMethod,
    #[serde(default)]
    pub(crate) wechat: WechatAuthMethod,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct AccountAuthMethod {
    pub(crate) bound: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct WechatAuthMethod {
    pub(crate) bound: bool,
    pub(crate) can_unbind: bool,
    pub(crate) nickname: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WechatBindingStartResponse {
    pub(crate) login_id: String,
    pub(crate) authorization_url: String,
    #[serde(default)]
    pub(crate) qr_image_base64: String,
    pub(crate) expires_in_seconds: u64,
    pub(crate) poll_after_seconds: u64,
    #[serde(default)]
    pub(crate) poll_after_milliseconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WechatBindingStatusResponse {
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) qr_status: Option<String>,
    pub(crate) message: Option<String>,
    #[serde(default)]
    pub(crate) bound: bool,
    pub(crate) can_unbind: Option<bool>,
    pub(crate) nickname: Option<String>,
}

#[derive(Serialize)]
struct WechatBindingStatusRequest<'a> {
    login_id: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct EmailBindingCodeResponse {
    pub(crate) email_masked: String,
    pub(crate) expires_in_seconds: u64,
    pub(crate) resend_after_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct EmailBindingResponse {
    pub(crate) bound: bool,
    pub(crate) email_masked: String,
}

#[derive(Serialize)]
struct EmailBindingCodeRequest<'a> {
    email: &'a str,
}

#[derive(Serialize)]
struct EmailBindingRequest<'a> {
    email: &'a str,
    code: &'a str,
}

#[derive(Serialize)]
struct InvitationCodeRequest<'a> {
    code: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct InvitationOverview {
    pub(crate) enabled: bool,
    pub(crate) reward_type: String,
    pub(crate) reward_rate_bps: u32,
    pub(crate) reward_rate_percent: String,
    pub(crate) invitation_code: Option<String>,
    pub(crate) invitation_count: u64,
    pub(crate) total_reward_credits: String,
    pub(crate) pending_reward_credits: String,
    pub(crate) reversed_reward_credits: String,
    pub(crate) reversal_debt_credits: String,
    pub(crate) rule_description: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct InvitedUserDto {
    pub(crate) id: String,
    pub(crate) email_masked: String,
    pub(crate) nickname: String,
    pub(crate) reward_credits: String,
    pub(crate) registered_at: String,
}

#[derive(Clone, Debug, Deserialize)]
struct InvitationUserList {
    items: Vec<InvitedUserDto>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct InvitationUserPage {
    pub(crate) items: Vec<InvitedUserDto>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct InvitationDashboard {
    pub(crate) overview: InvitationOverview,
    pub(crate) users: Vec<InvitedUserDto>,
    pub(crate) users_next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ModelPrice {
    pub(crate) quality: String,
    pub(crate) max_long_edge: Option<u32>,
    pub(crate) credit_cost: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ModelCatalogItem {
    pub(crate) code: String,
    pub(crate) version: u32,
    pub(crate) purpose: String,
    pub(crate) name: String,
    pub(crate) capabilities: Value,
    pub(crate) prices: Vec<ModelPrice>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelCatalog {
    items: Vec<ModelCatalogItem>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreditLedgerItem {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) entry_type: String,
    pub(crate) available_delta: String,
    pub(crate) reserved_delta: String,
    pub(crate) available_after: String,
    pub(crate) reserved_after: String,
    pub(crate) business_type: String,
    pub(crate) description: String,
    pub(crate) created_at: String,
}

pub(crate) const CREDIT_LEDGER_PAGE_SIZE: usize = 8;

#[derive(Clone, Debug)]
pub(crate) struct CreditLedgerPage {
    pub(crate) items: Vec<CreditLedgerItem>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountScopeDisposition {
    Current,
    CapturedTerminal,
    Stale,
}

pub(crate) fn account_scope_disposition(
    current_owner_user_id: Option<&str>,
    session: &SessionManager,
    scope: &SessionScope,
) -> AccountScopeDisposition {
    classify_account_scope(
        current_owner_user_id == Some(scope.owner_user_id.as_str()),
        session.is_scope_current(scope),
        session.access().is_some(),
        session.auth_epoch(),
        scope.auth_epoch,
    )
}

fn classify_account_scope(
    owner_matches: bool,
    scope_is_current: bool,
    session_has_access: bool,
    current_auth_epoch: u64,
    captured_auth_epoch: u64,
) -> AccountScopeDisposition {
    if owner_matches && scope_is_current {
        AccountScopeDisposition::Current
    } else if owner_matches
        && !session_has_access
        && (current_auth_epoch == captured_auth_epoch
            || current_auth_epoch == captured_auth_epoch.wrapping_add(1))
    {
        AccountScopeDisposition::CapturedTerminal
    } else {
        AccountScopeDisposition::Stale
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BackendSnapshot {
    pub(crate) account: AccountSnapshot,
    pub(crate) plans: Vec<MembershipPlan>,
    pub(crate) packs: Vec<CreditPack>,
    pub(crate) models: Vec<ModelCatalogItem>,
    pub(crate) ledger: Vec<CreditLedgerItem>,
    pub(crate) ledger_next_cursor: Option<String>,
    pub(crate) sessions: Vec<AccountSessionDto>,
    pub(crate) invitation: Option<InvitationDashboard>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AccountSessionDto {
    pub(crate) id: String,
    pub(crate) device_name: String,
    pub(crate) platform: String,
    pub(crate) app_version: String,
    pub(crate) last_seen_at: String,
    pub(crate) is_current: bool,
}

#[derive(Deserialize)]
struct SessionList {
    items: Vec<AccountSessionDto>,
}

#[derive(Clone)]
pub(crate) struct AccountApi {
    client: ApiClient,
}

impl AccountApi {
    pub(crate) fn new(client: ApiClient) -> Self {
        Self { client }
    }

    pub(crate) fn snapshot(&self) -> Result<BackendSnapshot, ApiError> {
        let auth_epoch = self
            .client
            .session()
            .access()
            .ok_or(ApiError::AuthenticationRequired)?
            .auth_epoch;
        self.snapshot_epoch(auth_epoch)
    }

    pub(crate) fn snapshot_epoch(&self, auth_epoch: u64) -> Result<BackendSnapshot, ApiError> {
        std::thread::scope(|scope| {
            let account_client = self.client.clone();
            let account = scope.spawn(move || {
                account_client
                    .authenticated_json_epoch::<AccountSnapshot>(
                        Method::GET,
                        "/v1/account",
                        None,
                        None,
                        auth_epoch,
                    )
                    .map(|response| response.data)
            });
            let credit_client = self.client.clone();
            let credits = scope.spawn(move || {
                credit_client
                    .authenticated_json_epoch::<CreditAccount>(
                        Method::GET,
                        "/v1/credits/account",
                        None,
                        None,
                        auth_epoch,
                    )
                    .map(|response| response.data)
            });
            let plan_client = self.client.clone();
            let plans = scope.spawn(move || {
                plan_client
                    .authenticated_json_epoch::<Vec<MembershipPlan>>(
                        Method::GET,
                        "/v1/membership/plans",
                        None,
                        None,
                        auth_epoch,
                    )
                    .map(|response| response.data)
            });
            let membership_client = self.client.clone();
            let membership = scope.spawn(move || {
                membership_client
                    .authenticated_json_epoch::<Value>(
                        Method::GET,
                        "/v1/membership/current",
                        None,
                        None,
                        auth_epoch,
                    )
                    .map(|response| response.data)
            });
            let pack_client = self.client.clone();
            let packs =
                scope.spawn(move || PaymentApi::new(pack_client).packs_epoch(auth_epoch));
            let model_client = self.client.clone();
            let models = scope.spawn(move || {
                model_client
                    .authenticated_json_epoch::<ModelCatalog>(
                        Method::GET,
                        "/v1/models",
                        None,
                        None,
                        auth_epoch,
                    )
                    .map(|response| response.data.items)
            });
            let ledger_client = self.client.clone();
            let ledger = scope.spawn(move || {
                AccountApi::new(ledger_client).ledger_page_epoch(
                    None,
                    CREDIT_LEDGER_PAGE_SIZE,
                    auth_epoch,
                )
            });
            let session_client = self.client.clone();
            let sessions = scope.spawn(move || {
                session_client
                    .authenticated_json_epoch::<SessionList>(
                        Method::GET,
                        "/v1/account/sessions",
                        None,
                        None,
                        auth_epoch,
                    )
                    .map(|response| response.data.items)
            });
            let invitation_client = self.client.clone();
            let invitation = scope.spawn(move || {
                AccountApi::new(invitation_client).invitation_dashboard_epoch(auth_epoch)
            });

            // Join every sibling before choosing an error. A terminal response can clear the
            // captured lease while another sibling, which has not cloned its token yet, observes
            // AuthenticationRequired. Fixed join order must not downgrade that terminal outcome.
            let account = join_snapshot(account).and_then(|value| value);
            let credits = join_snapshot(credits).and_then(|value| value);
            let plans = join_snapshot(plans).and_then(|value| value);
            let membership = join_snapshot(membership).and_then(|value| value);
            let packs = join_snapshot(packs).and_then(|value| value);
            let models = join_snapshot(models).and_then(|value| value);
            let ledger_page = join_snapshot(ledger).and_then(|value| value);
            let sessions = join_snapshot(sessions).and_then(|value| value);
            let invitation = join_snapshot(invitation).and_then(|value| value);
            if let Some(error) = preferred_session_snapshot_error([
                account.as_ref().err(),
                credits.as_ref().err(),
                plans.as_ref().err(),
                membership.as_ref().err(),
                packs.as_ref().err(),
                models.as_ref().err(),
                ledger_page.as_ref().err(),
                sessions.as_ref().err(),
                invitation.as_ref().err(),
            ]) {
                return Err(error);
            }

            let mut account = account?;
            account.credits = Some(credits?);
            let plans = plans?;
            let _current_membership = membership?;
            let packs = packs?;
            let models = models?;
            let ledger_page = ledger_page?;
            let sessions = sessions?;
            let invitation = invitation.ok();
            Ok(BackendSnapshot {
                account,
                plans,
                packs,
                models,
                ledger: ledger_page.items,
                ledger_next_cursor: ledger_page.next_cursor,
                sessions,
                invitation,
            })
        })
    }

    pub(crate) fn ledger_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<CreditLedgerPage, ApiError> {
        let mut path = format!("/v1/credits/ledger?limit={limit}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(cursor);
        }
        let response = self.client.authenticated_json::<Vec<CreditLedgerItem>>(
            Method::GET,
            &path,
            None,
            None,
        )?;
        Ok(credit_ledger_page(response))
    }

    pub(crate) fn ledger_page_epoch(
        &self,
        cursor: Option<&str>,
        limit: usize,
        auth_epoch: u64,
    ) -> Result<CreditLedgerPage, ApiError> {
        let mut path = format!("/v1/credits/ledger?limit={limit}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(cursor);
        }
        let response = self.client.authenticated_json_epoch::<Vec<CreditLedgerItem>>(
            Method::GET,
            &path,
            None,
            None,
            auth_epoch,
        )?;
        Ok(credit_ledger_page(response))
    }

    pub(crate) fn ledger_page_scoped(
        &self,
        cursor: Option<&str>,
        limit: usize,
        scope: &SessionScope,
    ) -> Result<CreditLedgerPage, ApiError> {
        let mut path = format!("/v1/credits/ledger?limit={limit}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(cursor);
        }
        let response = self
            .client
            .authenticated_json_scoped::<Vec<CreditLedgerItem>>(
                Method::GET,
                &path,
                None,
                None,
                scope,
            )?;
        Ok(credit_ledger_page(response))
    }

    pub(crate) fn revoke_session(&self, session_id: &str) -> Result<(), ApiError> {
        self.client.authenticated_json::<serde_json::Value>(
            Method::DELETE,
            &format!("/v1/account/sessions/{session_id}"),
            None,
            None,
        )?;
        Ok(())
    }

    pub(crate) fn revoke_session_scoped(
        &self,
        session_id: &str,
        scope: &SessionScope,
    ) -> Result<(), ApiError> {
        self.client.authenticated_json_scoped::<serde_json::Value>(
            Method::DELETE,
            &format!("/v1/account/sessions/{session_id}"),
            None,
            None,
            scope,
        )?;
        Ok(())
    }

    pub(crate) fn start_wechat_binding(&self) -> Result<WechatBindingStartResponse, ApiError> {
        self.client
            .authenticated_json::<WechatBindingStartResponse>(
                Method::POST,
                "/v1/account/wechat/bind/session",
                None,
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn start_wechat_binding_scoped(
        &self,
        scope: &SessionScope,
    ) -> Result<WechatBindingStartResponse, ApiError> {
        self.client
            .authenticated_json_scoped::<WechatBindingStartResponse>(
                Method::POST,
                "/v1/account/wechat/bind/session",
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn wechat_binding_status(
        &self,
        login_id: &str,
    ) -> Result<WechatBindingStatusResponse, ApiError> {
        let body =
            serde_json::to_value(WechatBindingStatusRequest { login_id }).map_err(|error| {
                ApiError::Protocol {
                    message: error.to_string(),
                    request_id: None,
                }
            })?;
        self.client
            .authenticated_json::<WechatBindingStatusResponse>(
                Method::POST,
                "/v1/account/wechat/bind/session/status",
                Some(body),
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn wechat_binding_status_scoped(
        &self,
        login_id: &str,
        scope: &SessionScope,
    ) -> Result<WechatBindingStatusResponse, ApiError> {
        let body =
            serde_json::to_value(WechatBindingStatusRequest { login_id }).map_err(|error| {
                ApiError::Protocol {
                    message: error.to_string(),
                    request_id: None,
                }
            })?;
        self.client
            .authenticated_json_scoped::<WechatBindingStatusResponse>(
                Method::POST,
                "/v1/account/wechat/bind/session/status",
                Some(body),
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn unbind_wechat(&self) -> Result<WechatAuthMethod, ApiError> {
        self.client
            .authenticated_json::<WechatAuthMethod>(
                Method::DELETE,
                "/v1/account/wechat",
                None,
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn unbind_wechat_scoped(
        &self,
        scope: &SessionScope,
    ) -> Result<WechatAuthMethod, ApiError> {
        self.client
            .authenticated_json_scoped::<WechatAuthMethod>(
                Method::DELETE,
                "/v1/account/wechat",
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn request_email_binding_code(
        &self,
        email: &str,
    ) -> Result<EmailBindingCodeResponse, ApiError> {
        let body = serde_json::to_value(EmailBindingCodeRequest { email }).map_err(|error| {
            ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            }
        })?;
        self.client
            .authenticated_json::<EmailBindingCodeResponse>(
                Method::POST,
                "/v1/account/email/code",
                Some(body),
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn request_email_binding_code_scoped(
        &self,
        email: &str,
        scope: &SessionScope,
    ) -> Result<EmailBindingCodeResponse, ApiError> {
        let body = serde_json::to_value(EmailBindingCodeRequest { email }).map_err(|error| {
            ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            }
        })?;
        self.client
            .authenticated_json_scoped::<EmailBindingCodeResponse>(
                Method::POST,
                "/v1/account/email/code",
                Some(body),
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn bind_email(
        &self,
        email: &str,
        code: &str,
    ) -> Result<EmailBindingResponse, ApiError> {
        let body = serde_json::to_value(EmailBindingRequest { email, code }).map_err(|error| {
            ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            }
        })?;
        self.client
            .authenticated_json::<EmailBindingResponse>(
                Method::POST,
                "/v1/account/email/bind",
                Some(body),
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn bind_email_scoped(
        &self,
        email: &str,
        code: &str,
        scope: &SessionScope,
    ) -> Result<EmailBindingResponse, ApiError> {
        let body = serde_json::to_value(EmailBindingRequest { email, code }).map_err(|error| {
            ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            }
        })?;
        self.client
            .authenticated_json_scoped::<EmailBindingResponse>(
                Method::POST,
                "/v1/account/email/bind",
                Some(body),
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn submit_invitation_code(&self, code: &str) -> Result<Option<String>, ApiError> {
        let body = serde_json::to_value(InvitationCodeRequest { code }).map_err(|error| {
            ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            }
        })?;
        self.client
            .authenticated_json::<Value>(
                Method::POST,
                "/v1/account/invitation-code",
                Some(body),
                None,
            )
            .map(|response| {
                response
                    .data
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    pub(crate) fn submit_invitation_code_scoped(
        &self,
        code: &str,
        scope: &SessionScope,
    ) -> Result<Option<String>, ApiError> {
        let body = serde_json::to_value(InvitationCodeRequest { code }).map_err(|error| {
            ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            }
        })?;
        self.client
            .authenticated_json_scoped::<Value>(
                Method::POST,
                "/v1/account/invitation-code",
                Some(body),
                None,
                scope,
            )
            .map(|response| {
                response
                    .data
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    pub(crate) fn invitation_dashboard(&self) -> Result<InvitationDashboard, ApiError> {
        let overview = self
            .client
            .authenticated_json::<InvitationOverview>(
                Method::GET,
                "/v1/account/invitation",
                None,
                None,
            )?
            .data;
        let users = self
            .client
            .authenticated_json::<InvitationUserList>(
                Method::GET,
                "/v1/account/invitations?limit=50",
                None,
                None,
            )?
            .data;
        Ok(InvitationDashboard {
            overview,
            users: users.items,
            users_next_cursor: users.next_cursor,
        })
    }

    pub(crate) fn invitation_dashboard_epoch(
        &self,
        auth_epoch: u64,
    ) -> Result<InvitationDashboard, ApiError> {
        let overview = self
            .client
            .authenticated_json_epoch::<InvitationOverview>(
                Method::GET,
                "/v1/account/invitation",
                None,
                None,
                auth_epoch,
            )?
            .data;
        let users = self
            .client
            .authenticated_json_epoch::<InvitationUserList>(
                Method::GET,
                "/v1/account/invitations?limit=50",
                None,
                None,
                auth_epoch,
            )?
            .data;
        Ok(InvitationDashboard {
            overview,
            users: users.items,
            users_next_cursor: users.next_cursor,
        })
    }

    pub(crate) fn invitation_dashboard_scoped(
        &self,
        scope: &SessionScope,
    ) -> Result<InvitationDashboard, ApiError> {
        let overview = self
            .client
            .authenticated_json_scoped::<InvitationOverview>(
                Method::GET,
                "/v1/account/invitation",
                None,
                None,
                scope,
            )?
            .data;
        let users = self
            .client
            .authenticated_json_scoped::<InvitationUserList>(
                Method::GET,
                "/v1/account/invitations?limit=50",
                None,
                None,
                scope,
            )?
            .data;
        Ok(InvitationDashboard {
            overview,
            users: users.items,
            users_next_cursor: users.next_cursor,
        })
    }

    pub(crate) fn invitation_users_scoped(
        &self,
        cursor: &str,
        scope: &SessionScope,
    ) -> Result<InvitationUserPage, ApiError> {
        let path = format!("/v1/account/invitations?limit=50&cursor={cursor}");
        self.client
            .authenticated_json_scoped::<InvitationUserList>(
                Method::GET,
                &path,
                None,
                None,
                scope,
            )
            .map(|response| InvitationUserPage {
                items: response.data.items,
                next_cursor: response.data.next_cursor,
            })
    }
}

fn credit_ledger_page(response: ApiResponse<Vec<CreditLedgerItem>>) -> CreditLedgerPage {
    CreditLedgerPage {
        items: response.data,
        next_cursor: response.meta.and_then(|meta| meta.next_cursor),
    }
}

fn join_snapshot<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> Result<T, ApiError> {
    handle.join().map_err(|_| ApiError::LocalState {
        message: "账号数据同步线程异常退出".to_string(),
    })
}

fn preferred_session_snapshot_error<'a>(
    errors: impl IntoIterator<Item = Option<&'a ApiError>>,
) -> Option<ApiError> {
    let errors = errors.into_iter().flatten().collect::<Vec<_>>();
    errors
        .iter()
        .copied()
        .find(|error| error.is_terminal_session_error())
        .or_else(|| {
            errors
                .iter()
                .copied()
                .find(|error| matches!(error, ApiError::AuthenticationRequired))
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::api::{ApiMeta, ApiResponse};

    fn ledger_item(id: &str) -> CreditLedgerItem {
        CreditLedgerItem {
            id: id.to_string(),
            entry_type: "grant".to_string(),
            available_delta: "10".to_string(),
            reserved_delta: "0".to_string(),
            available_after: "10".to_string(),
            reserved_after: "0".to_string(),
            business_type: "registration".to_string(),
            description: "注册赠送".to_string(),
            created_at: "2026-07-15T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn ledger_page_preserves_next_cursor_from_response_meta() {
        let response = ApiResponse {
            request_id: "request-1".to_string(),
            data: vec![ledger_item("43")],
            meta: Some(ApiMeta {
                next_cursor: Some("42".to_string()),
            }),
        };

        let page = credit_ledger_page(response);

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "43");
        assert_eq!(page.next_cursor.as_deref(), Some("42"));
    }

    #[test]
    fn ledger_page_without_meta_has_no_next_cursor() {
        let response = ApiResponse {
            request_id: "request-2".to_string(),
            data: vec![ledger_item("1")],
            meta: None,
        };

        let page = credit_ledger_page(response);

        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn snapshot_prefers_a_terminal_sibling_over_earlier_authentication_required() {
        let authentication_required = ApiError::AuthenticationRequired;
        let terminal = ApiError::Http {
            status: 401,
            code: "refresh_token_reused".to_string(),
            message: "revoked".to_string(),
            request_id: None,
            details: None,
        };

        let selected = preferred_session_snapshot_error([
            Some(&authentication_required),
            Some(&terminal),
        ])
        .expect("terminal sibling must win");

        assert!(selected.is_terminal_session_error());
        assert_eq!(selected.code(), Some("refresh_token_reused"));
    }

    #[test]
    fn snapshot_propagates_optional_invitation_authentication_required() {
        let authentication_required = ApiError::AuthenticationRequired;

        let selected = preferred_session_snapshot_error([Some(&authentication_required)])
            .expect("captured session loss cannot be downgraded as optional");

        assert!(matches!(selected, ApiError::AuthenticationRequired));
    }

    #[test]
    fn account_scope_classifier_distinguishes_current_terminal_and_stale_leases() {
        assert_eq!(
            classify_account_scope(true, true, true, 7, 7),
            AccountScopeDisposition::Current
        );
        assert_eq!(
            classify_account_scope(true, false, false, 8, 7),
            AccountScopeDisposition::CapturedTerminal
        );
        assert_eq!(
            classify_account_scope(false, false, true, 9, 7),
            AccountScopeDisposition::Stale
        );
        assert_eq!(
            classify_account_scope(true, false, true, 9, 7),
            AccountScopeDisposition::Stale
        );
    }

    #[test]
    fn invitation_code_request_serializes_only_the_code() {
        let body = serde_json::to_value(InvitationCodeRequest {
            code: "ELUNVI-2026",
        })
        .expect("invitation-code request should serialize");

        assert_eq!(body, serde_json::json!({ "code": "ELUNVI-2026" }));
    }

    #[test]
    fn legacy_account_snapshot_defaults_invitation_code_to_unsubmitted() {
        let user: AccountUser = serde_json::from_value(serde_json::json!({
            "id": "user-1",
            "email_masked": "u***@example.com",
            "nickname": null,
            "status": "active",
            "registered_at": "2026-08-10T00:00:00Z"
        }))
        .expect("legacy account user should remain compatible");

        assert!(!user.invitation_code_submitted);
    }
}
