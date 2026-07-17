use super::*;

pub(super) fn short_text(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

pub(super) fn load_system_fonts() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut names = BTreeSet::new();
    for face in db.faces() {
        for (family, _) in &face.families {
            let name = family.trim();
            if !name.is_empty() {
                names.insert(name.to_string());
            }
        }
    }
    for fallback in [
        "Microsoft YaHei UI",
        "Microsoft YaHei",
        "SimSun",
        "SimHei",
        "DengXian",
        "Segoe UI",
        "Arial",
    ] {
        names.insert(fallback.to_string());
    }
    names.into_iter().collect()
}

pub(super) fn normalized_used_models(used: Vec<String>, models: &[String]) -> Vec<String> {
    let available = models.iter().cloned().collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    used.into_iter()
        .filter(|model| available.contains(model) && seen.insert(model.clone()))
        .collect()
}

pub(super) fn zh_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("504")
        || lower.contains("gateway time-out")
        || lower.contains("gateway timeout")
    {
        return "生成请求超时，后端可能仍在生成。".to_string();
    }
    if lower.contains("timeout") || raw.contains("超时") {
        "请求超时，请检查网络环境或服务商接口状态后重试。".to_string()
    } else if lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("network")
    {
        "网络连接失败，请检查网络环境、代理或服务商接口地址。".to_string()
    } else if lower.contains("unauthorized")
        || lower.contains("invalid api key")
        || lower.contains("401")
    {
        "平台模型服务鉴权失败，请稍后重试或联系管理员。".to_string()
    } else if lower.contains("forbidden") || lower.contains("permission") || lower.contains("403") {
        "当前账号没有调用该模型的权限，请更换模型或升级会员。".to_string()
    } else if lower.contains("not found") || lower.contains("404") {
        "模型或接口地址不存在，请检查模型名称和 API 请求地址。".to_string()
    } else if lower.contains("rate") || lower.contains("429") {
        "请求过于频繁或额度不足，请稍后重试。".to_string()
    } else if lower.contains("quota") || lower.contains("balance") || lower.contains("billing") {
        "账号额度不足或计费状态异常，请检查服务商账户。".to_string()
    } else if lower.contains("size") || lower.contains("resolution") {
        "当前模型不支持所选尺寸，请更换比例或分辨率。".to_string()
    } else if lower.contains("model")
        && (lower.contains("unsupported") || lower.contains("not support"))
    {
        "当前模型不支持这类请求，请确认已选择生图模型或更换模型。".to_string()
    } else if lower.contains("json") || lower.contains("parse") || lower.contains("deserialize") {
        "接口返回内容格式异常，请检查 API 请求地址和模型类型是否正确。".to_string()
    } else if lower.contains("no prompt")
        || lower.contains("returned no prompt")
        || lower.contains("empty")
    {
        "推理模型没有返回可用提示词，请确认选择的是支持文本输出的推理模型，并检查 API 返回内容。"
            .to_string()
    } else if lower.contains("image") {
        "图片生成失败，请检查 API 配置、模型能力或稍后重试。".to_string()
    } else if raw.trim().is_empty() {
        "接口返回错误，请检查 API 配置、模型能力或稍后重试。".to_string()
    } else {
        "接口返回错误，请检查 API 配置、模型能力或稍后重试。".to_string()
    }
}
