use super::{
    ApiEnvelope, ApiError, ApiResponse, DeviceIdentity, RefreshRequest, SessionManager,
    SessionScope, TokenSet,
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

    pub(crate) fn public_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.send_once(method, path, body, None)
    }

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
            Some((&access_token, idempotency_key)),
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
                    Some((&refreshed, idempotency_key)),
                )
                .map_err(|error| self.clear_epoch_on_exhausted_auth_error(auth_epoch, error))
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn authenticated_json_scoped<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        scope: &SessionScope,
    ) -> Result<ApiResponse<T>, ApiError> {
        // Scope validation and token cloning happen under the same session lock. If another
        // account is installed immediately afterwards, this request still carries the old
        // account's cloned token and can never borrow the new account's credentials.
        let access_token = self.access_or_refresh_scope(scope)?;
        let first = self.send_once_scope(
            method.clone(),
            path,
            body.clone(),
            Some((&access_token, idempotency_key)),
            scope,
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
                    Some((&refreshed, idempotency_key)),
                )
                .map_err(|error| self.clear_scope_on_exhausted_auth_error(scope, error))
            }
            Err(error) => Err(error),
        }
    }

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
            Some((&access_token, idempotency_key)),
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
                    Some((&refreshed, idempotency_key)),
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
        self.send_once(method, path, body, Some((access_token, None)))
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
        authentication: Option<(&str, Option<&str>)>,
        auth_epoch: u64,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.send_once(method, path, body, authentication)
            .map_err(|error| self.clear_epoch_on_terminal_error(auth_epoch, error))
    }

    fn send_once_scope<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        authentication: Option<(&str, Option<&str>)>,
        scope: &SessionScope,
    ) -> Result<ApiResponse<T>, ApiError> {
        self.send_once(method, path, body, authentication)
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
        authentication: Option<(&str, Option<&str>)>,
    ) -> Result<ApiResponse<T>, ApiError> {
        let url = self.endpoint(path)?;
        let request_id = Uuid::new_v4().to_string();
        let mut request = self
            .http
            .request(method, url)
            .header("X-Request-ID", &request_id);
        if let Some((access_token, idempotency_key)) = authentication {
            request = request
                .header("X-Token", access_token)
                .header("X-Client-Version", &self.config.app_version)
                .header("X-Device-ID", &self.device.id);
            if let Some(key) = idempotency_key {
                request = request.header("Idempotency-Key", key);
            }
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
        let response = request.send()?;
        let status = response.status();
        let response_request_id = response
            .headers()
            .get("X-Request-ID")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let payload = response.bytes()?;
        let envelope = serde_json::from_slice::<ApiEnvelope<T>>(&payload).map_err(|error| {
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
            return Err(ApiError::Http {
                status: status.as_u16(),
                code: problem.code,
                message: problem.message,
                request_id: Some(envelope.request_id),
                details: problem.details,
            });
        }
        let data = envelope.data.ok_or_else(|| ApiError::Protocol {
            message: "成功响应缺少 data 字段".to_string(),
            request_id: Some(envelope.request_id.clone()),
        })?;
        Ok(ApiResponse {
            request_id: envelope.request_id,
            data,
            meta: envelope.meta,
        })
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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
