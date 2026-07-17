use super::{ApiClient, ApiError, ApiResponse, CreditPack, PaymentApi};
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AccountUser {
    pub(crate) id: String,
    pub(crate) email_masked: String,
    pub(crate) nickname: Option<String>,
    pub(crate) status: String,
    pub(crate) registered_at: String,
}

#[derive(Clone, Debug, Deserialize)]
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
    pub(crate) max_quality: String,
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
    pub(crate) membership: AccountMembership,
    pub(crate) credits: Option<CreditAccount>,
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

#[derive(Clone, Debug)]
pub(crate) struct BackendSnapshot {
    pub(crate) account: AccountSnapshot,
    pub(crate) plans: Vec<MembershipPlan>,
    pub(crate) packs: Vec<CreditPack>,
    pub(crate) models: Vec<ModelCatalogItem>,
    pub(crate) ledger: Vec<CreditLedgerItem>,
    pub(crate) ledger_next_cursor: Option<String>,
    pub(crate) sessions: Vec<AccountSessionDto>,
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
struct SessionList { items: Vec<AccountSessionDto> }

#[derive(Clone)]
pub(crate) struct AccountApi {
    client: ApiClient,
}

impl AccountApi {
    pub(crate) fn new(client: ApiClient) -> Self {
        Self { client }
    }

    pub(crate) fn snapshot(&self) -> Result<BackendSnapshot, ApiError> {
        std::thread::scope(|scope| {
            let account_client = self.client.clone();
            let account = scope.spawn(move || account_client
                .authenticated_json::<AccountSnapshot>(Method::GET, "/v1/account", None, None)
                .map(|response| response.data));
            let credit_client = self.client.clone();
            let credits = scope.spawn(move || credit_client
                .authenticated_json::<CreditAccount>(Method::GET, "/v1/credits/account", None, None)
                .map(|response| response.data));
            let plan_client = self.client.clone();
            let plans = scope.spawn(move || plan_client
                .authenticated_json::<Vec<MembershipPlan>>(Method::GET, "/v1/membership/plans", None, None)
                .map(|response| response.data));
            let membership_client = self.client.clone();
            let membership = scope.spawn(move || membership_client
                .authenticated_json::<Value>(Method::GET, "/v1/membership/current", None, None)
                .map(|response| response.data));
            let pack_client = self.client.clone();
            let packs = scope.spawn(move || PaymentApi::new(pack_client).packs());
            let model_client = self.client.clone();
            let models = scope.spawn(move || model_client
                .authenticated_json::<ModelCatalog>(Method::GET, "/v1/models", None, None)
                .map(|response| response.data.items));
            let ledger_client = self.client.clone();
            let ledger = scope.spawn(move || {
                AccountApi::new(ledger_client).ledger_page(None, CREDIT_LEDGER_PAGE_SIZE)
            });
            let session_client = self.client.clone();
            let sessions = scope.spawn(move || session_client
                .authenticated_json::<SessionList>(Method::GET, "/v1/account/sessions", None, None)
                .map(|response| response.data.items));

            let mut account = join_snapshot(account)??;
            account.credits = Some(join_snapshot(credits)??);
            let plans = join_snapshot(plans)??;
            let _current_membership = join_snapshot(membership)??;
            let packs = join_snapshot(packs)??;
            let models = join_snapshot(models)??;
            let ledger_page = join_snapshot(ledger)??;
            let sessions = join_snapshot(sessions)??;
            Ok(BackendSnapshot {
                account,
                plans,
                packs,
                models,
                ledger: ledger_page.items,
                ledger_next_cursor: ledger_page.next_cursor,
                sessions,
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

    pub(crate) fn revoke_session(&self, session_id: &str) -> Result<(), ApiError> {
        self.client.authenticated_json::<serde_json::Value>(
            Method::DELETE,
            &format!("/v1/account/sessions/{session_id}"),
            None,
            None,
        )?;
        Ok(())
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
}
