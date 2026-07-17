use super::{ApiClient, ApiError};
use reqwest::Method;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreditPack {
    pub(crate) code: String,
    pub(crate) name: String,
    pub(crate) price_cents: String,
    pub(crate) credits: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct PaymentCheckout {
    pub(crate) status: String,
    pub(crate) checkout_url: Option<String>,
    pub(crate) checkout_expires_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct OrderDetail {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) fulfillment_status: String,
    pub(crate) payable_amount_cents: String,
    pub(crate) payment: Option<PaymentCheckout>,
}

#[derive(Serialize)]
struct CreditOrderRequest<'a> {
    pack_code: &'a str,
    client_request_id: &'a str,
}

#[derive(Clone)]
pub(crate) struct PaymentApi {
    client: ApiClient,
}

impl PaymentApi {
    pub(crate) fn new(client: ApiClient) -> Self { Self { client } }

    pub(crate) fn packs(&self) -> Result<Vec<CreditPack>, ApiError> {
        self.client.authenticated_json::<Vec<CreditPack>>(
            Method::GET, "/v1/credits/packs", None, None,
        ).map(|response| response.data)
    }

    pub(crate) fn create_credit_order(
        &self,
        pack_code: &str,
        client_request_id: &str,
    ) -> Result<OrderDetail, ApiError> {
        let body = serde_json::to_value(CreditOrderRequest { pack_code, client_request_id })
            .map_err(|error| ApiError::Protocol { message: error.to_string(), request_id: None })?;
        self.client.authenticated_json::<OrderDetail>(
            Method::POST,
            "/v1/credits/orders",
            Some(body),
            Some(client_request_id),
        ).map(|response| response.data)
    }

    pub(crate) fn sync_order(&self, order_id: &str) -> Result<OrderDetail, ApiError> {
        self.client.authenticated_json::<OrderDetail>(
            Method::POST,
            &format!("/v1/orders/{order_id}/sync"),
            None,
            None,
        ).map(|response| response.data)
    }


    pub(crate) fn order(&self, order_id: &str) -> Result<OrderDetail, ApiError> {
        self.client.authenticated_json::<OrderDetail>(
            Method::GET,
            &format!("/v1/orders/{order_id}"),
            None,
            None,
        ).map(|response| response.data)
    }
}
