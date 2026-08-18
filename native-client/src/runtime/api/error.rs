use serde_json::Value;

pub(crate) fn is_generation_content_policy_blocked(code: &str, message: &str) -> bool {
    let code = code.trim().to_ascii_lowercase();
    let message = message.trim().to_lowercase();
    let blocked_code = [
        "content_policy_violation",
        "content_policy_blocked",
        "content_filter",
        "content_filtered",
        "safety_blocked",
        "moderation_blocked",
        "prompt_blocked",
        "image_safety",
        "sensitive_content",
        "policy_violation",
        "prohibited_content",
        "responsible_ai_policy_violation",
    ]
    .iter()
    .any(|marker| code == *marker)
        || (["content", "safety", "moderation", "policy"]
            .iter()
            .any(|marker| code.contains(marker))
            && ["blocked", "filtered", "violation", "rejected"]
                .iter()
                .any(|marker| code.contains(marker)));
    let blocked_message = [
        "violated the content policy",
        "violates the content policy",
        "blocked by the safety",
        "blocked by safety",
        "content was filtered",
        "content has been filtered",
        "rejected by the safety system",
        "blocked due to the safety policy",
        "violates the safety policy",
        "内容安全规则",
        "内容政策",
        "安全审核未通过",
        "内容审核未通过",
        "违反规则",
        "违反了关于",
        "触发了安全",
        "触发安全",
    ]
    .iter()
    .any(|marker| message.contains(marker));

    blocked_code || blocked_message
}

