// Legacy status producers share a string channel. Opt in to error feedback:
// an unfamiliar informational message must not become a retry/fee warning.
pub(super) fn is_generation_error_message(message: &str) -> bool {
    let message = message.trim().to_lowercase();
    [
        "失败", "错误", "异常", "无法", "未能", "未找到", "未返回", "不可用",
        "不支持", "不允许", "不匹配", "无效", "已过期", "超时", "拦截", "拒绝",
        "不足", "未初始化", "尚未初始化", "请先", "请输入", "请选择", "请重新",
        "超过限制", "超出限制", "连接中断", "会话已失效", "登录已失效",
        "failed", "failure", "error", "invalid", "unavailable", "unsupported",
        "not supported", "not found", "cannot", "could not", "unable to", "expired",
        "timed out", "timeout", "blocked", "rejected", "insufficient", "denied",
        "upload a reference", "upload a subject", "upload a building", "enter a prompt",
        "select an image model", "sign in first",
    ].iter().any(|marker| message.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn informational_statuses_do_not_offer_error_feedback() {
        for message in ["", "已添加参考图", "已从剪贴板粘贴参考图", "任务已提交，正在排队...",
            "正在生成...", "图片下载完成", "生成成功", "已停止生成",
            "Reference image added", "Task submitted, queued", "Generation completed"] {
            assert!(!is_generation_error_message(message), "{message}");
        }
        for message in ["服务响应异常，请稍后重试", "无法保存参考图", "生成失败", "积分不足",
            "请先上传主体参考图", "已被上游安全系统拦截", "Request timed out", "Image download failed"] {
            assert!(is_generation_error_message(message), "{message}");
        }
    }
}
