use super::{
    ApiEnvelope, ApiError, ApiResponse, BillingScope, DeviceIdentity, RefreshRequest,
    SessionManager, SessionScope, TokenSet, UpgradeLatch, RequiredUpgrade,
};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::{Method, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const DEFAULT_DEV_API_BASE_URL: &str = "https://artforge-api.honeykid.cn";
const DEFAULT_PROD_API_BASE_URL: &str = "https://artforge-api.honeykid.cn";

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestRequestReceipt {
    pub(crate) method: String, pub(crate) path: String,
    pub(crate) selected_group: Option<String>, pub(crate) has_auth: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ApiClientConfig {
    pub(crate) base_url: Url,
    pub(crate) app_version: String,
    pub(crate) timeout: Duration,
}

impl ApiClientConfig {
    pub(crate) fn from_environment() -> Result<Self, ApiError> {
        let default_url = if cfg!(debug_assertions) {
            DEFAULT_DEV_API_BASE_URL
        } else {
            DEFAULT_PROD_API_BASE_URL
        };
        let configured = if cfg!(debug_assertions) {
            std::env::var("ARTFORGE_API_BASE_URL").unwrap_or_else(|_| default_url.to_string())
        } else {
            default_url.to_string()
        };
        let mut base_url =
            Url::parse(configured.trim()).map_err(|error| ApiError::Configuration {
                message: format!("无效的后端地址：{error}"),
            })?;
        if !cfg!(debug_assertions) && base_url.scheme() != "https" {
            return Err(ApiError::Configuration {
                message: "生产环境后端地址必须使用 HTTPS".to_string(),
            });
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path().trim_end_matches('/')));
        }
        Ok(Self {
            base_url,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            timeout: Duration::from_secs(30),
        })
    }
}

#[derive(Clone)]
pub(crate) struct ApiClient {
    http: Client,
    config: ApiClientConfig,
    device: DeviceIdentity,
    session: Arc<SessionManager>,
    upgrade: UpgradeLatch,
    user_work: Arc<std::sync::OnceLock<crate::runtime::UserWorkAdmission>>,
    #[cfg(test)]
    request_receipts: Arc<std::sync::Mutex<Vec<TestRequestReceipt>>>,
}

impl ApiClient {
    pub(crate) fn new(
        config: ApiClientConfig,
        device: DeviceIdentity,
        session: Arc<SessionManager>,
    ) -> Result<Self, ApiError> {
        let http = Client::builder()
            .timeout(config.timeout)
            .user_agent(format!("ElunviCanvas/{}", config.app_version))
            .build()?;
        Ok(Self {
            http,
            config,
            device,
            session,
            upgrade: UpgradeLatch::default(),
            user_work: Arc::new(std::sync::OnceLock::new()),
            #[cfg(test)]
            request_receipts: Arc::new(std::sync::Mutex::new(Vec::new())),
        })
    }

    pub(crate) fn base_url(&self) -> &Url {
        &self.config.base_url
    }

    pub(crate) fn app_version(&self) -> &str {
        &self.config.app_version
    }

    pub(crate) fn device(&self) -> &DeviceIdentity {
        &self.device
    }

    pub(crate) fn session(&self) -> &Arc<SessionManager> {
        &self.session
    }

    pub(crate) fn upgrade_latch(&self) -> &UpgradeLatch { &self.upgrade }
    pub(crate) fn bind_user_work(&self, admission: crate::runtime::UserWorkAdmission) -> Result<(), ApiError> {
        self.user_work.set(admission).map_err(|_| ApiError::LocalState { message: "用户任务入口已绑定".into() })
    }
    pub(crate) fn begin_user_work(&self, scope: &SessionScope) -> Result<crate::runtime::UserActivityPermit, ApiError> {
        if let Some(required) = self.upgrade.snapshot() { return Err(required.as_error()); }
        if !self.session.is_scope_current(scope) { return Err(ApiError::AuthenticationRequired); }
        self.user_work.get().ok_or_else(|| ApiError::LocalState { message: "用户任务入口尚未激活".into() })?
            .begin(scope).map_err(crate::runtime::transition_error)
    }
    pub(crate) fn user_work_is_current(&self, scope: &SessionScope) -> bool {
        !self.upgrade.is_tripped() && self.session.is_scope_current(scope)
            && self.user_work.get().is_some_and(|admission| admission.is_current(scope))
    }
    pub(crate) fn replay_saved<T: DeserializeOwned>(&self, request: &crate::runtime::SavedReplayRequest) -> Result<ApiResponse<T>, ApiError> {
        let _unit = self.begin_user_work(request.session())?;
        request.verify().map_err(crate::runtime::transition_error)?;
        self.authenticated_json_with_scope(Method::POST, request.path(), Some(request.body()),
            Some(request.key()), request.session(), Some(request.payer()))
            .map_err(|error| error.with_billing_payer(request.payer()))
    }
    #[cfg(test)]
    pub(crate) fn test_request_receipts(&self) -> Vec<TestRequestReceipt> {
        self.request_receipts.lock().unwrap().clone()
    }

    pub(crate) fn refresh_persisted_owner(&self, owner: &str) -> Result<SessionScope, ApiError> {
        self.session.refresh_persisted_owner(owner, |token| self.request_refresh(token))
    }

