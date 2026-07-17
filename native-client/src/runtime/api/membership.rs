use super::{ApiClient, ApiError, OrderDetail};
use reqwest::Method;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UpgradeQuote {
    pub(crate) id: String,
    pub(crate) target_plan_code: String,
    pub(crate) payable_amount_cents: String,
    pub(crate) credit_delta: String,
    pub(crate) expires_at: String,
}

#[derive(Serialize)]
struct MembershipOrderRequest<'a> {
    plan_code: &'a str,
    client_request_id: &'a str,
}

#[derive(Serialize)]
struct UpgradeQuoteRequest<'a> {
    target_plan_code: &'a str,
}

#[derive(Serialize)]
struct UpgradeOrderRequest<'a> {
    quote_id: &'a str,
    client_request_id: &'a str,
}

#[derive(Clone)]
pub(crate) struct MembershipApi {
    client: ApiClient,
}

impl MembershipApi {
    pub(crate) fn new(client: ApiClient) -> Self { Self { client } }

    pub(crate) fn create_order(
        &self,
        plan_code: &str,
        client_request_id: &str,
    ) -> Result<OrderDetail, ApiError> {
        let body = serde_json::to_value(MembershipOrderRequest { plan_code, client_request_id })
            .map_err(protocol_error)?;
        self.client.authenticated_json::<OrderDetail>(
            Method::POST,
            "/v1/membership/orders",
            Some(body),
            Some(client_request_id),
        ).map(|response| response.data)
    }

    pub(crate) fn create_upgrade_quote(&self, target_plan_code: &str) -> Result<UpgradeQuote, ApiError> {
        let body = serde_json::to_value(UpgradeQuoteRequest { target_plan_code })
            .map_err(protocol_error)?;
        self.client.authenticated_json::<UpgradeQuote>(
            Method::POST,
            "/v1/membership/upgrade-quotes",
            Some(body),
            None,
        ).map(|response| response.data)
    }

    pub(crate) fn create_upgrade_order(
        &self,
        quote_id: &str,
        client_request_id: &str,
    ) -> Result<OrderDetail, ApiError> {
        let body = serde_json::to_value(UpgradeOrderRequest { quote_id, client_request_id })
            .map_err(protocol_error)?;
        self.client.authenticated_json::<OrderDetail>(
            Method::POST,
            "/v1/membership/upgrade-orders",
            Some(body),
            Some(client_request_id),
        ).map(|response| response.data)
    }
}

fn protocol_error(error: serde_json::Error) -> ApiError {
    ApiError::Protocol { message: error.to_string(), request_id: None }
}
