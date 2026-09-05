use super::{ApiClient, ApiError, BillingScope, SessionScope, TeamItems, TeamPage};
use reqwest::Method;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreditPack {
    pub(crate) code: String,
    pub(crate) name: String,
    pub(crate) price_cents: String,
    pub(crate) payable_price_cents: Option<String>,
    pub(crate) discount_amount_cents: Option<String>,
    pub(crate) recharge_discount_bps: Option<i32>,
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
    pub(crate) billing_account_group_id: String,
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
    pub(crate) fn new(client: ApiClient) -> Self {
        Self { client }
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn packs(&self) -> Result<Vec<CreditPack>, ApiError> {
        self.client
            .authenticated_json::<Vec<CreditPack>>(Method::GET, "/v1/credits/packs", None, None)
            .map(|response| response.data)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn packs_epoch(&self, auth_epoch: u64) -> Result<Vec<CreditPack>, ApiError> {
        self.client
            .authenticated_json_epoch::<Vec<CreditPack>>(
                Method::GET,
                "/v1/credits/packs",
                None,
                None,
                auth_epoch,
            )
            .map(|response| response.data)
    }

    pub(crate) fn packs_billing(&self, scope: &BillingScope) -> Result<Vec<CreditPack>, ApiError> {
        self.client
            .billing_json_scoped::<Vec<CreditPack>>(
                Method::GET,
                "/v1/credits/packs",
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_credit_order(
        &self,
        pack_code: &str,
        client_request_id: &str,
    ) -> Result<OrderDetail, ApiError> {
        let body = serde_json::to_value(CreditOrderRequest {
            pack_code,
            client_request_id,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .authenticated_json::<OrderDetail>(
                Method::POST,
                "/v1/credits/orders",
                Some(body),
                Some(client_request_id),
            )
            .map(|response| response.data)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_credit_order_scoped(
        &self,
        pack_code: &str,
        client_request_id: &str,
        scope: &SessionScope,
    ) -> Result<OrderDetail, ApiError> {
        let body = serde_json::to_value(CreditOrderRequest {
            pack_code,
            client_request_id,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .authenticated_json_scoped::<OrderDetail>(
                Method::POST,
                "/v1/credits/orders",
                Some(body),
                Some(client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_credit_order_billing(
        &self,
        pack_code: &str,
        client_request_id: &str,
        scope: &BillingScope,
    ) -> Result<OrderDetail, ApiError> {
        let body = serde_json::to_value(CreditOrderRequest {
            pack_code,
            client_request_id,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .billing_json_scoped::<OrderDetail>(
                Method::POST,
                "/v1/credits/orders",
                Some(body),
                Some(client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn orders_billing(
        &self,
        cursor: Option<&str>,
        scope: &BillingScope,
    ) -> Result<TeamPage<OrderDetail>, ApiError> {
        let path = super::team::page_path("/v1/orders", cursor)?;
        let response = self.client.billing_json_scoped::<TeamItems<OrderDetail>>(
            Method::GET,
            &path,
            None,
            None,
            scope,
        )?;
        Ok(TeamPage {
            items: response.data.items,
            next_cursor: response.meta.and_then(|meta| meta.next_cursor),
        })
    }

    pub(crate) fn sync_order(&self, order_id: &str) -> Result<OrderDetail, ApiError> {
        self.client
            .authenticated_json::<OrderDetail>(
                Method::POST,
                &format!("/v1/orders/{order_id}/sync"),
                None,
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn sync_order_scoped(
        &self,
        order_id: &str,
        scope: &SessionScope,
    ) -> Result<OrderDetail, ApiError> {
        self.client
            .identity_json_scoped::<OrderDetail>(
                Method::POST,
                &format!("/v1/orders/{order_id}/sync"),
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn order(&self, order_id: &str) -> Result<OrderDetail, ApiError> {
        self.client
            .authenticated_json::<OrderDetail>(
                Method::GET,
                &format!("/v1/orders/{order_id}"),
                None,
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn order_scoped(
        &self,
        order_id: &str,
        scope: &SessionScope,
    ) -> Result<OrderDetail, ApiError> {
        self.client
            .identity_json_scoped::<OrderDetail>(
                Method::GET,
                &format!("/v1/orders/{order_id}"),
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_detail_requires_billing_account_group_id() {
        let order = serde_json::json!({
            "id": "order-1", "status": "pending",
            "fulfillment_status": "pending", "payable_amount_cents": "100",
            "payment": null
        });
        assert!(serde_json::from_value::<OrderDetail>(order.clone()).is_err());
        let mut order_null = order;
        order_null["billing_account_group_id"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<OrderDetail>(order_null).is_err());
    }
}
