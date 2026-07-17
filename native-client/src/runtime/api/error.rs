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

    pub(crate) fn is_insufficient_credits(&self) -> bool {
        self.code() == Some("insufficient_credits")
    }

    pub(crate) fn user_message(&self) -> String {
        match self.code() {
            Some("email_code_invalid" | "verification_code_invalid") => {
                "验证码不正确或已失效".to_string()
            }
            Some("email_code_rate_limited" | "rate_limited") => {
                "操作过于频繁，请稍后再试".to_string()
            }
            Some("agreement_acceptance_required") => {
                "请阅读并同意最新协议后重试".to_string()
            }
            Some("client_update_required") => "当前客户端版本过旧，请更新后重试".to_string(),
            Some("account_disabled" | "account_unavailable") => {
                "当前账号暂不可用，请联系客服".to_string()
            }
            Some("authentication_required" | "access_token_invalid" | "session_invalid"
                | "session_device_mismatch" | "refresh_token_invalid" | "refresh_token_reused") => {
                "登录状态已失效，请重新登录".to_string()
            }
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
            Some("client_request_conflict" | "idempotency_key_conflict") => {
                "请求记录已变化，请重新发起操作".to_string()
            }
            Some("delivery_checksum_mismatch") => {
                "生成文件完整性校验失败，请重新下载".to_string()
            }
            Some("membership_plan_unavailable" | "credit_pack_unavailable") => {
                "所选商品已下线，请刷新后重试".to_string()
            }
            Some("membership_upgrade_required") => "请使用会员升级入口完成购买".to_string(),
            Some("membership_downgrade_unsupported") => {
                "当前暂不支持降级会员套餐".to_string()
            }
            Some("membership_operation_in_progress") => {
                "已有会员订单正在处理中，请稍后再试".to_string()
            }
            Some("membership_missing" | "membership_upgrade_invalid") => {
                "当前会员状态暂不支持升级，请刷新后重试".to_string()
            }
            Some("payment_amount_mismatch") => "支付金额校验失败，请重新下单".to_string(),
            Some("order_not_found") => "订单不存在或已失效，请重新操作".to_string(),
            Some("validation_error") => "提交内容有误，请检查后重试".to_string(),
            Some(_) => "服务暂时异常，请稍后重试".to_string(),
            None => match self {
                Self::Http { .. } => "服务暂时异常，请稍后重试".to_string(),
                Self::Network { timeout: true, .. } => "请求超时，请稍后重试".to_string(),
                Self::Network { .. } => "无法连接服务端，请检查网络后重试".to_string(),
                Self::Protocol { .. } => "服务响应异常，请稍后重试".to_string(),
                Self::AuthenticationRequired => "请先登录后再继续操作".to_string(),
                Self::Credential { .. } => "安全凭据处理失败，请重新登录".to_string(),
                Self::Configuration { .. } => "客户端配置异常，请联系管理员".to_string(),
                Self::LocalState { .. } => "本地数据保存失败，请重试".to_string(),
            },
        }
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
            _ => self.user_message(),
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

    #[test]
    fn user_messages_hide_request_ids_and_internal_codes() {
        let error = http_error("email_code_invalid");
        let message = error.user_message();
        assert_eq!(message, "验证码不正确或已失效");
        assert!(!message.contains("request-1"));
        assert!(!message.contains("email_code_invalid"));

        let unknown = http_error("unexpected_internal_error");
        assert_eq!(unknown.user_message(), "服务暂时异常，请稍后重试");
    }
}