pub(crate) fn generation_content_policy_message(message: &str) -> String {
    let message = message.to_lowercase();
    let policy = if [
        "裸露",
        "色情",
        "情色",
        "性内容",
        "nudity",
        "sexual",
        "porn",
        "adult content",
        "nsfw",
    ]
    .iter()
    .any(|marker| message.contains(marker))
    {
        "裸露、色情或情色内容"
    } else if ["未成年人", "儿童色情", "minor", "child sexual"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        "未成年人安全内容"
    } else if ["暴力", "血腥", "violence", "gore"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        "暴力或血腥内容"
    } else {
        "内容安全规则"
    };

    format!(
        "生成失败：提示词或参考图可能涉及{policy}，已被上游安全系统拦截。请调整内容后重试；此类违规拦截不返还积分。"
    )
}

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

    pub(crate) fn is_invitation_code_already_submitted(&self) -> bool {
        matches!(
            self.code(),
            Some(
                "invitation_code_already_submitted"
                    | "invitation_code_already_used"
                    | "invitation_already_bound"
            )
        )
    }

    pub(crate) fn should_preserve_generation_recovery(&self) -> bool {
        match self {
            Self::Network { .. } | Self::Protocol { .. } => true,
            Self::Http { status, .. } => *status >= 500 || matches!(*status, 408 | 425 | 429),
            _ => false,
        }
    }

    pub(crate) fn should_preserve_redemption_retry(&self) -> bool {
        match self {
            Self::Network { .. } | Self::Protocol { .. } | Self::LocalState { .. } => true,
            Self::Http { status, code, .. } => {
                *status >= 500
                    || matches!(*status, 408 | 425 | 429)
                    || code == "request_in_progress"
            }
            _ => false,
        }
    }

    pub(crate) fn user_message(&self) -> String {
        match self.code() {
            Some("email_code_invalid" | "verification_code_invalid") => {
                "验证码不正确或已失效".to_string()
            }
            Some("password_invalid" | "email_password_invalid" | "invalid_credentials") => {
                "邮箱或密码不正确".to_string()
            }
            Some("password_login_unavailable") => {
                "密码登录服务暂未开放，请使用验证码登录".to_string()
            }
            Some("email_code_rate_limited" | "rate_limited") => {
                "操作过于频繁，请稍后再试".to_string()
            }
            Some("email_already_bound") => "当前账号已经绑定邮箱".to_string(),
            Some(
                "invitation_code_already_submitted"
                | "invitation_code_already_used"
                | "invitation_already_bound",
            ) => "当前账号已填写过邀请码，每个账号只能填写一次".to_string(),
            Some("email_identity_conflict") => "该邮箱已属于其他账号，不能直接绑定".to_string(),
            Some("email_delivery_failed") => "验证码发送失败，请稍后重试".to_string(),
            Some("wechat_login_unavailable") => "微信登录暂未开放，请使用邮箱登录".to_string(),
            Some("wechat_login_expired") => "二维码已失效，请点击刷新".to_string(),
            Some("wechat_code_invalid" | "wechat_profile_unavailable") => {
                "微信授权未完成，请刷新二维码重试".to_string()
            }
            Some("wechat_provider_unavailable") => "微信服务暂时不可用，请稍后重试".to_string(),
            Some("wechat_already_bound") => "当前账号已经绑定微信".to_string(),
            Some("wechat_identity_conflict") => {
                "该微信已绑定其他账号，请更换微信后重试".to_string()
            }
            Some("wechat_binding_expired") => "绑定二维码已失效，请点击刷新".to_string(),
            Some("wechat_unbind_not_allowed") => {
                "当前账号只能使用微信登录，不能解绑唯一登录方式".to_string()
            }
            Some("wechat_not_bound") => "当前账号尚未绑定微信".to_string(),
            Some("agreement_acceptance_required") => "请阅读并同意最新协议后重试".to_string(),
            Some("client_update_required") => "当前客户端版本过旧，请更新后重试".to_string(),
            Some("account_disabled" | "account_unavailable") => {
                "当前账号暂不可用，请联系客服".to_string()
            }
            Some(
                "authentication_required"
                | "access_token_invalid"
                | "session_invalid"
                | "session_device_mismatch"
                | "refresh_token_invalid"
                | "refresh_token_reused",
            ) => "登录状态已失效，请重新登录".to_string(),
            Some("insufficient_credits") => "积分不足，请充值后重试".to_string(),
            Some("membership_quality_forbidden") => {
                "当前会员不支持所选清晰度，请降低清晰度或升级会员".to_string()
            }
            Some("model_quality_unavailable") => {
                "当前模型暂不支持所选清晰度，请更换清晰度".to_string()
            }
            Some("model_aspect_ratio_unsupported") => {
                "当前模型不支持所选画面比例，请更换比例后重试".to_string()
            }
            Some("model_references_unsupported") => {
                "当前模型不支持参考图，请移除参考图或更换模型".to_string()
            }
            Some("model_task_type_unsupported") => {
                "当前模型不支持这项图片操作，请更换模型".to_string()
            }
            Some("image_target_size_invalid") => {
                "放大尺寸超过所选清晰度上限，请调整清晰度后重试".to_string()
            }
            Some("watermark_image_aspect_ratio_unsupported") => {
                "图片比例超过 3:1，暂时无法一键去水印".to_string()
            }
            Some(
                "watermark_image_dimensions_invalid" | "watermark_image_dimensions_unsupported",
            ) => "图片尺寸不符合去水印要求，请更换图片后重试".to_string(),
            Some("image_enhancement_type_unsupported") => {
                "图片变清晰仅支持 JPG、PNG 和 WebP 格式".to_string()
            }
            Some("image_enhancement_file_too_large") => "图片超过 20 MB，请压缩后重试".to_string(),
            Some("image_enhancement_aspect_ratio_unsupported") => {
                "图片比例超过 2:1，暂时无法进行生成式超分".to_string()
            }
            Some(
                "image_enhancement_dimensions_invalid"
                | "image_enhancement_dimensions_too_small"
                | "image_enhancement_dimensions_too_large",
            ) => "图片尺寸不符合超分要求，请更换图片后重试".to_string(),
            Some("image_enhancement_quality_invalid") => "请选择 2K 或 4K 清晰度".to_string(),
            Some("image_cutout_type_unsupported") => {
                "智能抠图仅支持 JPG、PNG 和 WebP 格式".to_string()
            }
            Some("image_cutout_file_too_large") => {
                "图片超过所选抠图类型的大小限制，可改选“通用”或压缩后重试".to_string()
            }
            Some(
                "image_cutout_dimensions_invalid"
                | "image_cutout_dimensions_too_small"
                | "image_cutout_dimensions_too_large",
            ) => "图片尺寸超出所选抠图类型范围，可改选“通用”或调整图片尺寸".to_string(),
            Some("image_cutout_subject_type_invalid") => "请选择有效的抠图主体类型".to_string(),
            Some("image_colorization_type_unsupported") => {
                "老照片上色仅支持 JPG、PNG 和 BMP 格式".to_string()
            }
            Some("image_colorization_file_too_large") => "图片超过 10 MB，请压缩后重试".to_string(),
            Some(
                "image_colorization_dimensions_invalid" | "image_colorization_dimensions_too_large",
            ) => "图片宽高均需小于 3000 像素".to_string(),
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
            Some("delivery_checksum_mismatch") => "生成文件完整性校验失败，请重新下载".to_string(),
            Some("membership_plan_unavailable" | "credit_pack_unavailable") => {
                "所选商品已下线，请刷新后重试".to_string()
            }
            Some("membership_upgrade_required") => "请使用会员升级入口完成购买".to_string(),
            Some("membership_downgrade_unsupported") => "当前暂不支持降级会员套餐".to_string(),
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

    pub(crate) fn redemption_message(&self, english: bool) -> String {
        if matches!(self, Self::Http { status: 404, .. }) {
            return if english {
                "This server version does not support redemption codes yet.".to_string()
            } else {
                "当前服务端版本暂不支持兑换码，请稍后再试".to_string()
            };
        }
        let chinese = match self.code() {
            Some("redemption_code_unavailable") => {
                "兑换码无效、已使用或当前不可用，请检查后重试".to_string()
            }
            Some("redemption_rate_limited") => "操作过于频繁，请稍后再试".to_string(),
            Some("redemption_service_disabled") => "兑换码服务暂未开放，请稍后再试".to_string(),
            Some("redemption_rate_limit_unavailable") => {
                "兑换服务暂时不可用，请稍后再试".to_string()
            }
            Some("idempotency_key_mismatch") => "兑换请求标识不一致，请重试".to_string(),
            Some("idempotency_key_required" | "idempotency_key_conflict") => {
                "兑换请求校验失败，请重新提交".to_string()
            }
            Some("request_in_progress") => "兑换请求正在处理中，请稍后再试".to_string(),
            Some("validation_failed" | "validation_error") => {
                "兑换码格式不正确，请检查后重试".to_string()
            }
            _ => match self {
                Self::LocalState { .. } => "兑换任务意外中断，请重试".to_string(),
                _ => self.user_message(),
            },
        };
        if !english {
            return chinese;
        }
        match self.code() {
            Some("redemption_code_unavailable") => {
                "The code is invalid, already used, or currently unavailable.".to_string()
            }
            Some("redemption_rate_limited") => {
                "Too many attempts. Please try again later.".to_string()
            }
            Some("redemption_service_disabled") => {
                "Redemption codes are not available yet. Please try again later.".to_string()
            }
            Some("redemption_rate_limit_unavailable") => {
                "The redemption service is temporarily unavailable.".to_string()
            }
            Some("idempotency_key_mismatch") => {
                "The redemption request identifiers do not match. Please try again.".to_string()
            }
            Some("idempotency_key_required" | "idempotency_key_conflict") => {
                "The redemption request could not be verified. Please submit it again.".to_string()
            }
            Some("request_in_progress") => {
                "This redemption is still being processed. Please try again shortly.".to_string()
            }
            Some("validation_failed" | "validation_error") => {
                "The redemption code format is invalid.".to_string()
            }
            _ => match self {
                Self::Network { timeout: true, .. } => {
                    "The request timed out. Please try again.".to_string()
                }
                Self::Network { .. } => {
                    "Unable to reach the server. Check your network and try again.".to_string()
                }
                Self::Protocol { .. } => {
                    "The server returned an invalid response. Please try again.".to_string()
                }
                Self::AuthenticationRequired => {
                    "Please sign in before redeeming a code.".to_string()
                }
                Self::Credential { .. } => {
                    "Your secure session could not be used. Please sign in again.".to_string()
                }
                Self::Configuration { .. } => {
                    "The client configuration is invalid. Please contact support.".to_string()
                }
                Self::LocalState { .. } => {
                    "The redemption task was interrupted. Please try again.".to_string()
                }
                Self::Http { .. } => "The service is temporarily unavailable.".to_string(),
            },
        }
    }

    pub(crate) fn generation_message(&self) -> String {
        if let Self::Http { code, message, .. } = self {
            if is_generation_content_policy_blocked(code, message) {
                return generation_content_policy_message(message);
            }
        }

        match self.code() {
            Some("insufficient_credits") => "积分不足，请充值后重试".to_string(),
            Some("membership_quality_forbidden") => {
                "当前会员不支持所选清晰度，请降低清晰度或升级会员".to_string()
            }
            Some("model_quality_unavailable") => {
                "当前模型暂不支持所选清晰度，请更换清晰度".to_string()
            }
            Some("model_aspect_ratio_unsupported") => {
                "当前模型不支持所选画面比例，请更换比例后重试".to_string()
            }
            Some("model_references_unsupported") => {
                "当前模型不支持参考图，请移除参考图或更换模型".to_string()
            }
            Some("model_task_type_unsupported") => {
                "当前模型不支持这项图片操作，请更换模型".to_string()
            }
            Some("image_target_size_invalid") => {
                "放大尺寸超过所选清晰度上限，请调整清晰度后重试".to_string()
            }
            Some("watermark_image_aspect_ratio_unsupported") => {
                "图片比例超过 3:1，暂时无法一键去水印".to_string()
            }
            Some(
                "watermark_image_dimensions_invalid" | "watermark_image_dimensions_unsupported",
            ) => "图片尺寸不符合去水印要求，请更换图片后重试".to_string(),
            Some("image_enhancement_type_unsupported") => {
                "图片变清晰仅支持 JPG、PNG 和 WebP 格式".to_string()
            }
            Some("image_enhancement_file_too_large") => "图片超过 20 MB，请压缩后重试".to_string(),
            Some("image_enhancement_aspect_ratio_unsupported") => {
                "图片比例超过 2:1，暂时无法进行生成式超分".to_string()
            }
            Some(
                "image_enhancement_dimensions_invalid"
                | "image_enhancement_dimensions_too_small"
                | "image_enhancement_dimensions_too_large",
            ) => "图片尺寸不符合超分要求，请更换图片后重试".to_string(),
            Some("image_enhancement_quality_invalid") => "请选择 2K 或 4K 清晰度".to_string(),
            Some("image_cutout_type_unsupported") => {
                "智能抠图仅支持 JPG、PNG 和 WebP 格式".to_string()
            }
            Some("image_cutout_file_too_large") => {
                "图片超过所选抠图类型的大小限制，可改选“通用”或压缩后重试".to_string()
            }
            Some(
                "image_cutout_dimensions_invalid"
                | "image_cutout_dimensions_too_small"
                | "image_cutout_dimensions_too_large",
            ) => "图片尺寸超出所选抠图类型范围，可改选“通用”或调整图片尺寸".to_string(),
            Some("image_cutout_subject_type_invalid") => "请选择有效的抠图主体类型".to_string(),
            Some("image_colorization_type_unsupported") => {
                "老照片上色仅支持 JPG、PNG 和 BMP 格式".to_string()
            }
            Some("image_colorization_file_too_large") => "图片超过 10 MB，请压缩后重试".to_string(),
            Some(
                "image_colorization_dimensions_invalid" | "image_colorization_dimensions_too_large",
            ) => "图片宽高均需小于 3000 像素".to_string(),
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
            Some("delivery_checksum_mismatch") => "生成文件完整性校验失败，请重新下载".to_string(),
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
    fn invitation_code_reuse_errors_lock_the_single_submission_flow() {
        for code in [
            "invitation_code_already_submitted",
            "invitation_code_already_used",
            "invitation_already_bound",
        ] {
            let error = http_error(code);
            assert!(error.is_invitation_code_already_submitted(), "{code}");
            assert_eq!(
                error.user_message(),
                "当前账号已填写过邀请码，每个账号只能填写一次"
            );
        }
        assert!(!http_error("invitation_code_invalid").is_invitation_code_already_submitted());
    }

    #[test]
    fn password_login_errors_have_actionable_messages() {
        assert_eq!(
            http_error("invalid_credentials").user_message(),
            "邮箱或密码不正确"
        );
        assert_eq!(
            http_error("password_login_unavailable").user_message(),
            "密码登录服务暂未开放，请使用验证码登录"
        );
    }

    #[test]
    fn redemption_errors_have_stable_actionable_messages() {
        let cases = [
            (
                "redemption_code_unavailable",
                "兑换码无效、已使用或当前不可用，请检查后重试",
            ),
            ("redemption_rate_limited", "操作过于频繁，请稍后再试"),
            (
                "redemption_service_disabled",
                "兑换码服务暂未开放，请稍后再试",
            ),
            (
                "redemption_rate_limit_unavailable",
                "兑换服务暂时不可用，请稍后再试",
            ),
            ("idempotency_key_required", "兑换请求校验失败，请重新提交"),
            ("idempotency_key_conflict", "兑换请求校验失败，请重新提交"),
            ("idempotency_key_mismatch", "兑换请求标识不一致，请重试"),
            ("request_in_progress", "兑换请求正在处理中，请稍后再试"),
            ("validation_failed", "兑换码格式不正确，请检查后重试"),
        ];

        for (code, expected) in cases {
            assert_eq!(
                http_error(code).redemption_message(false),
                expected,
                "{code}"
            );
        }
    }

    #[test]
    fn redemption_404_identifies_an_old_server_without_exposing_internals() {
        let error = ApiError::Http {
            status: 404,
            code: "route_not_found".to_string(),
            message: "POST /v1/credits/redemptions was not found".to_string(),
            request_id: Some("request-1".to_string()),
            details: None,
        };

        let message = error.redemption_message(false);
        assert_eq!(message, "当前服务端版本暂不支持兑换码，请稍后再试");
        assert!(!message.contains("route_not_found"));
        assert!(!message.contains("request-1"));
    }

    #[test]
    fn redemption_errors_have_english_messages_when_the_ui_is_english() {
        assert_eq!(
            http_error("redemption_code_unavailable").redemption_message(true),
            "The code is invalid, already used, or currently unavailable."
        );
        assert_eq!(
            http_error("idempotency_key_mismatch").redemption_message(true),
            "The redemption request identifiers do not match. Please try again."
        );
        assert_eq!(
            ApiError::Network {
                message: "offline".to_string(),
                timeout: true,
            }
            .redemption_message(true),
            "The request timed out. Please try again."
        );
    }

    #[test]
    fn interrupted_redemption_uses_a_redemption_specific_chinese_message() {
        assert_eq!(
            ApiError::LocalState {
                message: "worker disconnected".to_string(),
            }
            .redemption_message(false),
            "兑换任务意外中断，请重试"
        );
    }

    #[test]
    fn only_ambiguous_redemption_failures_keep_the_same_retry_identity() {
        assert!(ApiError::Network {
            message: "timeout".to_string(),
            timeout: true,
        }
        .should_preserve_redemption_retry());
        assert!(http_error("request_in_progress").should_preserve_redemption_retry());
        assert!(!http_error("redemption_code_unavailable").should_preserve_redemption_retry());
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
        assert!(http_error("model_aspect_ratio_unsupported")
            .generation_message()
            .contains("画面比例"));
        assert!(http_error("image_cutout_file_too_large")
            .generation_message()
            .contains("改选“通用”"));
        assert!(http_error("image_cutout_dimensions_too_large")
            .generation_message()
            .contains("调整图片尺寸"));
    }

    #[test]
    fn generation_http_content_policy_failure_has_no_refund_message() {
        let error = ApiError::Http {
            status: 400,
            code: "provider_rejected".to_string(),
            message: "生成的图片可能违反了关于裸露、色情或情色内容的防护规则".to_string(),
            request_id: Some("request-1".to_string()),
            details: None,
        };

        let message = error.generation_message();
        assert!(message.contains("上游安全系统拦截"));
        assert!(message.contains("裸露、色情或情色内容"));
        assert!(message.contains("不返还积分"));
    }

    #[test]
    fn generation_http_content_filter_outage_is_not_a_policy_block() {
        let error = ApiError::Http {
            status: 503,
            code: "content_filter_service_error".to_string(),
            message: "内容审核服务暂时不可用，请稍后重试".to_string(),
            request_id: Some("request-1".to_string()),
            details: None,
        };

        assert_eq!(error.generation_message(), "服务暂时异常，请稍后重试");
    }

    #[test]
    fn generation_http_safety_service_outage_is_not_a_policy_block() {
        let error = ApiError::Http {
            status: 503,
            code: "provider_service_error".to_string(),
            message: "Safety system temporarily unavailable".to_string(),
            request_id: Some("request-1".to_string()),
            details: None,
        };

        assert_eq!(error.generation_message(), "服务暂时异常，请稍后重试");
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