    pub(crate) fn public_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.send_once(method, path, body, None, None, None)
    }

    pub(crate) fn public_json_idempotent<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: &str,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.send_once(
            method,
            path,
            body,
            Some(idempotency_key),
            None,
            None,
        )
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn authenticated_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
    ) -> Result<ApiResponse<T>, ApiError> {
        let auth_epoch = self.session.auth_epoch();
        let access_token = self.access_or_refresh_epoch(auth_epoch)?;
        let first = self.send_once_epoch(
            method.clone(),
            path,
            body.clone(),
            idempotency_key,
            Some(&access_token),
            auth_epoch,
        );
        match first {
            Ok(response) => Ok(response),
            Err(error) if error.is_access_token_rejected() => {
                let refreshed = self
                    .session
                    .refresh_epoch(auth_epoch, Some(&access_token), |refresh_token| {
                        self.request_refresh(refresh_token)
                    })
                    .map_err(|error| self.clear_epoch_on_terminal_error(auth_epoch, error))?;
                self.send_once(
                    method,
                    path,
                    body,
                    idempotency_key,
                    Some(&refreshed),
                    None,
                )
                .map_err(|error| self.clear_epoch_on_exhausted_auth_error(auth_epoch, error))
            }
            Err(error) => Err(error),
        }
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn authenticated_json_scoped<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        scope: &SessionScope,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.identity_json_scoped(method, path, body, idempotency_key, scope)
    }

    pub(crate) fn identity_json_scoped<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        scope: &SessionScope,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.authenticated_json_with_scope(method, path, body, idempotency_key, scope, None)
    }

    pub(crate) fn billing_json_scoped<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        scope: &BillingScope,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.authenticated_json_with_scope(
            method,
            path,
            body,
            idempotency_key,
            &scope.request.session,
            Some(&scope.request.account_group_id),
        )
        .map_err(|error| error.with_billing_payer(&scope.request.account_group_id))
    }

    fn authenticated_json_with_scope<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        scope: &SessionScope,
        account_group_id: Option<&str>,
    ) -> Result<ApiResponse<T>, ApiError> {
        let account_group_id = account_group_id.map(str::to_owned);
        // Scope validation and token cloning happen under the same session lock. If another
        // account is installed immediately afterwards, this request still carries the old
        // account's cloned token and can never borrow the new account's credentials.
        let access_token = self.access_or_refresh_scope(scope)?;
        let first = self.send_once_scope(
            method.clone(),
            path,
            body.clone(),
            idempotency_key,
            Some(&access_token),
            scope,
            account_group_id.as_deref(),
        );
        match first {
            Ok(response) => Ok(response),
            Err(error) if error.is_access_token_rejected() => {
                let refreshed = self
                    .session
                    .refresh_scope(scope, Some(&access_token), |refresh_token| {
                        self.request_refresh(refresh_token)
                    })
                    .map_err(|error| self.clear_scope_on_terminal_error(scope, error))?;
                self.send_once(
                    method,
                    path,
                    body,
                    idempotency_key,
                    Some(&refreshed),
                    account_group_id.as_deref(),
                )
                .map_err(|error| self.clear_scope_on_exhausted_auth_error(scope, error))
            }
            Err(error) => Err(error),
        }
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn authenticated_json_epoch<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        auth_epoch: u64,
    ) -> Result<ApiResponse<T>, ApiError> {
        let access_token = self.access_or_refresh_epoch(auth_epoch)?;
        let first = self.send_once_epoch(
            method.clone(),
            path,
            body.clone(),
            idempotency_key,
            Some(&access_token),
            auth_epoch,
        );
        match first {
            Ok(response) => Ok(response),
            Err(error) if error.is_access_token_rejected() => {
                let refreshed = self
                    .session
                    .refresh_epoch(auth_epoch, Some(&access_token), |refresh_token| {
                        self.request_refresh(refresh_token)
                    })
                    .map_err(|error| self.clear_epoch_on_terminal_error(auth_epoch, error))?;
                self.send_once(
                    method,
                    path,
                    body,
                    idempotency_key,
                    Some(&refreshed),
                    None,
                )
                .map_err(|error| self.clear_epoch_on_exhausted_auth_error(auth_epoch, error))
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn authenticated_json_with_fixed_token<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        access_token: &str,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.send_once(method, path, body, None, Some(access_token), None)
    }

    pub(crate) fn refresh_session(&self) -> Result<String, ApiError> {
        let auth_epoch = self.session.auth_epoch();
        self.session
            .refresh_epoch(auth_epoch, None, |refresh_token| {
                self.request_refresh(refresh_token)
            })
            .map_err(|error| self.clear_epoch_on_terminal_error(auth_epoch, error))
    }

    pub(crate) fn refresh_session_epoch(&self, auth_epoch: u64) -> Result<String, ApiError> {
        self.session
            .refresh_epoch(auth_epoch, None, |refresh_token| {
                self.request_refresh(refresh_token)
            })
            .map_err(|error| self.clear_epoch_on_terminal_error(auth_epoch, error))
    }

    fn access_or_refresh_epoch(&self, auth_epoch: u64) -> Result<String, ApiError> {
        match self.session.access_token_for_epoch(auth_epoch) {
            Ok(access_token) => Ok(access_token),
            Err(ApiError::AuthenticationRequired) if self.session.auth_epoch() == auth_epoch => {
                self.session
                    .refresh_epoch(auth_epoch, None, |refresh_token| {
                        self.request_refresh(refresh_token)
                    })
                    .map_err(|error| self.clear_epoch_on_terminal_error(auth_epoch, error))
            }
            Err(error) => Err(self.clear_epoch_on_terminal_error(auth_epoch, error)),
        }
    }

    fn access_or_refresh_scope(&self, scope: &SessionScope) -> Result<String, ApiError> {
        match self.session.access_token_for_scope(scope) {
            Ok(access_token) => Ok(access_token),
            Err(ApiError::AuthenticationRequired) if self.session.is_scope_current(scope) => self
                .session
                .refresh_scope(scope, None, |refresh_token| {
                    self.request_refresh(refresh_token)
                })
                .map_err(|error| self.clear_scope_on_terminal_error(scope, error)),
            Err(error) => Err(self.clear_scope_on_terminal_error(scope, error)),
        }
    }

    fn send_once_epoch<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        access_token: Option<&str>,
        auth_epoch: u64,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.send_once(method, path, body, idempotency_key, access_token, None)
            .map_err(|error| self.clear_epoch_on_terminal_error(auth_epoch, error))
    }

    fn send_once_scope<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        access_token: Option<&str>,
        scope: &SessionScope,
        account_group_id: Option<&str>,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.send_once(
            method,
            path,
            body,
            idempotency_key,
            access_token,
            account_group_id,
        )
        .map_err(|error| self.clear_scope_on_terminal_error(scope, error))
    }

    fn clear_epoch_on_terminal_error(&self, auth_epoch: u64, error: ApiError) -> ApiError {
        if error.is_terminal_session_error() {
            let _ = self.session.clear_epoch(auth_epoch);
        }
        error
    }

    fn clear_scope_on_terminal_error(&self, scope: &SessionScope, error: ApiError) -> ApiError {
        if error.is_terminal_session_error() {
            let _ = self.session.clear_scope(scope);
        }
        error
    }

    fn clear_epoch_on_exhausted_auth_error(&self, auth_epoch: u64, error: ApiError) -> ApiError {
        if error.is_access_token_rejected() || error.is_terminal_session_error() {
            let _ = self.session.clear_epoch(auth_epoch);
        }
        error
    }

    fn clear_scope_on_exhausted_auth_error(
        &self,
        scope: &SessionScope,
        error: ApiError,
    ) -> ApiError {
        if error.is_access_token_rejected() || error.is_terminal_session_error() {
            let _ = self.session.clear_scope(scope);
        }
        error
    }

    fn request_refresh(&self, refresh_token: &str) -> Result<TokenSet, ApiError> {
        let body = serde_json::to_value(RefreshRequest {
            refresh_token,
            device_id: &self.device.id,
            app_version: &self.config.app_version,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.public_json(Method::POST, "/v1/auth/refresh", Some(body))
            .map(|response: ApiResponse<TokenSet>| response.data)
    }

    fn send_once<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        access_token: Option<&str>,
        account_group_id: Option<&str>,
    ) -> Result<ApiResponse<T>, ApiError> {
        if account_group_id.is_some() && access_token.is_none() {
            return Err(ApiError::Protocol {
                message: "账号组请求头必须绑定已认证会话".to_string(),
                request_id: None,
            });
        }
        let url = self.endpoint(path)?;
        let request_id = Uuid::new_v4().to_string();
        let mut request = self
            .http
            .request(method, url)
            .header("X-Request-ID", &request_id);
        if let Some(access_token) = access_token {
            request = request
                .header("X-Token", access_token)
                .header("X-Client-Version", &self.config.app_version)
                .header("X-Device-ID", &self.device.id);
        }
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(account_group_id) = account_group_id {
            request = request.header("X-Account-Group-ID", account_group_id);
        }
        if let Some(value) = body {
            request = request.json(&value);
        }
        self.execute(request)
    }

    fn execute<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<ApiResponse<T>, ApiError> {
        let permit = self.upgrade.begin_ordinary_transfer().map_err(|required| required.as_error())?;
        #[cfg(test)]
        if let Some(Ok(prepared)) = request.try_clone().map(RequestBuilder::build) {
            let receipt = TestRequestReceipt {
                method: prepared.method().to_string(),
                path: match prepared.url().query() { Some(query) => format!("{}?{query}", prepared.url().path()), None => prepared.url().path().into() },
                selected_group: prepared.headers().get("X-Account-Group-ID").and_then(|value| value.to_str().ok()).map(str::to_owned),
                has_auth: prepared.headers().contains_key("X-Token"),
            };
            let mut receipts = self.request_receipts.lock().unwrap();
            assert!(receipts.len() < 4096, "test request receipt bound exceeded");
            receipts.push(receipt);
        }
        let response = request.send()?;
        let status = response.status();
        let response_request_id = response
            .headers()
            .get("X-Request-ID")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let payload = response.bytes()?;
        // Error authority is independent of the success DTO. A valid exact426
        // must not be hidden by malformed, unrelated data for T.
        let envelope = serde_json::from_slice::<ApiEnvelope<Value>>(&payload).map_err(|error| {
            ApiError::Protocol {
                message: format!("无法解析服务端响应：{error}"),
                request_id: response_request_id.clone(),
            }
        })?;
        if !status.is_success() || envelope.error.is_some() {
            let problem = envelope.error.unwrap_or(super::ApiProblem {
                code: "request_error".to_string(),
                message: format!("HTTP {}", status.as_u16()),
                details: None,
            });
            let error = ApiError::Http {
                status: status.as_u16(),
                code: problem.code,
                message: problem.message,
                request_id: Some(envelope.request_id),
                details: problem.details,
            };
            if let Some(required) = RequiredUpgrade::from_error(&error) {
                let safe_error = required.as_error();
                self.upgrade.trip_from_ordinary_transfer(permit, required, || drop((payload, envelope.data, envelope.meta, error)));
                return Err(safe_error);
            }
            return Err(error);
        }
        // Decode successful data from its original stream: a Value intermediate
        // would erase duplicate security fields before their DTO visitors run.
        let envelope = serde_json::from_slice::<ApiEnvelope<T>>(&payload).map_err(|error| ApiError::Protocol {
            message: format!("无法解析服务端响应：{error}"),
            request_id: Some(envelope.request_id.clone()),
        })?;
        let data = envelope.data.ok_or_else(|| ApiError::Protocol {
            message: "成功响应缺少 data 字段".to_string(),
            request_id: Some(envelope.request_id.clone()),
        })?;
        let result = ApiResponse {
            request_id: envelope.request_id,
            data,
            meta: envelope.meta,
        };
        self.upgrade.apply_if_open(|| result).map_err(|required| required.as_error())
    }

    fn endpoint(&self, path: &str) -> Result<Url, ApiError> {
        self.config
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| ApiError::Configuration {
                message: format!("无法构造接口地址：{error}"),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::api::session::test_support::MemoryRefreshTokenStore;
    use crate::runtime::api::{
        AccountApi, BillingScope, CreateGenerationTask, CreateImageColorization, CreateImageCutout,
        CreateImageEditTask, CreateImageEnhancement, CreatePromptOptimization,
        CreateUpscaleGenerationTask, CreateVideoGenerationTask, CreateVideoQuote,
        CreateWatermarkRemoval, CreditAccount, CreditLedgerPage, CreditPack,
        CreditRedemptionResult, GenerationApi, GenerationTaskDetail, GroupRequestScope,
        MembershipApi, OrderDetail, PaymentApi, PromptOptimizationApi, PromptOptimizationDetail,
        SessionScope, TeamPage, UpgradeQuote, VideoQuote,
    };
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;
    use std::thread::{self, JoinHandle};

    const TEST_USER_ID: &str = "11111111-1111-4111-8111-111111111111";
    const TEST_GROUP_ID: &str = "22222222-2222-4222-8222-222222222222";
    const TEST_TASK_ID: &str = "task-matrix";
    const TEST_PROMPT_ID: &str = "prompt-matrix";
    const TEST_ORDER_ID: &str = "order-matrix";
    const TEST_QUOTE_ID: &str = "quote-matrix";
    #[test]
    fn core_saved_generation_lookup_rejects_wrong_payer_without_selected_header() {
        let captured = CapturedRequests::serve_json_values(vec![generation_detail_json()]);
        let client = authenticated_test_client(captured.base_url.clone());
        let scope = client.session().scope_for_user(TEST_USER_ID).unwrap();
        let api = GenerationApi::new(client).with_saved_group("33333333-3333-4333-8333-333333333333");
        assert!(api.task_scoped(TEST_TASK_ID, &scope).is_err());
        let receipts = captured.finish();
        assert_eq!(receipts.len(), 1);
        assert!(receipts[0].header("X-Account-Group-ID").is_none());
    }
    #[test]
    fn core_upgrade_error_is_recognized_before_unrelated_success_data_decoding() {
        #[derive(Debug, serde::Deserialize)]
        struct ExpectedSuccess { _expected_integer: u64 }
        let captured = CapturedRequests::serve_sequence(vec![("426 Upgrade Required", r#"{"request_id":"upgrade-malformed-data","data":{"_expected_integer":"wrong-type"},"error":{"code":"client_upgrade_required","message":"untrusted","details":{"minimum_version":"1.2.3"}},"meta":null}"#)]);
        let client = client_for(captured.base_url.clone(), Duration::from_secs(2));
        let result = client.public_json::<ExpectedSuccess>(Method::GET, "/v1/agreements", None);
        assert!(result.unwrap_err().is_client_update_required());
        assert!(client.upgrade_latch().is_tripped());
        assert_eq!(captured.finish().len(), 1);
    }

    #[test]
    fn core_upgrade_transport_trips_all_clones_and_never_sends_a_later_request() {
        let captured = CapturedRequests::serve_sequence(vec![("426 Upgrade Required", r#"{"request_id":"upgrade-fixture","data":null,"error":{"code":"client_upgrade_required","message":"untrusted-server-copy","details":{"minimum_version":"1.2.3"}},"meta":null}"#)]);
        let client = client_for(captured.base_url.clone(), Duration::from_secs(2));
        let sibling = client.clone();
        let first = client.public_json::<Value>(Method::GET, "/v1/agreements", None).unwrap_err();
        assert!(first.is_client_update_required());
        assert!(sibling.upgrade_latch().is_tripped());
        let second = sibling.public_json::<Value>(Method::GET, "/must-not-send", None).unwrap_err();
        assert!(second.is_client_update_required());
        assert!(!second.user_message().contains("untrusted-server-copy"));
        assert_eq!(captured.finish().len(), 1);
    }

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        target: String,
        path: String,
        headers: HashMap<String, String>,
        body: Option<Value>,
    }

    impl CapturedRequest {
        fn parse(raw: &[u8]) -> Self {
            let header_end = raw
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap_or(raw.len());
            let request = String::from_utf8_lossy(&raw[..header_end]);
            let mut lines = request.lines();
            let mut request_line = lines.next().unwrap_or_default().split_whitespace();
            let method = request_line.next().unwrap_or_default().to_string();
            let target = request_line.next().unwrap_or_default().to_string();
            let headers = lines
                .take_while(|line| !line.is_empty())
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
                .collect();
            let body_start = header_end.saturating_add(4).min(raw.len());
            let body = if body_start == raw.len() {
                None
            } else {
                Some(serde_json::from_slice(&raw[body_start..]).unwrap())
            };
            Self {
                method,
                path: target.clone(),
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

        fn json_body(&self) -> Option<&Value> {
            self.body.as_ref()
        }
    }

    fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        let mut expected_len = None;
        loop {
            let received = stream.read(&mut chunk).unwrap();
            if received == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..received]);
            if expected_len.is_none() {
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_len = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    expected_len = Some(header_end + 4 + content_len);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        request
    }

    struct CapturedRequests {
        base_url: String,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        worker: Option<JoinHandle<()>>,
    }

    impl CapturedRequests {
        fn serve_json(count: usize, body: &'static str) -> Self {
            Self::serve_sequence(vec![("200 OK", body); count])
        }

        fn serve_sequence(responses: Vec<(&'static str, &'static str)>) -> Self {
            Self::serve_owned_sequence(
                responses
                    .into_iter()
                    .map(|(status, body)| (status.to_string(), body.to_string()))
                    .collect(),
            )
        }

        fn serve_json_values(values: Vec<Value>) -> Self {
            Self::serve_json_values_with_page_meta_at(values, None)
        }

        fn serve_json_values_with_page_meta_at(
            values: Vec<Value>,
            page_response_index: Option<usize>,
        ) -> Self {
            Self::serve_owned_sequence(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(index, data)| {
                        (
                            "200 OK".to_string(),
                            serde_json::json!({
                                "request_id": "task4-matrix",
                                "data": data,
                                "error": null,
                                "meta": if page_response_index == Some(index) {
                                    serde_json::json!({"next_cursor": null})
                                } else {
                                    Value::Null
                                }
                            })
                            .to_string(),
                        )
                    })
                    .collect(),
            )
        }

        fn serve_owned_sequence(responses: Vec<(String, String)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::with_capacity(responses.len())));
            let captured = requests.clone();
            let worker = thread::spawn(move || {
                for (status, body) in responses {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_http_request(&mut stream);
                    captured
                        .lock()
                        .unwrap()
                        .push(CapturedRequest::parse(&request));
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

    fn client_for(base_url: String, timeout: Duration) -> ApiClient {
        ApiClient::new(
            ApiClientConfig {
                base_url: Url::parse(&base_url).unwrap(),
                app_version: "1.2.3".to_string(),
                timeout,
            },
            DeviceIdentity {
                id: Uuid::new_v4().to_string(),
                name: "test-device".to_string(),
                platform: "windows".to_string(),
            },
            Arc::new(SessionManager::new(Arc::new(
                MemoryRefreshTokenStore::default(),
            ))),
        )
        .unwrap()
    }

    fn one_response(status: &str, body: &'static str, delay: Duration) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            let _ = stream.write_all(response.as_bytes());
        });
        format!("http://{address}/")
    }

    fn sequential_responses(responses: Vec<(&'static str, &'static str)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{address}/")
    }

    fn tokens(access: &str, refresh: &str) -> TokenSet {
        TokenSet {
            access_token: access.to_string(),
            access_expires_in_seconds: 1800,
            refresh_token: refresh.to_string(),
            refresh_expires_at: "2099-01-01T00:00:00Z".to_string(),
            token_type: "X-Token".to_string(),
        }
    }

    fn authenticated_test_client(base_url: String) -> ApiClient {
        let client = client_for(base_url, Duration::from_secs(1));
        client
            .session()
            .install_tokens_for_user(&tokens("access-old", "refresh-old"), TEST_USER_ID)
            .unwrap();
        client
    }

    fn success_envelope() -> &'static str {
        r#"{"request_id":"success","data":{"ok":true},"error":null,"meta":null}"#
    }

    fn rejected_access_envelope() -> &'static str {
        r#"{"request_id":"rejected","data":null,"error":{"code":"access_token_invalid","message":"expired","details":null},"meta":null}"#
    }

    fn refresh_success_envelope() -> &'static str {
        r#"{"request_id":"refresh","data":{"access_token":"access-new","access_expires_in_seconds":1800,"refresh_token":"refresh-new","refresh_expires_at":"2099-01-01T00:00:00Z","token_type":"X-Token"},"error":null,"meta":null}"#
    }

    #[derive(Debug)]
    struct ExpectedRequest {
        method: &'static str,
        target: String,
        account_group_id: Option<&'static str>,
        idempotency_key: Option<&'static str>,
        body: Option<Value>,
    }

    fn expected(
        method: &'static str,
        target: &str,
        account_group_id: Option<&'static str>,
        idempotency_key: Option<&'static str>,
        body: Option<Value>,
    ) -> ExpectedRequest {
        ExpectedRequest {
            method,
            target: target.to_string(),
            account_group_id,
            idempotency_key,
            body,
        }
    }

    fn image_generation_request() -> CreateGenerationTask {
        CreateGenerationTask {
            client_request_id: "generation-key".to_string(),
            task_type: "image_generation".to_string(),
            model_code: "image-model".to_string(),
            prompt: "draw a lighthouse".to_string(),
            quality: Some("2K".to_string()),
            count: Some(2),
            aspect_ratio: Some("16:9".to_string()),
            reference_file_ids: Some(vec!["reference-a".to_string()]),
            target_language: Some("zh".to_string()),
        }
    }

    fn image_generation_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "task_type": "image_generation",
            "model_code": "image-model",
            "prompt": "draw a lighthouse",
            "quality": "2K",
            "count": 2,
            "aspect_ratio": "16:9",
            "reference_file_ids": ["reference-a"],
            "target_language": "zh"
        })
    }

    fn prompt_creation_request() -> CreatePromptOptimization {
        CreatePromptOptimization {
            client_request_id: "prompt-key".to_string(),
            prompt: "make this cinematic".to_string(),
            run_mode: "automatic".to_string(),
            focus_mode: "balanced".to_string(),
            max_rounds: 3,
            target_score: 92,
        }
    }

    fn prompt_creation_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "prompt": "make this cinematic",
            "run_mode": "automatic",
            "focus_mode": "balanced",
            "max_rounds": 3,
            "target_score": 92
        })
    }

    fn video_quote_request() -> CreateVideoQuote {
        CreateVideoQuote {
            model_code: "video-model".to_string(),
            source_file_id: "source-video".to_string(),
            aspect_ratio: "16:9".to_string(),
            resolution: "720P".to_string(),
            duration_secs: 8,
        }
    }

    fn video_quote_body() -> Value {
        serde_json::json!({
            "model_code": "video-model",
            "source_file_id": "source-video",
            "aspect_ratio": "16:9",
            "resolution": "720P",
            "duration_secs": 8
        })
    }

    fn video_generation_request() -> CreateVideoGenerationTask {
        CreateVideoGenerationTask {
            client_request_id: "video-key".to_string(),
            task_type: "image_to_video".to_string(),
            model_code: "video-model".to_string(),
            prompt: "slow camera move".to_string(),
            source_file_id: "source-video".to_string(),
            aspect_ratio: "16:9".to_string(),
            resolution: "720P".to_string(),
            duration_secs: 8,
            quote_id: TEST_QUOTE_ID.to_string(),
        }
    }

    fn video_generation_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "task_type": "image_to_video",
            "model_code": "video-model",
            "prompt": "slow camera move",
            "source_file_id": "source-video",
            "aspect_ratio": "16:9",
            "resolution": "720P",
            "duration_secs": 8,
            "quote_id": TEST_QUOTE_ID
        })
    }

    fn upscale_request() -> CreateUpscaleGenerationTask {
        CreateUpscaleGenerationTask {
            client_request_id: "upscale-key".to_string(),
            task_type: "image_upscale".to_string(),
            model_code: "upscale-model".to_string(),
            prompt: "preserve detail".to_string(),
            quality: "4K".to_string(),
            reference_file_ids: vec!["upscale-source".to_string()],
            target_width: 4096,
            target_height: 2304,
        }
    }

    fn upscale_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "task_type": "image_upscale",
            "model_code": "upscale-model",
            "prompt": "preserve detail",
            "quality": "4K",
            "reference_file_ids": ["upscale-source"],
            "target_width": 4096,
            "target_height": 2304
        })
    }

    fn image_edit_request() -> CreateImageEditTask {
        CreateImageEditTask {
            client_request_id: "image-edit-key".to_string(),
            task_type: "image_edit".to_string(),
            model_code: "edit-model".to_string(),
            prompt: "replace the sky".to_string(),
            quality: "2K".to_string(),
            aspect_ratio: "16:9".to_string(),
            source_file_id: "edit-source".to_string(),
            mask_file_id: "edit-mask".to_string(),
        }
    }

    fn image_edit_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "task_type": "image_edit",
            "model_code": "edit-model",
            "prompt": "replace the sky",
            "quality": "2K",
            "aspect_ratio": "16:9",
            "source_file_id": "edit-source",
            "mask_file_id": "edit-mask"
        })
    }

    fn watermark_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "reference_file_id": "watermark-source"
        })
    }

    fn colorization_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "reference_file_id": "colorize-source"
        })
    }

    fn enhancement_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "reference_file_id": "enhance-source",
            "target_quality": "4K"
        })
    }

    fn cutout_body(client_request_id: &str) -> Value {
        serde_json::json!({
            "client_request_id": client_request_id,
            "reference_file_id": "cutout-source",
            "subject_type": "person"
        })
    }

    fn generation_detail_json() -> Value {
        serde_json::json!({
            "id": TEST_TASK_ID,
            "billing_account_group_id": TEST_GROUP_ID,
            "status": "queued",
            "progress_percent": 0,
            "success_count": 0,
            "failure_count": 0,
            "failure": null,
            "prompt": null,
            "result_prompt": null,
            "items": []
        })
    }

    fn prompt_detail_json() -> Value {
        serde_json::json!({
            "id": TEST_PROMPT_ID,
            "billing_account_group_id": TEST_GROUP_ID,
            "max_rounds": 3,
            "current_round": 0,
            "completed_rounds": 0,
            "target_score": 92,
            "baseline_score": null,
            "best_score": null,
            "best_round_no": null,
            "progress_percent": 0,
            "result_score": null,
            "result_round_no": null
        })
    }

    fn order_detail_json() -> Value {
        serde_json::json!({
            "id": TEST_ORDER_ID,
            "billing_account_group_id": TEST_GROUP_ID,
            "status": "pending",
            "fulfillment_status": "pending",
            "payable_amount_cents": "100",
            "payment": null
        })
    }

    fn capture_task4_route_matrix() -> Vec<CapturedRequest> {
        let generation = generation_detail_json();
        let prompt = prompt_detail_json();
        let order = order_detail_json();
        let capture = CapturedRequests::serve_json_values_with_page_meta_at(vec![
            generation.clone(),
            generation.clone(),
            prompt.clone(),
            prompt,
            order.clone(),
            serde_json::json!({"items": [order.clone()]}),
            serde_json::json!({
                "available": "1000", "reserved": "10", "lifetime_granted": "1200",
                "lifetime_spent": "190", "version": "8"
            }),
            serde_json::json!([]),
            serde_json::json!({
                "redemption_id": "redemption-1", "credits_granted": "500",
                "redeemed_at": "2026-09-05T00:00:00Z", "credit_expires_at": null,
                "account": {
                    "available": "1500", "reserved": "10", "lifetime_granted": "1700",
                    "lifetime_spent": "190", "version": "9"
                }
            }),
            serde_json::json!({
                "quote_id": TEST_QUOTE_ID, "credit_cost": "30",
                "expires_at": "2026-09-05T00:05:00Z", "aspect_ratio": "16:9",
                "resolution": "720P", "duration_secs": 8
            }),
            generation.clone(),
            generation.clone(),
            generation.clone(),
            generation.clone(),
            generation.clone(),
            generation.clone(),
            generation,
            serde_json::json!([]),
            order.clone(),
            order.clone(),
            serde_json::json!({
                "id": TEST_QUOTE_ID, "target_plan_code": "pro",
                "payable_amount_cents": "100", "credit_delta": "500",
                "expires_at": "2026-09-05T00:05:00Z"
            }),
            order,
        ], Some(5));
        let client = authenticated_test_client(capture.base_url());
        let session = client.session().scope_for_user(TEST_USER_ID).unwrap();
        let scope = BillingScope {
            request: GroupRequestScope {
                session: session.clone(),
                account_group_id: TEST_GROUP_ID.to_string(),
            },
            context_epoch: 4,
        };
        let generation_api = GenerationApi::new(client.clone());
        let prompt_api = PromptOptimizationApi::new(client.clone());
        let payment_api = PaymentApi::new(client.clone());
        let account_api = AccountApi::new(client.clone());
        let membership_api = MembershipApi::new(client);

        generation_api
            .create_task_billing(&image_generation_request(), &scope)
            .unwrap();
        generation_api.task_scoped(TEST_TASK_ID, &session).unwrap();
        prompt_api
            .create_billing(&prompt_creation_request(), &scope)
            .unwrap();
        prompt_api.retry_billing(TEST_PROMPT_ID, "prompt-retry-key", &scope).unwrap();
        payment_api.order_scoped(TEST_ORDER_ID, &session).unwrap();
        payment_api.orders_billing(None, &scope).unwrap();
        account_api.credit_account_billing(&scope).unwrap();
        account_api.ledger_page_billing(None, 50, &scope).unwrap();
        account_api
            .redeem_credit_code_billing("REDEEM-CODE", "redeem-key", &scope)
            .unwrap();
        generation_api
            .quote_video_billing(&video_quote_request(), &scope)
            .unwrap();
        generation_api
            .create_video_task_billing(&video_generation_request(), &scope)
            .unwrap();
        generation_api
            .create_upscale_task_billing(&upscale_request(), &scope)
            .unwrap();
        generation_api
            .create_image_edit_task_billing(&image_edit_request(), &scope)
            .unwrap();
        generation_api
            .create_watermark_removal_billing(
                &CreateWatermarkRemoval {
                    client_request_id: "watermark-key".to_string(),
                    reference_file_id: "watermark-source".to_string(),
                },
                &scope,
            )
            .unwrap();
        generation_api
            .create_image_colorization_billing(
                &CreateImageColorization {
                    client_request_id: "colorize-key".to_string(),
                    reference_file_id: "colorize-source".to_string(),
                },
                &scope,
            )
            .unwrap();
        generation_api
            .create_image_enhancement_billing(
                &CreateImageEnhancement {
                    client_request_id: "enhance-key".to_string(),
                    reference_file_id: "enhance-source".to_string(),
                    target_quality: "4K".to_string(),
                },
                &scope,
            )
            .unwrap();
        generation_api
            .create_image_cutout_billing(
                &CreateImageCutout {
                    client_request_id: "cutout-key".to_string(),
                    reference_file_id: "cutout-source".to_string(),
                    subject_type: "person".to_string(),
                },
                &scope,
            )
            .unwrap();
        payment_api.packs_billing(&scope).unwrap();
        payment_api
            .create_credit_order_billing("pack_1000", "credit-order-key", &scope)
            .unwrap();
        membership_api
            .create_order_billing("pro", "membership-order-key", &scope)
            .unwrap();
        membership_api
            .create_upgrade_quote_billing("pro", "upgrade-order-key", &scope)
            .unwrap();
        membership_api
            .create_upgrade_order_billing(TEST_QUOTE_ID, "upgrade-order-key", &scope)
            .unwrap();

        capture.finish()
    }

    #[test]
    fn upgrade_quote_identity_billing_api_sends_durable_key_and_exact_body() {
        let capture = CapturedRequests::serve_json_values(vec![serde_json::json!({
            "id": TEST_QUOTE_ID,
            "target_plan_code": "pro",
            "payable_amount_cents": "100",
            "credit_delta": "500",
            "expires_at": "2026-09-05T00:05:00Z"
        })]);
        let client = authenticated_test_client(capture.base_url());
        let session = client.session().scope_for_user(TEST_USER_ID).unwrap();
        let scope = BillingScope {
            request: GroupRequestScope {
                session,
                account_group_id: TEST_GROUP_ID.to_string(),
            },
            context_epoch: 4,
        };

        MembershipApi::new(client)
            .create_upgrade_quote_billing("pro", "upgrade-quote-key", &scope)
            .unwrap();

        let requests = capture.finish();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].target, "/v1/membership/upgrade-quotes");
        assert_eq!(
            requests[0].header("x-account-group-id"),
            Some(TEST_GROUP_ID)
        );
        assert_eq!(
            requests[0].header("idempotency-key"),
            Some("upgrade-quote-key")
        );
        assert_eq!(
            requests[0].json_body(),
            Some(&serde_json::json!({"target_plan_code": "pro"}))
        );
    }

    #[test]
    fn route_methods_require_the_intended_scope_type() {
        let _: fn(
            &GenerationApi,
            &CreateGenerationTask,
            &BillingScope,
        ) -> Result<GenerationTaskDetail, ApiError> = GenerationApi::create_task_billing;
        let _: fn(&GenerationApi, &str, &SessionScope) -> Result<GenerationTaskDetail, ApiError> =
            GenerationApi::task_scoped;
        let _: fn(
            &PromptOptimizationApi,
            &CreatePromptOptimization,
            &BillingScope,
        ) -> Result<PromptOptimizationDetail, ApiError> = PromptOptimizationApi::create_billing;
        let _: fn(
            &PromptOptimizationApi,
            &str,
            &str,
            &BillingScope,
        ) -> Result<PromptOptimizationDetail, ApiError> = PromptOptimizationApi::retry_billing;
        let _: fn(&PaymentApi, &str, &SessionScope) -> Result<OrderDetail, ApiError> =
            PaymentApi::order_scoped;
        let _: fn(
            &PaymentApi,
            Option<&str>,
            &BillingScope,
        ) -> Result<TeamPage<OrderDetail>, ApiError> = PaymentApi::orders_billing;
        let _: fn(&AccountApi, &BillingScope) -> Result<CreditAccount, ApiError> =
            AccountApi::credit_account_billing;
        let _: fn(
            &AccountApi,
            Option<&str>,
            usize,
            &BillingScope,
        ) -> Result<CreditLedgerPage, ApiError> = AccountApi::ledger_page_billing;
        let _: fn(
            &AccountApi,
            &str,
            &str,
            &BillingScope,
        ) -> Result<CreditRedemptionResult, ApiError> = AccountApi::redeem_credit_code_billing;
        let _: fn(
            &GenerationApi,
            &CreateVideoQuote,
            &BillingScope,
        ) -> Result<VideoQuote, ApiError> = GenerationApi::quote_video_billing;
        let _: fn(
            &GenerationApi,
            &CreateVideoGenerationTask,
            &BillingScope,
        ) -> Result<GenerationTaskDetail, ApiError> = GenerationApi::create_video_task_billing;
        let _: fn(
            &GenerationApi,
            &CreateUpscaleGenerationTask,
            &BillingScope,
        ) -> Result<GenerationTaskDetail, ApiError> = GenerationApi::create_upscale_task_billing;
        let _: fn(
            &GenerationApi,
            &CreateImageEditTask,
            &BillingScope,
        ) -> Result<GenerationTaskDetail, ApiError> = GenerationApi::create_image_edit_task_billing;
        let _: fn(
            &GenerationApi,
            &CreateWatermarkRemoval,
            &BillingScope,
        ) -> Result<GenerationTaskDetail, ApiError> =
            GenerationApi::create_watermark_removal_billing;
        let _: fn(
            &GenerationApi,
            &CreateImageColorization,
            &BillingScope,
        ) -> Result<GenerationTaskDetail, ApiError> =
            GenerationApi::create_image_colorization_billing;
        let _: fn(
            &GenerationApi,
            &CreateImageEnhancement,
            &BillingScope,
        ) -> Result<GenerationTaskDetail, ApiError> =
            GenerationApi::create_image_enhancement_billing;
        let _: fn(
            &GenerationApi,
            &CreateImageCutout,
            &BillingScope,
        ) -> Result<GenerationTaskDetail, ApiError> = GenerationApi::create_image_cutout_billing;
        let _: fn(&PaymentApi, &BillingScope) -> Result<Vec<CreditPack>, ApiError> =
            PaymentApi::packs_billing;
        let _: fn(&PaymentApi, &str, &str, &BillingScope) -> Result<OrderDetail, ApiError> =
            PaymentApi::create_credit_order_billing;
        let _: fn(&MembershipApi, &str, &str, &BillingScope) -> Result<OrderDetail, ApiError> =
            MembershipApi::create_order_billing;
        let _: fn(&MembershipApi, &str, &str, &BillingScope) -> Result<UpgradeQuote, ApiError> =
            MembershipApi::create_upgrade_quote_billing;
        let _: fn(&MembershipApi, &str, &str, &BillingScope) -> Result<OrderDetail, ApiError> =
            MembershipApi::create_upgrade_order_billing;
    }

    #[test]
    fn every_task4_route_sends_exact_authority_and_idempotency_policy() {
        let requests = capture_task4_route_matrix();
        let expected = [
            expected(
                "POST",
                "/v1/generation/tasks",
                Some(TEST_GROUP_ID),
                Some("generation-key"),
                Some(image_generation_body("generation-key")),
            ),
            expected(
                "GET",
                &format!("/v1/generation/tasks/{TEST_TASK_ID}"),
                None,
                None,
                None,
            ),
            expected(
                "POST",
                "/v1/prompt-optimizations",
                Some(TEST_GROUP_ID),
                Some("prompt-key"),
                Some(prompt_creation_body("prompt-key")),
            ),
            expected(
                "POST",
                &format!("/v1/prompt-optimizations/{TEST_PROMPT_ID}/retry"),
                Some(TEST_GROUP_ID),
                Some("prompt-retry-key"),
                Some(serde_json::json!({"client_request_id":"prompt-retry-key"})),
            ),
            expected(
                "GET",
                &format!("/v1/orders/{TEST_ORDER_ID}"),
                None,
                None,
                None,
            ),
            expected(
                "GET",
                "/v1/orders?page_size=50",
                Some(TEST_GROUP_ID),
                None,
                None,
            ),
            expected(
                "GET",
                "/v1/credits/account",
                Some(TEST_GROUP_ID),
                None,
                None,
            ),
            expected(
                "GET",
                "/v1/credits/ledger?limit=50",
                Some(TEST_GROUP_ID),
                None,
                None,
            ),
            expected(
                "POST",
                "/v1/credits/redemptions",
                Some(TEST_GROUP_ID),
                Some("redeem-key"),
                Some(serde_json::json!({
                    "code": "REDEEM-CODE", "client_request_id": "redeem-key"
                })),
            ),
            expected(
                "POST",
                "/v1/generation/video-quotes",
                Some(TEST_GROUP_ID),
                None,
                Some(video_quote_body()),
            ),
            expected(
                "POST",
                "/v1/generation/tasks",
                Some(TEST_GROUP_ID),
                Some("video-key"),
                Some(video_generation_body("video-key")),
            ),
            expected(
                "POST",
                "/v1/generation/tasks",
                Some(TEST_GROUP_ID),
                Some("upscale-key"),
                Some(upscale_body("upscale-key")),
            ),
            expected(
                "POST",
                "/v1/generation/tasks",
                Some(TEST_GROUP_ID),
                Some("image-edit-key"),
                Some(image_edit_body("image-edit-key")),
            ),
            expected(
                "POST",
                "/v1/toolbox/watermark-removals",
                Some(TEST_GROUP_ID),
                Some("watermark-key"),
                Some(watermark_body("watermark-key")),
            ),
            expected(
                "POST",
                "/v1/toolbox/image-colorizations",
                Some(TEST_GROUP_ID),
                Some("colorize-key"),
                Some(colorization_body("colorize-key")),
            ),
            expected(
                "POST",
                "/v1/toolbox/image-enhancements",
                Some(TEST_GROUP_ID),
                Some("enhance-key"),
                Some(enhancement_body("enhance-key")),
            ),
            expected(
                "POST",
                "/v1/toolbox/image-cutouts",
                Some(TEST_GROUP_ID),
                Some("cutout-key"),
                Some(cutout_body("cutout-key")),
            ),
            expected("GET", "/v1/credits/packs", Some(TEST_GROUP_ID), None, None),
            expected(
                "POST",
                "/v1/credits/orders",
                Some(TEST_GROUP_ID),
                Some("credit-order-key"),
                Some(serde_json::json!({
                    "pack_code": "pack_1000", "client_request_id": "credit-order-key"
                })),
            ),
            expected(
                "POST",
                "/v1/membership/orders",
                Some(TEST_GROUP_ID),
                Some("membership-order-key"),
                Some(serde_json::json!({
                    "plan_code": "pro", "client_request_id": "membership-order-key"
                })),
            ),
            expected(
                "POST",
                "/v1/membership/upgrade-quotes",
                Some(TEST_GROUP_ID),
                Some("upgrade-order-key"),
                Some(serde_json::json!({"target_plan_code": "pro"})),
            ),
            expected(
                "POST",
                "/v1/membership/upgrade-orders",
                Some(TEST_GROUP_ID),
                Some("upgrade-order-key"),
                Some(serde_json::json!({
                    "quote_id": TEST_QUOTE_ID, "client_request_id": "upgrade-order-key"
                })),
            ),
        ];

        assert_eq!(requests.len(), expected.len());
        for (request, expected) in requests.iter().zip(expected.iter()) {
            assert_eq!(request.method, expected.method);
            assert_eq!(request.target, expected.target);
            assert_eq!(
                request.header("x-account-group-id"),
                expected.account_group_id
            );
            assert_eq!(request.header("idempotency-key"), expected.idempotency_key);
            assert_eq!(request.json_body(), expected.body.as_ref());
        }
    }

    #[test]
    fn selected_wallet_reads_and_redemption_require_the_billing_scope() {
        let capture = CapturedRequests::serve_json_values(vec![
            serde_json::json!({
                "available": "1000", "reserved": "10", "lifetime_granted": "1200",
                "lifetime_spent": "190", "version": "8"
            }),
            serde_json::json!([]),
            serde_json::json!({
                "redemption_id": "redemption-1", "credits_granted": "500",
                "redeemed_at": "2026-09-05T00:00:00Z", "credit_expires_at": null,
                "account": {
                    "available": "1500", "reserved": "10", "lifetime_granted": "1700",
                    "lifetime_spent": "190", "version": "9"
                }
            }),
        ]);
        let client = authenticated_test_client(capture.base_url());
        let session = client.session().scope_for_user(TEST_USER_ID).unwrap();
        let scope = BillingScope {
            request: GroupRequestScope {
                session,
                account_group_id: TEST_GROUP_ID.to_string(),
            },
            context_epoch: 4,
        };
        let api = AccountApi::new(client);
        api.credit_account_billing(&scope).unwrap();
        api.ledger_page_billing(Some("next/+ page"), 50, &scope)
            .unwrap();
        api.redeem_credit_code_billing("REDEEM-CODE", "redeem-key", &scope)
            .unwrap();
        let requests = capture.finish();
        assert_eq!(
            requests[1].target,
            "/v1/credits/ledger?limit=50&cursor=next%2F%2B+page"
        );
        assert!(requests
            .iter()
            .all(|request| { request.header("x-account-group-id") == Some(TEST_GROUP_ID) }));
    }

    #[test]
    fn identity_omits_and_billing_includes_group_header() {
        let capture = CapturedRequests::serve_json(2, success_envelope());
        let client = authenticated_test_client(capture.base_url());
        let session = client.session().scope_for_user(TEST_USER_ID).unwrap();
        let billing = BillingScope {
            request: GroupRequestScope {
                session: session.clone(),
                account_group_id: TEST_GROUP_ID.to_string(),
            },
            context_epoch: 1,
        };
        let auth_epoch_before = client.session().auth_epoch();

        client
            .identity_json_scoped::<Value>(
                Method::GET,
                "/v1/account-groups",
                None,
                None,
                &session,
            )
            .unwrap();
        client
            .billing_json_scoped::<Value>(
                Method::GET,
                "/v1/account",
                None,
                None,
                &billing,
            )
            .unwrap();

        let requests = capture.finish();
        assert_eq!(requests[0].header("x-account-group-id"), None);
        assert_eq!(
            requests[1].header("x-account-group-id"),
            Some(TEST_GROUP_ID)
        );
        assert_eq!(client.session().auth_epoch(), auth_epoch_before);
    }

    #[test]
    fn group_header_without_access_token_is_rejected_before_network() {
        let client = client_for(
            "http://127.0.0.1:9/".to_string(),
            Duration::from_millis(100),
        );

        let error = client
            .send_once::<Value>(
                Method::GET,
                "/must-not-send",
                None,
                Some("independent-key"),
                None,
                Some(TEST_GROUP_ID),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            ApiError::Protocol {
                request_id: None,
                ..
            }
        ));
    }

    #[test]
    fn refreshed_business_retry_keeps_captured_billing_group() {
        let capture = CapturedRequests::serve_sequence(vec![
            ("401 Unauthorized", rejected_access_envelope()),
            ("200 OK", refresh_success_envelope()),
            ("200 OK", success_envelope()),
        ]);
        let client = authenticated_test_client(capture.base_url());
        let session = client.session().scope_for_user(TEST_USER_ID).unwrap();
        let scope = BillingScope {
            request: GroupRequestScope {
                session,
                account_group_id: TEST_GROUP_ID.to_string(),
            },
            context_epoch: 3,
        };
        let auth_epoch_before = client.session().auth_epoch();

        client
            .billing_json_scoped::<Value>(
                Method::POST,
                "/v1/generation/tasks",
                Some(serde_json::json!({"probe": true})),
                Some("header-retry-test"),
                &scope,
            )
            .unwrap();

        let requests = capture.finish();
        assert_eq!(
            requests[0].header("x-account-group-id"),
            Some(TEST_GROUP_ID)
        );
        assert_eq!(requests[1].path, "/v1/auth/refresh");
        assert_eq!(requests[1].header("x-account-group-id"), None);
        assert_eq!(
            requests[2].header("x-account-group-id"),
            Some(TEST_GROUP_ID)
        );
        assert_eq!(client.session().auth_epoch(), auth_epoch_before);
    }

    #[test]
    fn request_timeout_is_a_network_timeout() {
        let url = one_response(
            "200 OK",
            r#"{"request_id":"slow","data":{},"error":null,"meta":null}"#,
            Duration::from_millis(150),
        );
        let error = client_for(url, Duration::from_millis(30))
            .public_json::<Value>(Method::GET, "/slow", None)
            .unwrap_err();
        assert!(matches!(error, ApiError::Network { timeout: true, .. }));
    }

    #[test]
    fn unauthorized_envelope_preserves_status_code_and_request_id() {
        let url = one_response(
            "401 Unauthorized",
            r#"{"request_id":"req-401","data":null,"error":{"code":"access_token_invalid","message":"invalid","details":null},"meta":null}"#,
            Duration::ZERO,
        );
        let error = client_for(url, Duration::from_secs(1))
            .public_json::<Value>(Method::GET, "/unauthorized", None)
            .unwrap_err();
        assert!(
            matches!(error, ApiError::Http { status: 401, ref code, ref request_id, .. }
            if code == "access_token_invalid" && request_id.as_deref() == Some("req-401"))
        );
    }

    #[test]
    fn server_error_code_is_not_collapsed_into_a_network_error() {
        let url = one_response(
            "503 Service Unavailable",
            r#"{"request_id":"req-503","data":null,"error":{"code":"service_unavailable","message":"later","details":{"retry":true}},"meta":null}"#,
            Duration::ZERO,
        );
        let error = client_for(url, Duration::from_secs(1))
            .public_json::<Value>(Method::GET, "/unavailable", None)
            .unwrap_err();
        assert!(matches!(error, ApiError::Http { status: 503, ref code, .. }
            if code == "service_unavailable"));
    }

    #[test]
    fn invalid_json_is_reported_as_protocol_error() {
        let url = one_response("200 OK", "not-json", Duration::ZERO);
        let error = client_for(url, Duration::from_secs(1))
            .public_json::<Value>(Method::GET, "/broken", None)
            .unwrap_err();
        assert!(matches!(error, ApiError::Protocol { .. }));
    }

    #[test]
    fn rejected_second_response_after_refresh_clears_the_captured_lease() {
        let url = sequential_responses(vec![
            (
                "401 Unauthorized",
                r#"{"request_id":"first","data":null,"error":{"code":"access_token_invalid","message":"expired","details":null},"meta":null}"#,
            ),
            (
                "200 OK",
                r#"{"request_id":"refresh","data":{"access_token":"access-new","access_expires_in_seconds":1800,"refresh_token":"refresh-new","refresh_expires_at":"2099-01-01T00:00:00Z","token_type":"X-Token"},"error":null,"meta":null}"#,
            ),
            (
                "401 Unauthorized",
                r#"{"request_id":"second","data":null,"error":{"code":"access_token_invalid","message":"still invalid","details":null},"meta":null}"#,
            ),
        ]);
        let client = client_for(url, Duration::from_secs(1));
        client
            .session()
            .install_tokens_for_user(&tokens("access-old", "refresh-old"), "user-a")
            .unwrap();

        let error = client
            .authenticated_json::<Value>(Method::GET, "/resource", None, None)
            .unwrap_err();

        assert!(error.is_access_token_rejected());
        assert!(client.session().access().is_none());
        assert!(!client.session().has_refresh_token().unwrap());
    }

    #[test]
    fn authentication_required_after_refresh_is_an_exhausted_auth_error() {
        let client = client_for("http://127.0.0.1:1/".to_string(), Duration::from_secs(1));
        let scope = client
            .session()
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        let rejected = ApiError::Http {
            status: 401,
            code: "authentication_required".to_string(),
            message: "still rejected".to_string(),
            request_id: None,
            details: None,
        };

        let returned = client.clear_scope_on_exhausted_auth_error(&scope, rejected);

        assert!(returned.is_access_token_rejected());
        assert!(client.session().access().is_none());
        assert!(!client.session().has_refresh_token().unwrap());
    }

    #[test]
    fn stale_exhausted_auth_error_from_account_a_cannot_clear_account_b() {
        let client = client_for("http://127.0.0.1:1/".to_string(), Duration::from_secs(1));
        let scope_a = client
            .session()
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        let scope_b = client
            .session()
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();
        let rejected = ApiError::Http {
            status: 401,
            code: "access_token_invalid".to_string(),
            message: "stale rejection".to_string(),
            request_id: None,
            details: None,
        };

        let returned = client.clear_scope_on_exhausted_auth_error(&scope_a, rejected);

        assert!(returned.is_access_token_rejected());
        assert_eq!(
            client.session().access_token_for_scope(&scope_b).unwrap(),
            "access-b"
        );
        assert_eq!(
            client.session().has_refresh_token().unwrap(),
            true,
            "the current account's refresh token must survive a stale rejection"
        );
    }

    #[test]
    fn authenticated_call_retries_refresh_after_a_transient_refresh_failure() {
        let url = sequential_responses(vec![
            (
                "200 OK",
                r#"{"request_id":"refresh","data":{"access_token":"access-new","access_expires_in_seconds":1800,"refresh_token":"refresh-new","refresh_expires_at":"2099-01-01T00:00:00Z","token_type":"X-Token"},"error":null,"meta":null}"#,
            ),
            (
                "200 OK",
                r#"{"request_id":"resource","data":{"ok":true},"error":null,"meta":null}"#,
            ),
        ]);
        let client = client_for(url, Duration::from_secs(1));
        let scope = client
            .session()
            .install_tokens_for_user(&tokens("access-old", "refresh-old"), "user-a")
            .unwrap();

        let first_refresh = client.session().refresh_scope(
            &scope,
            Some("access-old"),
            |_| {
                Err(ApiError::Network {
                    message: "temporarily offline".to_string(),
                    timeout: false,
                })
            },
        );
        assert!(matches!(first_refresh, Err(ApiError::Network { .. })));
        assert!(client.session().is_scope_current(&scope));
        assert!(client.session().access().is_none());
        assert!(client.session().has_refresh_token().unwrap());

        let response = client
            .authenticated_json_scoped::<Value>(Method::GET, "/resource", None, None, &scope)
            .unwrap();

        assert_eq!(response.data.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            client.session().access_token_for_scope(&scope).unwrap(),
            "access-new"
        );
    }
}
