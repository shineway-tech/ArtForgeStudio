use serde_json::Value;

#[derive(Clone, Debug, thiserror::Error)]
pub(crate) enum ApiError {
    #[error("网络请求失败：{message}")]
    Network { message: String, timeout: bool },
    #[error("接口返回错误 {code}：{message}")]
    Http {
        status: u16,
        code: String,
        message: String,
        request_id: Option<String>,
        details: Option<Value>,
    },
    #[error("接口响应格式错误：{message}")]
    Protocol {
        message: String,
        request_id: Option<String>,
    },
    #[error("当前设备尚未登录")]
    AuthenticationRequired,
    #[error("安全凭据操作失败：{message}")]
    Credential { message: String },
    #[error("客户端配置错误：{message}")]
    Configuration { message: String },
    #[error("本地状态操作失败：{message}")]
    LocalState { message: String },
}

impl ApiError {
    pub(crate) fn code(&self) -> Option<&str> {
        match self {
            Self::Http { code, .. } => Some(code),
            Self::AuthenticationRequired => Some("authentication_required"),
            _ => None,
        }
    }

    pub(crate) fn request_id(&self) -> Option<&str> {
        match self {
            Self::Http { request_id, .. } | Self::Protocol { request_id, .. } => {
                request_id.as_deref()
            }
            _ => None,
        }
    }

    pub(crate) fn is_access_token_rejected(&self) -> bool {
        matches!(
            self.code(),
            Some("access_token_invalid" | "authentication_required")
        )
    }

    pub(crate) fn is_terminal_session_error(&self) -> bool {
        matches!(
            self.code(),
            Some(
                "session_invalid"
                    | "session_device_mismatch"
                    | "refresh_token_invalid"
                    | "refresh_token_reused"
                    | "account_disabled"
                    | "account_unavailable"
            )
        )
    }

    pub(crate) fn is_client_update_required(&self) -> bool {
        self.code() == Some("client_update_required")
    }

    pub(crate) fn is_network_error(&self) -> bool {
        matches!(self, Self::Network { .. })
    }

    pub(crate) fn generation_message(&self) -> String {
        match self.code() {
            Some("insufficient_credits") => "积分不足，请充值后重试".to_string(),
            Some("membership_quality_forbidden") => {
                "当前会员不支持所选清晰度，请降低清晰度或升级会员".to_string()
            }
            Some("model_quality_unavailable") => {
                "当前模型暂不支持所选清晰度，请更换清晰度".to_string()
            }
            Some("model_unavailable" | "model_configuration_missing") => {
                "所选模型已下线或暂不可用，请刷新模型目录后重试".to_string()
            }
            Some("generation_queue_limit_reached") => {
                "当前排队任务过多，请等待已有任务完成后重试".to_string()
            }
            Some("reference_file_unavailable" | "result_file_expired") => {
                "任务文件已过期或不可用，请重新上传后生成".to_string()
            }
            Some("reference_files_too_large" | "reference_image_too_large") => {
                "参考图超过大小限制，请压缩后重试".to_string()
            }
            Some("client_request_conflict") => {
                "请求恢复信息与服务端记录冲突，请重新发起生成".to_string()
            }
            Some("delivery_checksum_mismatch") => {
                "生成文件完整性校验失败，请重新下载".to_string()
            }
            _ if self.is_client_update_required() => "当前客户端版本过旧，请更新后重试".to_string(),
            _ if self.is_terminal_session_error() => "登录状态已失效，请重新登录".to_string(),
            _ => self.to_string(),
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        Self::Network {
            timeout: error.is_timeout(),
            message: if error.is_timeout() {
                "请求超时".to_string()
            } else if error.is_connect() {
                "无法连接到服务端".to_string()
            } else {
                "网络通信失败".to_string()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_error(code: &str) -> ApiError {
        ApiError::Http {
            status: 401,
            code: code.to_string(),
            message: "test".to_string(),
            request_id: Some("request-1".to_string()),
            details: None,
        }
    }

    #[test]
    fn terminal_session_errors_are_distinct_from_network_failures() {
        for code in [
            "session_invalid",
            "session_device_mismatch",
            "refresh_token_invalid",
            "refresh_token_reused",
            "account_disabled",
            "account_unavailable",
        ] {
            let error = http_error(code);
            assert!(error.is_terminal_session_error(), "{code}");
            assert!(!error.is_network_error(), "{code}");
        }

        let network = ApiError::Network {
            message: "offline".to_string(),
            timeout: false,
        };
        assert!(network.is_network_error());
        assert!(!network.is_terminal_session_error());
    }

    #[test]
    fn update_and_access_token_errors_are_classified_without_revoking_offline_state() {
        assert!(http_error("client_update_required").is_client_update_required());
        assert!(http_error("access_token_invalid").is_access_token_rejected());
        assert!(!http_error("access_token_invalid").is_terminal_session_error());
    }

    #[test]
    fn generation_business_errors_have_actionable_messages() {
        assert_eq!(
            http_error("insufficient_credits").generation_message(),
            "积分不足，请充值后重试"
        );
        assert!(http_error("membership_quality_forbidden")
            .generation_message()
            .contains("升级会员"));
        assert!(http_error("model_unavailable")
            .generation_message()
            .contains("刷新模型目录"));
    }
}
