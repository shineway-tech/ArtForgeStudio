use super::{
    ApiClient, ApiError, ApiResponse, BillingScope, SessionScope, TeamItems, TeamPage,
};
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
    #[serde(default)] pub(crate) bonus_credits: Option<String>,
    #[serde(default)] pub(crate) total_credits: Option<String>,
    #[serde(default)] pub(crate) promotion_id: Option<String>,
    #[serde(default)] pub(crate) promotion_title: Option<String>,
    #[serde(default)] pub(crate) promotion_description: Option<String>,
    #[serde(default)] pub(crate) promotion_label: Option<String>,
    #[serde(default)] pub(crate) promotion_ends_at: Option<String>,
    #[serde(default)] pub(crate) promotion_copy: Option<String>,
    #[serde(default)] pub(crate) promotion_deadline_label: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreditRechargeSummary {
    #[serde(default)] pub(crate) base_credits: Option<String>,
    #[serde(default)] pub(crate) bonus_credits: Option<String>,
    #[serde(default)] pub(crate) total_credits: Option<String>,
    #[serde(default)] pub(crate) pack_code: Option<String>,
    #[serde(default)] pub(crate) promotion_id: Option<String>,
    #[serde(default)] pub(crate) promotion_ends_at: Option<String>,
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
    #[serde(default)] pub(crate) credit_recharge: Option<CreditRechargeSummary>,
}

#[derive(Serialize)]
struct CreditOrderRequest<'a> {
    pack_code: &'a str,
    client_request_id: &'a str,
}

fn order_page(
    response: ApiResponse<TeamItems<OrderDetail>>,
) -> Result<TeamPage<OrderDetail>, ApiError> {
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
        order_page(response)
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
    use crate::runtime::api::session::test_support::MemoryRefreshTokenStore;
    use crate::runtime::api::{
        ApiClientConfig, DeviceIdentity, GroupRequestScope, SessionManager, TokenSet,
    };
    use reqwest::Url;
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    const TEST_USER_ID: &str = "11111111-1111-4111-8111-111111111111";
    const TEST_GROUP_ID: &str = "22222222-2222-4222-8222-222222222222";

    struct OrderServer {
        base_url: String,
        targets: Arc<Mutex<Vec<String>>>,
        worker: Option<JoinHandle<()>>,
    }

    impl OrderServer {
        fn serve(responses: Vec<Value>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let targets = Arc::new(Mutex::new(Vec::with_capacity(responses.len())));
            let captured = targets.clone();
            let worker = thread::spawn(move || {
                for response in responses {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut request = [0_u8; 4096];
                    let received = stream.read(&mut request).unwrap();
                    let request_line = String::from_utf8_lossy(&request[..received])
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    captured.lock().unwrap().push(
                        request_line
                            .split_whitespace()
                            .nth(1)
                            .unwrap_or_default()
                            .to_string(),
                    );
                    let body = response.to_string();
                    let http = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len(),
                    );
                    stream.write_all(http.as_bytes()).unwrap();
                }
            });
            Self {
                base_url: format!("http://{address}/"),
                targets,
                worker: Some(worker),
            }
        }

        fn finish(mut self) -> Vec<String> {
            self.worker.take().unwrap().join().unwrap();
            Arc::try_unwrap(self.targets)
                .unwrap()
                .into_inner()
                .unwrap()
        }
    }

    fn order_json(id: &str) -> Value {
        json!({
            "id": id,
            "billing_account_group_id": TEST_GROUP_ID,
            "status": "pending",
            "fulfillment_status": "pending",
            "payable_amount_cents": "100",
            "payment": null
        })
    }

    fn order_api(base_url: &str) -> (PaymentApi, BillingScope) {
        let session = Arc::new(SessionManager::new(Arc::new(
            MemoryRefreshTokenStore::default(),
        )));
        let session_scope = session
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
        let client = ApiClient::new(
            ApiClientConfig {
                base_url: Url::parse(base_url).unwrap(),
                app_version: "task4-order-page-test".to_string(),
                timeout: Duration::from_secs(1),
            },
            DeviceIdentity {
                id: "task4-order-page-device".to_string(),
                name: "Task 4 order page test".to_string(),
                platform: "test".to_string(),
            },
            session,
        )
        .unwrap();
        let scope = BillingScope {
            request: GroupRequestScope {
                session: session_scope,
                account_group_id: TEST_GROUP_ID.to_string(),
            },
            context_epoch: 4,
        };
        (PaymentApi::new(client), scope)
    }

    fn assert_protocol_request_id(error: ApiError) {
        match error {
            ApiError::Protocol { request_id, .. } => {
                assert_eq!(request_id.as_deref(), Some("orders-request"));
            }
            other => panic!("expected protocol error, got {other:?}"),
        }
    }

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

    #[test]
    fn orders_billing_accepts_explicit_null_cursor() {
        let server = OrderServer::serve(vec![json!({
            "request_id": "orders-request",
            "data": {"items": [order_json("order-1")]},
            "error": null,
            "meta": {"next_cursor": null}
        })]);
        let (api, scope) = order_api(&server.base_url);

        let page = api.orders_billing(None, &scope).unwrap();

        assert_eq!(page.items.len(), 1);
        assert!(page.next_cursor.is_none());
        assert_eq!(server.finish(), vec!["/v1/orders?page_size=50"]);
    }

    #[test]
    fn orders_billing_percent_encodes_opaque_cursor() {
        let server = OrderServer::serve(vec![json!({
            "request_id": "orders-request",
            "data": {"items": []},
            "error": null,
            "meta": {"next_cursor": "after/+ page"}
        })]);
        let (api, scope) = order_api(&server.base_url);

        let page = api
            .orders_billing(Some("before/+ page"), &scope)
            .unwrap();

        assert_eq!(page.next_cursor.as_deref(), Some("after/+ page"));
        assert_eq!(
            server.finish(),
            vec!["/v1/orders?page_size=50&cursor=before%2F%2B+page"]
        );
    }

    #[test]
    fn orders_billing_rejects_missing_or_null_meta_with_request_id() {
        let server = OrderServer::serve(vec![
            json!({
                "request_id": "orders-request",
                "data": {"items": []},
                "error": null
            }),
            json!({
                "request_id": "orders-request",
                "data": {"items": []},
                "error": null,
                "meta": null
            }),
        ]);
        let (api, scope) = order_api(&server.base_url);

        assert_protocol_request_id(api.orders_billing(None, &scope).unwrap_err());
        assert_protocol_request_id(api.orders_billing(None, &scope).unwrap_err());

        assert_eq!(server.finish().len(), 2);
    }

    #[test]
    fn orders_billing_rejects_more_than_fifty_items_with_request_id() {
        let items = (0..51)
            .map(|index| order_json(&format!("order-{index}")))
            .collect::<Vec<_>>();
        let server = OrderServer::serve(vec![json!({
            "request_id": "orders-request",
            "data": {"items": items},
            "error": null,
            "meta": {"next_cursor": null}
        })]);
        let (api, scope) = order_api(&server.base_url);

        assert_protocol_request_id(api.orders_billing(None, &scope).unwrap_err());

        assert_eq!(server.finish().len(), 1);
    }
}
