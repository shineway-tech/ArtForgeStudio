use super::{
    AccountGroupChoice, ApiClient, ApiError, ApiResponse, BillingScope,
    BillingSummary, CreditPack, KnownCapability, OrderDetail, PaymentApi, QuotaSummary,
    SessionManager, SessionScope, TeamApi, TeamPage,
};
use reqwest::{Method, Url};
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
#[serde(deny_unknown_fields)]
pub(crate) struct MembershipPlanSummary {
    pub(crate) code: String,
    pub(crate) name: String,
    pub(crate) tier_rank: i32,
    pub(crate) recharge_discount_bps: i32,
    pub(crate) max_quality: String,
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
#[serde(deny_unknown_fields)]
pub(crate) struct AccountMembership {
    pub(crate) revision: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub(crate) period_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub(crate) starts_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub(crate) ends_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub(crate) plan: Option<MembershipPlanSummary>,
}

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreditAccount {
    pub(crate) available: String,
    pub(crate) reserved: String,
    pub(crate) lifetime_granted: String,
    pub(crate) lifetime_spent: String,
    pub(crate) version: String,
}

#[derive(Serialize)]
struct CreditRedemptionRequest<'a> {
    code: &'a str,
    client_request_id: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreditRedemptionAccount {
    pub(crate) available: String,
    pub(crate) reserved: String,
    pub(crate) lifetime_granted: String,
    pub(crate) lifetime_spent: String,
    pub(crate) version: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreditRedemptionResult {
    pub(crate) redemption_id: String,
    pub(crate) credits_granted: String,
    pub(crate) redeemed_at: String,
    pub(crate) credit_expires_at: Option<String>,
    pub(crate) account: CreditRedemptionAccount,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AccountSnapshot {
    pub(crate) user: AccountUser,
    pub(crate) read_only: bool,
    pub(crate) capabilities: Vec<String>,
    #[serde(default)]
    pub(crate) auth_methods: AccountAuthMethods,
    pub(crate) membership: Option<AccountMembership>,
    pub(crate) billing_group: AccountGroupChoice,
    pub(crate) entitlement: Value,
    pub(crate) credits: Option<CreditAccount>,
    pub(crate) quota: Option<QuotaSummary>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct AccountAuthMethods {
    #[serde(default)]
    pub(crate) email: AccountAuthMethod,
    #[serde(default)]
    pub(crate) wechat: WechatAuthMethod,
    #[serde(default)]
    pub(crate) password: PasswordAuthMethod,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct AccountAuthMethod {
    #[serde(default)]
    pub(crate) bound: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct WechatAuthMethod {
    #[serde(default)]
    pub(crate) bound: bool,
    #[serde(default)]
    pub(crate) can_unbind: bool,
    pub(crate) nickname: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PasswordAuthMethod {
    pub(crate) set: bool,
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

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct PasswordCodeResponse {
    pub(crate) email_masked: String,
    pub(crate) expires_in_seconds: u64,
    pub(crate) resend_after_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct PasswordMutationResponse {
    pub(crate) set: bool,
    pub(crate) changed_at: String,
    pub(crate) other_sessions_revoked: bool,
}

#[derive(Serialize)]
struct PasswordMutationRequest<'a> {
    new_password: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_password: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email_code: Option<&'a str>,
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
    pub(crate) models: Option<Vec<ModelCatalogItem>>,
    pub(crate) plans: Option<Vec<MembershipPlan>>,
    pub(crate) packs: Option<Vec<CreditPack>>,
    pub(crate) ledger: Option<Vec<CreditLedgerItem>>,
    pub(crate) ledger_next_cursor: Option<String>,
    pub(crate) orders: Option<TeamPage<OrderDetail>>,
    pub(crate) owner_billing: Option<BillingSummary>,
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
    pub(crate) fn video_model_catalog(&self, scope: &BillingScope) -> Result<Vec<ModelCatalogItem>, ApiError> {
        self.client.billing_json_scoped::<ModelCatalog>(Method::GET, "/v1/models", None, None, scope).map(|response| response.data.items)
    }

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
            let packs = scope.spawn(move || PaymentApi::new(pack_client).packs_epoch(auth_epoch));
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
                plans: Some(plans),
                packs: Some(packs),
                models: Some(models),
                ledger: Some(ledger_page.items),
                ledger_next_cursor: ledger_page.next_cursor,
                orders: None,
                owner_billing: None,
                sessions,
                invitation,
            })
        })
    }

    pub(crate) fn snapshot_billing(
        &self,
        scope: &BillingScope,
    ) -> Result<BackendSnapshot, ApiError> {
        let mut account = self
            .client
            .billing_json_scoped::<AccountSnapshot>(Method::GET, "/v1/account", None, None, scope)?
            .data;
        validate_snapshot_context(&account, scope)?;

        let active = account.billing_group.selectable;
        let can_bill = active && account.billing_group.has_capability(KnownCapability::Bill);
        let can_purchase =
            active && account.billing_group.has_capability(KnownCapability::Purchase);
        let can_read_finance =
            account.billing_group.has_capability(KnownCapability::ReadGroupFinance);

        std::thread::scope(|thread_scope| {
            let session_client = self.client.clone();
            let session_scope = scope.request.session.clone();
            let sessions = thread_scope.spawn(move || {
                session_client
                    .identity_json_scoped::<SessionList>(
                        Method::GET,
                        "/v1/account/sessions",
                        None,
                        None,
                        &session_scope,
                    )
                    .map(|response| response.data.items)
            });

            let invitation_client = self.client.clone();
            let invitation_scope = scope.request.session.clone();
            let invitation = thread_scope.spawn(move || {
                AccountApi::new(invitation_client).invitation_dashboard_scoped(&invitation_scope)
            });

            let models = can_bill.then(|| {
                let client = self.client.clone();
                let request_scope = scope.clone();
                thread_scope.spawn(move || {
                    client
                        .billing_json_scoped::<ModelCatalog>(
                            Method::GET,
                            "/v1/models",
                            None,
                            None,
                            &request_scope,
                        )
                        .map(|response| response.data.items)
                })
            });

            let plans = can_purchase.then(|| {
                let client = self.client.clone();
                let request_scope = scope.clone();
                thread_scope.spawn(move || {
                    client
                        .billing_json_scoped::<Vec<MembershipPlan>>(
                            Method::GET,
                            "/v1/membership/plans",
                            None,
                            None,
                            &request_scope,
                        )
                        .map(|response| response.data)
                })
            });
            let packs = can_purchase.then(|| {
                let client = self.client.clone();
                let request_scope = scope.clone();
                thread_scope.spawn(move || PaymentApi::new(client).packs_billing(&request_scope))
            });

            let credits = (active && can_read_finance).then(|| {
                let client = self.client.clone();
                let request_scope = scope.clone();
                thread_scope
                    .spawn(move || AccountApi::new(client).credit_account_billing(&request_scope))
            });
            let ledger = (active && can_read_finance).then(|| {
                let client = self.client.clone();
                let request_scope = scope.clone();
                thread_scope.spawn(move || {
                    AccountApi::new(client).ledger_page_billing(
                        None,
                        CREDIT_LEDGER_PAGE_SIZE,
                        &request_scope,
                    )
                })
            });
            let orders = (active && can_read_finance).then(|| {
                let client = self.client.clone();
                let request_scope = scope.clone();
                thread_scope
                    .spawn(move || PaymentApi::new(client).orders_billing(None, &request_scope))
            });
            let owner_billing = can_read_finance.then(|| {
                let client = self.client.clone();
                let session_scope = scope.request.session.clone();
                let group_id = scope.request.account_group_id.clone();
                thread_scope
                    .spawn(move || TeamApi::new(client).billing_summary(&group_id, &session_scope))
            });

            let sessions = join_snapshot(sessions).and_then(|value| value);
            let invitation = join_snapshot(invitation).and_then(|value| value);
            let models = join_optional_snapshot(models);
            let plans = join_optional_snapshot(plans);
            let packs = join_optional_snapshot(packs);
            let credits = join_optional_snapshot(credits);
            let ledger = join_optional_snapshot(ledger);
            let orders = join_optional_snapshot(orders);
            let owner_billing = join_optional_snapshot(owner_billing);

            if let Some(error) = preferred_session_snapshot_error([
                sessions.as_ref().err(),
                invitation.as_ref().err(),
                models.as_ref().err(),
                plans.as_ref().err(),
                packs.as_ref().err(),
                credits.as_ref().err(),
                ledger.as_ref().err(),
                orders.as_ref().err(),
                owner_billing.as_ref().err(),
            ]) {
                return Err(error);
            }

            let sessions = sessions?;
            let invitation = invitation.ok();
            let models = models?;
            let plans = plans?;
            let packs = packs?;
            let credits = credits?;
            let ledger = ledger?;
            let orders = orders?;
            let owner_billing = owner_billing?;

            if can_read_finance {
                if let Some(summary) = owner_billing.as_ref() {
                    account.membership = Some(summary.membership.clone());
                    account.credits = Some(summary.credits.clone());
                }
                if let Some(credits) = credits {
                    account.credits = Some(credits);
                }
            } else {
                account.membership = None;
                account.credits = None;
            }

            let (ledger, ledger_next_cursor) = match ledger {
                Some(page) => (Some(page.items), page.next_cursor),
                None => (None, None),
            };
            Ok(BackendSnapshot {
                account,
                models,
                plans,
                packs,
                ledger,
                ledger_next_cursor,
                orders,
                owner_billing,
                sessions,
                invitation,
            })
        })
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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
        let response = self
            .client
            .authenticated_json_epoch::<Vec<CreditLedgerItem>>(
                Method::GET,
                &path,
                None,
                None,
                auth_epoch,
            )?;
        Ok(credit_ledger_page(response))
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn credit_account_scoped(
        &self,
        scope: &SessionScope,
    ) -> Result<CreditAccount, ApiError> {
        self.client
            .authenticated_json_scoped::<CreditAccount>(
                Method::GET,
                "/v1/credits/account",
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn redeem_credit_code_scoped(
        &self,
        code: &str,
        client_request_id: &str,
        scope: &SessionScope,
    ) -> Result<CreditRedemptionResult, ApiError> {
        let body = serde_json::to_value(CreditRedemptionRequest {
            code,
            client_request_id,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .authenticated_json_scoped::<CreditRedemptionResult>(
                Method::POST,
                "/v1/credits/redemptions",
                Some(body),
                Some(client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn ledger_page_billing(
        &self,
        cursor: Option<&str>,
        limit: usize,
        scope: &BillingScope,
    ) -> Result<CreditLedgerPage, ApiError> {
        let mut url = Url::parse("http://desktop.invalid/v1/credits/ledger").map_err(|error| {
            ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            }
        })?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", &limit.to_string());
            if let Some(cursor) = cursor {
                query.append_pair("cursor", cursor);
            }
        }
        let path = format!("{}?{}", url.path(), url.query().unwrap_or_default());
        let response = self.client.billing_json_scoped::<Vec<CreditLedgerItem>>(
            Method::GET,
            &path,
            None,
            None,
            scope,
        )?;
        Ok(credit_ledger_page(response))
    }

    pub(crate) fn credit_account_billing(
        &self,
        scope: &BillingScope,
    ) -> Result<CreditAccount, ApiError> {
        self.client
            .billing_json_scoped::<CreditAccount>(
                Method::GET,
                "/v1/credits/account",
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn redeem_credit_code_billing(
        &self,
        code: &str,
        client_request_id: &str,
        scope: &BillingScope,
    ) -> Result<CreditRedemptionResult, ApiError> {
        let body = serde_json::to_value(CreditRedemptionRequest {
            code,
            client_request_id,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .billing_json_scoped::<CreditRedemptionResult>(
                Method::POST,
                "/v1/credits/redemptions",
                Some(body),
                Some(client_request_id),
                scope,
            )
            .map(|response| response.data)
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

    pub(crate) fn request_password_code_scoped(
        &self,
        scope: &SessionScope,
    ) -> Result<PasswordCodeResponse, ApiError> {
        self.client
            .authenticated_json_scoped::<PasswordCodeResponse>(
                Method::POST,
                "/v1/account/password/code",
                Some(serde_json::json!({})),
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn set_password_scoped(
        &self,
        new_password: &str,
        current_password: Option<&str>,
        email_code: Option<&str>,
        scope: &SessionScope,
    ) -> Result<PasswordMutationResponse, ApiError> {
        let body = serde_json::to_value(PasswordMutationRequest {
            new_password,
            current_password,
            email_code,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .authenticated_json_scoped::<PasswordMutationResponse>(
                Method::PUT,
                "/v1/account/password",
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
            .identity_json_scoped::<InvitationOverview>(
                Method::GET,
                "/v1/account/invitation",
                None,
                None,
                scope,
            )?
            .data;
        let users = self
            .client
            .identity_json_scoped::<InvitationUserList>(
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
            .authenticated_json_scoped::<InvitationUserList>(Method::GET, &path, None, None, scope)
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

fn join_optional_snapshot<T>(
    handle: Option<std::thread::ScopedJoinHandle<'_, Result<T, ApiError>>>,
) -> Result<Option<T>, ApiError> {
    match handle {
        Some(handle) => join_snapshot(handle).and_then(|value| value).map(Some),
        None => Ok(None),
    }
}

pub(crate) fn validate_snapshot_context(
    account: &AccountSnapshot,
    scope: &BillingScope,
) -> Result<(), ApiError> {
    let identity_matches = account.user.id == scope.request.session.owner_user_id;
    let group_matches = account.billing_group.group_id == scope.request.account_group_id;
    let context_is_readable = account.billing_group.readable_context;
    let active_is_selectable = account.billing_group.selectable
        && account.billing_group.group_status == "active"
        && matches!(account.billing_group.role.as_str(), "owner" | "member")
        && !account.read_only;
    let frozen_owner_fallback = account.billing_group.role == "owner"
        && account.billing_group.group_status == "frozen"
        && context_is_readable
        && !account.billing_group.selectable
        && account.read_only
        && !account.billing_group.has_capability(KnownCapability::Bill)
        && !account.billing_group.has_capability(KnownCapability::Purchase)
        && !account.billing_group.has_capability(KnownCapability::Redeem);
    let capabilities_match = account.capabilities.iter().collect::<std::collections::BTreeSet<_>>()
        == account.billing_group.capabilities.iter().collect::<std::collections::BTreeSet<_>>();
    if identity_matches
        && group_matches
        && context_is_readable
        && capabilities_match
        && (active_is_selectable || frozen_owner_fallback)
    {
        return Ok(());
    }
    Err(ApiError::Protocol {
        message: "服务端返回的账号上下文与已确认计费账号不一致".to_string(),
        request_id: None,
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
    use crate::runtime::api::session::test_support::MemoryRefreshTokenStore;
    use crate::runtime::api::{
        ApiClientConfig, ApiMeta, ApiResponse, DeviceIdentity, GroupRequestScope, SessionManager,
        TokenSet,
    };
    use crate::runtime::backend_generation::billing_capture_test_support::read_request_bytes;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    const TEST_USER_ID: &str = "11111111-1111-4111-8111-111111111111";
    const TEST_GROUP_ID: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn core_snapshot_requires_authoritative_flags_and_rejects_contradictions() {
        let mut wire = member_account_snapshot_json(vec!["bill", "future_read"]);
        wire["read_only"] = serde_json::json!(false);
        wire["capabilities"] = serde_json::json!(["bill", "future_read"]);
        let scope = BillingScope { request: GroupRequestScope {
            session: SessionScope { owner_user_id: TEST_USER_ID.into(), auth_epoch: 1 },
            account_group_id: TEST_GROUP_ID.into(),
        }, context_epoch: 1 };
        let parsed: AccountSnapshot = serde_json::from_value(wire.clone()).unwrap();
        assert!(!parsed.read_only);
        assert_eq!(parsed.capabilities, ["bill", "future_read"]);
        validate_snapshot_context(&parsed, &scope).unwrap();
        for field in ["read_only", "capabilities"] {
            let mut missing = wire.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<AccountSnapshot>(missing).is_err());
        }
        for invalid in [serde_json::json!(null), serde_json::json!("false"), serde_json::json!(0)] {
            let mut invalid_wire = wire.clone(); invalid_wire["read_only"] = invalid;
            assert!(serde_json::from_value::<AccountSnapshot>(invalid_wire).is_err());
        }
        wire["read_only"] = serde_json::json!(true);
        assert!(validate_snapshot_context(&serde_json::from_value(wire.clone()).unwrap(), &scope).is_err());
        wire["read_only"] = serde_json::json!(false);
        wire["capabilities"] = serde_json::json!(["bill", "manage_group"]);
        assert!(validate_snapshot_context(&serde_json::from_value(wire).unwrap(), &scope).is_err());
    }

    fn member_account_snapshot_json(capabilities: Vec<&str>) -> Value {
        serde_json::json!({
            "read_only": false,
            "capabilities": capabilities,
            "user": {
                "id": TEST_USER_ID,
                "email_masked": "m***@example.com",
                "nickname": "Member",
                "status": "active",
                "registered_at": "2026-09-05T00:00:00.000Z",
                "invitation_code_submitted": false
            },
            "auth_methods": {
                "email": {"bound": true},
                "wechat": {"bound": false, "can_unbind": false, "nickname": null},
                "password": {"set": true}
            },
            "billing_group": {
                "group_id": TEST_GROUP_ID,
                "name": "Studio Team",
                "group_status": "active",
                "role": "member",
                "member_id": "33333333-3333-4333-8333-333333333333",
                "relationship_status": "active",
                "readable_context": true,
                "selectable": true,
                "group_version": "4",
                "membership_version": "8",
                "capabilities": capabilities,
                "quota": null
            },
            "entitlement": {
                "plan_code": "free", "tier_rank": 0, "max_quality": "4K",
                "max_concurrent_tasks": 5, "membership_period_public_id": null,
                "expires_at": null
            },
            "quota": {
                "period_start": "2026-09-01T00:00:00Z",
                "period_end": "2026-10-01T00:00:00Z",
                "monthly_limit": "500",
                "settled": "120",
                "reserved": "30",
                "remaining": "350"
            }
        })
    }

    #[test]
    fn member_snapshot_has_quota_but_no_group_wallet() {
        let snapshot: AccountSnapshot =
            serde_json::from_value(member_account_snapshot_json(vec!["bill"])).unwrap();
        assert_eq!(snapshot.billing_group.role, "member");
        assert!(snapshot.quota.is_some());
        assert!(snapshot.credits.is_none());
        assert!(snapshot.membership.is_none());
        assert!(snapshot.billing_group.selectable);
        let wire = member_account_snapshot_json(vec!["bill"]);
        assert!(wire.get("membership").is_none());
        assert!(!wire.to_string().contains("ends_at"));
        assert!(!wire.to_string().contains("\"plan\""));
    }

    fn owner_account_snapshot_json(
        group_status: &str,
        selectable: bool,
        capabilities: Vec<&str>,
    ) -> Value {
        serde_json::json!({
            "read_only": !selectable,
            "capabilities": capabilities,
            "user": {
                "id": TEST_USER_ID,
                "email_masked": "o***@example.com",
                "nickname": "Owner",
                "status": "active",
                "registered_at": "2026-09-05T00:00:00.000Z",
                "invitation_code_submitted": false
            },
            "auth_methods": {
                "email": {"bound": true},
                "wechat": {"bound": false, "can_unbind": false, "nickname": null},
                "password": {"set": true}
            },
            "membership": {
                "revision": "3",
                "period_id": null,
                "starts_at": null,
                "ends_at": null,
                "plan": null
            },
            "billing_group": {
                "group_id": TEST_GROUP_ID,
                "name": "Owner Studio",
                "group_status": group_status,
                "role": "owner",
                "member_id": null,
                "relationship_status": null,
                "readable_context": true,
                "selectable": selectable,
                "group_version": "4",
                "membership_version": null,
                "capabilities": capabilities,
                "quota": null
            },
            "entitlement": {
                "plan_code": "free", "tier_rank": 0, "max_quality": "4K",
                "max_concurrent_tasks": 5, "membership_period_public_id": null,
                "expires_at": null
            },
            "credits": {
                "available": "1000",
                "reserved": "10",
                "lifetime_granted": "1200",
                "lifetime_spent": "190",
                "version": "8"
            }
        })
    }

    fn active_owner_snapshot_json(capabilities: Vec<&str>) -> Value {
        owner_account_snapshot_json("active", true, capabilities)
    }

    fn frozen_owner_snapshot_json(capabilities: Vec<&str>) -> Value {
        owner_account_snapshot_json("frozen", false, capabilities)
    }

    #[derive(Debug)]
    struct SnapshotRequest {
        target: String,
        account_group_id: Option<String>,
    }

    impl SnapshotRequest {
        fn parse(raw: &[u8]) -> Self {
            let request = String::from_utf8_lossy(raw);
            let mut lines = request.lines();
            let target = lines
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default()
                .to_string();
            let account_group_id = lines
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("x-account-group-id"))
                .map(|(_, value)| value.trim().to_string());
            Self {
                target,
                account_group_id,
            }
        }
    }

    struct SnapshotCapture {
        base_url: String,
        requests: Arc<Mutex<Vec<SnapshotRequest>>>,
        worker: Option<JoinHandle<()>>,
    }

    impl SnapshotCapture {
        fn serve(account: Value, maximum_requests: usize) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::with_capacity(maximum_requests)));
            let captured = requests.clone();
            let worker = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                while captured.lock().unwrap().len() < maximum_requests && Instant::now() < deadline
                {
                    let (mut stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("snapshot capture accept failed: {error}"),
                    };
                    // Accepted sockets can inherit nonblocking mode on macOS.
                    // Reuse the bounded recorder that also waits for complete,
                    // possibly fragmented headers instead of a single read.
                    let raw = read_request_bytes(&mut stream);
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let request = SnapshotRequest::parse(&raw);
                    let target = request.target.clone();
                    captured.lock().unwrap().push(request);
                    let (data, meta) = snapshot_response(&target, &account);
                    let body = serde_json::json!({
                        "request_id": "snapshot-capture",
                        "data": data,
                        "error": null,
                        "meta": meta
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
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

        fn finish(mut self) -> Vec<SnapshotRequest> {
            self.worker.take().unwrap().join().unwrap();
            Arc::try_unwrap(self.requests)
                .unwrap()
                .into_inner()
                .unwrap()
        }
    }

    fn snapshot_response(target: &str, account: &Value) -> (Value, Value) {
        let credit_account = || {
            serde_json::json!({
                "available": "1000", "reserved": "10", "lifetime_granted": "1200",
                "lifetime_spent": "190", "version": "8"
            })
        };
        let membership = || {
            serde_json::json!({
                "revision": "3", "period_id": null, "starts_at": null,
                "ends_at": null, "plan": null
            })
        };
        match target {
            "/v1/account" => (account.clone(), Value::Null),
            "/v1/account/sessions" => (serde_json::json!({"items": []}), Value::Null),
            "/v1/account/invitation" => (
                serde_json::json!({
                    "enabled": true,
                    "reward_type": "credits",
                    "reward_rate_bps": 100,
                    "reward_rate_percent": "1",
                    "invitation_code": null,
                    "invitation_count": 0,
                    "total_reward_credits": "0",
                    "pending_reward_credits": "0",
                    "reversed_reward_credits": "0",
                    "reversal_debt_credits": "0",
                    "rule_description": "",
                }),
                Value::Null,
            ),
            "/v1/account/invitations?limit=50" => (
                serde_json::json!({"items": [], "next_cursor": null}),
                Value::Null,
            ),
            "/v1/models" => (serde_json::json!({"items": []}), Value::Null),
            "/v1/membership/plans" | "/v1/credits/packs" => (serde_json::json!([]), Value::Null),
            "/v1/credits/account" => (credit_account(), Value::Null),
            "/v1/credits/ledger?limit=8" => (
                serde_json::json!([]),
                serde_json::json!({"next_cursor": null}),
            ),
            "/v1/orders?page_size=50" => (
                serde_json::json!({"items": []}),
                serde_json::json!({"next_cursor": null}),
            ),
            path if path == format!("/v1/account-groups/{TEST_GROUP_ID}/billing-summary") => (
                serde_json::json!({
                    "group_id": TEST_GROUP_ID,
                    "group_version": "4",
                    "credits": credit_account(),
                    "membership": membership()
                }),
                Value::Null,
            ),
            other => panic!("unexpected snapshot path: {other}"),
        }
    }

    fn snapshot_client(base_url: &str) -> ApiClient {
        let client = ApiClient::new(
            ApiClientConfig {
                base_url: Url::parse(base_url).unwrap(),
                app_version: "1.0.22".to_string(),
                timeout: Duration::from_secs(2),
            },
            DeviceIdentity {
                id: Uuid::new_v4().to_string(),
                name: "snapshot-test".to_string(),
                platform: "macos".to_string(),
            },
            Arc::new(SessionManager::new(Arc::new(
                MemoryRefreshTokenStore::default(),
            ))),
        )
        .unwrap();
        client
            .session()
            .install_tokens_for_user(
                &TokenSet {
                    access_token: "access".to_string(),
                    access_expires_in_seconds: 1800,
                    refresh_token: "refresh".to_string(),
                    refresh_expires_at: "2099-01-01T00:00:00Z".to_string(),
                    token_type: "X-Token".to_string(),
                },
                TEST_USER_ID,
            )
            .unwrap();
        client
    }

    struct CapturedSnapshot {
        snapshot: BackendSnapshot,
        requests: Vec<SnapshotRequest>,
    }

    impl CapturedSnapshot {
        fn paths(&self) -> Vec<String> {
            self.requests
                .iter()
                .map(|request| request.target.clone())
                .collect()
        }

        fn billing_paths(&self) -> Vec<String> {
            let mut paths = self
                .requests
                .iter()
                .filter(|request| request.account_group_id.as_deref() == Some(TEST_GROUP_ID))
                .map(|request| request.target.clone())
                .collect::<Vec<_>>();
            paths.sort();
            paths
        }

        fn identity_paths(&self) -> Vec<String> {
            let mut paths = self
                .requests
                .iter()
                .filter(|request| request.account_group_id.is_none())
                .map(|request| request.target.clone())
                .collect::<Vec<_>>();
            paths.sort();
            paths
        }
    }

    fn capture_snapshot(account: Value, maximum_requests: usize) -> CapturedSnapshot {
        let capture = SnapshotCapture::serve(account, maximum_requests);
        let client = snapshot_client(&capture.base_url);
        let session = client.session().scope_for_user(TEST_USER_ID).unwrap();
        let scope = BillingScope {
            request: GroupRequestScope {
                session,
                account_group_id: TEST_GROUP_ID.to_string(),
            },
            context_epoch: 4,
        };
        let snapshot = AccountApi::new(client).snapshot_billing(&scope);
        let requests = capture.finish();
        let snapshot = snapshot.unwrap();
        assert_eq!(requests.len(), maximum_requests);
        assert_eq!(requests[0].target, "/v1/account");
        assert_eq!(requests[0].account_group_id.as_deref(), Some(TEST_GROUP_ID));
        CapturedSnapshot { snapshot, requests }
    }

    #[test]
    fn snapshot_loads_only_routes_allowed_for_the_confirmed_context() {
        let member = capture_snapshot(member_account_snapshot_json(vec!["bill"]), 5);
        assert_eq!(member.billing_paths(), ["/v1/account", "/v1/models"]);
        assert!(member.snapshot.account.credits.is_none());
        assert!(member.paths().iter().all(|path| !matches!(
            path.as_str(),
            "/v1/credits/account"
                | "/v1/credits/ledger?limit=8"
                | "/v1/orders?page_size=50"
                | "/v1/credits/packs"
                | "/v1/membership/plans"
        )));

        let frozen = capture_snapshot(
            frozen_owner_snapshot_json(vec!["read_group_finance", "read_group_usage"]),
            5,
        );
        assert_eq!(frozen.billing_paths(), ["/v1/account"]);
        assert!(frozen.identity_paths().contains(&format!(
            "/v1/account-groups/{TEST_GROUP_ID}/billing-summary"
        )));
        assert!(!frozen.paths().iter().any(|path| path == "/v1/models"));

        let owner = capture_snapshot(
            active_owner_snapshot_json(vec!["bill", "purchase", "read_group_finance"]),
            11,
        );
        assert!(owner
            .billing_paths()
            .contains(&"/v1/credits/account".to_string()));
        assert!(owner
            .billing_paths()
            .contains(&"/v1/orders?page_size=50".to_string()));
        assert!(owner.requests.iter().all(|request| {
            let is_selected_route = request.account_group_id.as_deref() == Some(TEST_GROUP_ID);
            is_selected_route
                || matches!(
                    request.target.as_str(),
                    "/v1/account/sessions"
                        | "/v1/account/invitation"
                        | "/v1/account/invitations?limit=50"
                )
                || request.target == format!("/v1/account-groups/{TEST_GROUP_ID}/billing-summary")
        }));

        let unknown = capture_snapshot(member_account_snapshot_json(vec!["future_capability"]), 4);
        assert_eq!(unknown.billing_paths(), ["/v1/account"]);
        assert!(unknown.snapshot.models.is_none());
        assert!(unknown.snapshot.account.credits.is_none());
        assert_eq!(unknown.snapshot.account.quota.unwrap().remaining, "350");
    }

    #[test]
    fn snapshot_rejects_mismatched_context_before_starting_siblings() {
        let mut mismatched_user = member_account_snapshot_json(vec!["bill"]);
        mismatched_user["user"]["id"] = serde_json::json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let mut mismatched_group = member_account_snapshot_json(vec!["bill"]);
        mismatched_group["billing_group"]["group_id"] =
            serde_json::json!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");

        for (kind, account) in [
            ("user", mismatched_user),
            ("billing group", mismatched_group),
        ] {
            let capture = SnapshotCapture::serve(account, 1);
            let client = snapshot_client(&capture.base_url);
            let session = client.session().scope_for_user(TEST_USER_ID).unwrap();
            let scope = BillingScope {
                request: GroupRequestScope {
                    session,
                    account_group_id: TEST_GROUP_ID.to_string(),
                },
                context_epoch: 4,
            };

            let error = AccountApi::new(client)
                .snapshot_billing(&scope)
                .expect_err("mismatched snapshot context must fail closed");
            assert!(
                matches!(error, ApiError::Protocol { .. }),
                "{kind} mismatch returned {error:?}"
            );
            let requests = capture.finish();
            assert_eq!(requests.len(), 1, "{kind} mismatch started a sibling");
            assert_eq!(requests[0].target, "/v1/account");
            assert_eq!(requests[0].account_group_id.as_deref(), Some(TEST_GROUP_ID));
        }
    }

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

        let selected =
            preferred_session_snapshot_error([Some(&authentication_required), Some(&terminal)])
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
    fn password_change_request_omits_the_unused_verification_method() {
        let body = serde_json::to_value(PasswordMutationRequest {
            new_password: "a sufficiently long passphrase",
            current_password: Some("the current long passphrase"),
            email_code: None,
        })
        .expect("serialize password mutation");

        assert!(body.get("current_password").is_some());
        assert!(body.get("email_code").is_none());
    }

    #[test]
    fn account_auth_methods_default_missing_password_to_unset() {
        let methods: AccountAuthMethods = serde_json::from_value(serde_json::json!({
            "email": {},
            "wechat": {}
        }))
        .expect("deserialize legacy auth methods");

        assert!(!methods.password.set);

        let methods: AccountAuthMethods = serde_json::from_value(serde_json::json!({
            "password": { "set": true }
        }))
        .expect("deserialize password auth method");

        assert!(methods.password.set);
    }

    #[test]
    fn credit_redemption_request_serializes_code_and_idempotency_id() {
        let body = serde_json::to_value(CreditRedemptionRequest {
            code: "SUMMER-2026",
            client_request_id: "73b25c73-f694-4c26-8f89-46a12a48a471",
        })
        .expect("credit-redemption request should serialize");

        assert_eq!(
            body,
            serde_json::json!({
                "code": "SUMMER-2026",
                "client_request_id": "73b25c73-f694-4c26-8f89-46a12a48a471"
            })
        );
    }

    #[test]
    fn credit_redemption_response_preserves_decimal_strings() {
        let result: CreditRedemptionResult = serde_json::from_value(serde_json::json!({
            "redemption_id": "redemption-1",
            "credits_granted": "500",
            "redeemed_at": "2026-08-13T08:00:00.000Z",
            "credit_expires_at": null,
            "account": {
                "available": "9007199254740993",
                "reserved": "20",
                "lifetime_granted": "9007199254741493",
                "lifetime_spent": "480",
                "version": "42"
            }
        }))
        .expect("credit-redemption response should deserialize");

        assert_eq!(result.redemption_id, "redemption-1");
        assert_eq!(result.credits_granted, "500");
        assert_eq!(result.redeemed_at, "2026-08-13T08:00:00.000Z");
        assert_eq!(result.credit_expires_at, None);
        assert_eq!(result.account.available, "9007199254740993");
        assert_eq!(result.account.reserved, "20");
        assert_eq!(result.account.lifetime_granted, "9007199254741493");
        assert_eq!(result.account.lifetime_spent, "480");
        assert_eq!(result.account.version, "42");
    }

    #[test]
    fn account_membership_requires_nullable_keys_to_be_present() {
        let exact = serde_json::json!({
            "revision": "3",
            "period_id": null,
            "starts_at": null,
            "ends_at": null,
            "plan": null
        });
        let membership: AccountMembership = serde_json::from_value(exact.clone())
            .expect("explicit null membership fields are valid");
        assert_eq!(membership.period_id, None);
        assert_eq!(membership.starts_at, None);
        assert_eq!(membership.ends_at, None);
        assert!(membership.plan.is_none());

        for field in ["period_id", "starts_at", "ends_at", "plan"] {
            let mut missing = exact.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<AccountMembership>(missing).is_err(),
                "membership accepted missing nullable field {field}"
            );
        }

        let mut unknown = exact.clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<AccountMembership>(unknown).is_err());

        for (field, wrong) in [
            ("period_id", serde_json::json!(3)),
            ("starts_at", serde_json::json!(false)),
            ("ends_at", serde_json::json!({})),
            ("plan", serde_json::json!("pro")),
        ] {
            let mut mismatched = exact.clone();
            mismatched
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), wrong);
            assert!(
                serde_json::from_value::<AccountMembership>(mismatched).is_err(),
                "membership accepted wrong type for {field}"
            );
        }

        let raw = serde_json::to_string(&exact).unwrap();
        let duplicate = format!("{{\"period_id\":\"duplicate\",{}", &raw[1..]);
        assert!(serde_json::from_str::<AccountMembership>(&duplicate).is_err());
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
