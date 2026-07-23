use super::{ApiClient, ApiError, ApiResponse, TokenSet};
use reqwest::Method;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AgreementItem {
    #[serde(rename = "type")]
    pub(crate) agreement_type: String,
    pub(crate) version: String,
    pub(crate) title: String,
    pub(crate) content_url: String,
    pub(crate) content_sha256: String,
    pub(crate) required: bool,
    pub(crate) required_action: String,
    pub(crate) effective_at: String,
}

#[derive(Clone, Debug, Deserialize)]
struct AgreementList {
    items: Vec<AgreementItem>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AgreementAcceptance {
    #[serde(rename = "type")]
    pub(crate) agreement_type: String,
    pub(crate) version: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct EmailCodeResponse {
    pub(crate) email_masked: String,
    pub(crate) expires_in_seconds: u64,
    pub(crate) resend_after_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LoginUser {
    pub(crate) id: String,
    pub(crate) email_masked: String,
    pub(crate) nickname: Option<String>,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LoginResponse {
    #[serde(flatten)]
    pub(crate) tokens: TokenSet,
    pub(crate) is_new_user: bool,
    pub(crate) registration_credit_granted: String,
    pub(crate) user: LoginUser,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LogoutResponse {
    #[serde(default)]
    pub(crate) logged_out: bool,
    #[serde(default)]
    pub(crate) logged_out_all: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WechatLoginStartResponse {
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
pub(crate) struct WechatLoginStatusResponse {
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) qr_status: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) login: Option<LoginResponse>,
}

#[derive(Serialize)]
struct EmailCodeRequest<'a> {
    email: &'a str,
    app_version: &'a str,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    email: &'a str,
    code: &'a str,
    device_id: &'a str,
    device_name: &'a str,
    platform: &'a str,
    app_version: &'a str,
    agreement_acceptances: &'a [AgreementAcceptance],
}

#[derive(Serialize)]
struct WechatLoginStartRequest<'a> {
    device_id: &'a str,
    device_name: &'a str,
    platform: &'a str,
    app_version: &'a str,
    agreement_acceptances: &'a [AgreementAcceptance],
}

#[derive(Serialize)]
struct WechatLoginStatusRequest<'a> {
    login_id: &'a str,
    agreement_acceptances: &'a [AgreementAcceptance],
}

#[derive(Serialize)]
struct AcceptAgreementsRequest<'a> {
    agreements: &'a [AgreementAcceptance],
}

#[derive(Clone)]
pub(crate) struct AuthApi {
    client: ApiClient,
}

impl AuthApi {
    pub(crate) fn new(client: ApiClient) -> Self {
        Self { client }
    }

    pub(crate) fn list_agreements(&self) -> Result<Vec<AgreementItem>, ApiError> {
        self.client
            .public_json(Method::GET, "/v1/agreements", None)
            .map(|response: ApiResponse<AgreementList>| response.data.items)
    }

    pub(crate) fn request_email_code(
        &self,
        email: &str,
    ) -> Result<EmailCodeResponse, ApiError> {
        let body = serde_json::to_value(EmailCodeRequest {
            email,
            app_version: self.client.app_version(),
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .public_json(Method::POST, "/v1/auth/email/code", Some(body))
            .map(|response: ApiResponse<EmailCodeResponse>| response.data)
    }

    pub(crate) fn login(
        &self,
        email: &str,
        code: &str,
        acceptances: &[AgreementAcceptance],
    ) -> Result<LoginResponse, ApiError> {
        let device = self.client.device();
        let body = serde_json::to_value(LoginRequest {
            email,
            code,
            device_id: &device.id,
            device_name: &device.name,
            platform: &device.platform,
            app_version: self.client.app_version(),
            agreement_acceptances: acceptances,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        let response = self
            .client
            .public_json(Method::POST, "/v1/auth/email/login", Some(body))?;
        let response: ApiResponse<LoginResponse> = response;
        self.client.session().install_tokens(&response.data.tokens)?;
        Ok(response.data)
    }

    pub(crate) fn start_wechat_login(
        &self,
        acceptances: &[AgreementAcceptance],
    ) -> Result<WechatLoginStartResponse, ApiError> {
        let device = self.client.device();
        let body = serde_json::to_value(WechatLoginStartRequest {
            device_id: &device.id,
            device_name: &device.name,
            platform: &device.platform,
            app_version: self.client.app_version(),
            agreement_acceptances: acceptances,
        })
        .map_err(|error| ApiError::Protocol {
            message: error.to_string(),
            request_id: None,
        })?;
        self.client
            .public_json(Method::POST, "/v1/auth/wechat/session", Some(body))
            .map(|response: ApiResponse<WechatLoginStartResponse>| response.data)
    }

    pub(crate) fn wechat_login_status(
        &self,
        login_id: &str,
        acceptances: &[AgreementAcceptance],
    ) -> Result<WechatLoginStatusResponse, ApiError> {
        let body = serde_json::to_value(WechatLoginStatusRequest {
            login_id,
            agreement_acceptances: acceptances,
        })
            .map_err(|error| ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            })?;
        let response: ApiResponse<WechatLoginStatusResponse> = self.client.public_json(
            Method::POST,
            "/v1/auth/wechat/session/status",
            Some(body),
        )?;
        if let Some(login) = response.data.login.as_ref() {
            self.client.session().install_tokens(&login.tokens)?;
        }
        Ok(response.data)
    }

    pub(crate) fn refresh(&self) -> Result<String, ApiError> {
        self.client.refresh_session()
    }

    pub(crate) fn accept_agreements(
        &self,
        agreements: &[AgreementAcceptance],
    ) -> Result<(), ApiError> {
        if agreements.is_empty() {
            return Ok(());
        }
        let body = serde_json::to_value(AcceptAgreementsRequest { agreements })
            .map_err(|error| ApiError::Protocol {
                message: error.to_string(),
                request_id: None,
            })?;
        self.client.authenticated_json::<serde_json::Value>(
            Method::POST,
            "/v1/agreements/accept",
            Some(body),
            None,
        )?;
        Ok(())
    }

    pub(crate) fn logout(&self, all_devices: bool) -> Result<(), ApiError> {
        let path = if all_devices {
            "/v1/auth/logout_all"
        } else {
            "/v1/auth/logout"
        };
        let response: Result<ApiResponse<LogoutResponse>, ApiError> =
            self.client.authenticated_json(Method::POST, path, None, None);
        match response {
            Ok(value) => {
                let _ = value.data.logged_out || value.data.logged_out_all;
                self.client.session().clear()
            }
            Err(error) if error.is_terminal_session_error() => {
                let _ = self.client.session().clear();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}
