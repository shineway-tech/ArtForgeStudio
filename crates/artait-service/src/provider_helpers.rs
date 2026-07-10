//! Provider 辅助函数：密钥管理、模型选项、API 风格、默认值修正。

use artait_model::{AppConfig, ProviderFamily, ProviderInstance, ProviderScope};
use artait_task::TaskError;

// ── 密钥管理 ──────────────────────────────────────────────────────────────

/// 将系统凭据同步到配置内存。返回是否有变动。
pub fn normalize_provider_secrets(cfg: &mut AppConfig) -> bool {
    let mut changed = false;
    for inst in &mut cfg.providers {
        if inst.family != ProviderFamily::OpenAiCompatible {
            continue;
        }

        if inst.secret_ref.is_none() {
            inst.secret_ref = Some(artait_config::secret_store::ref_key(&inst.id, "api_key"));
            changed = true;
        }

        if inst
            .api_key
            .as_deref()
            .is_some_and(|secret| !secret.trim().is_empty())
        {
            continue;
        }

        if let Some(secret) = inst
            .secret_ref
            .as_deref()
            .and_then(|key| artait_config::secret_store::get(key).ok().flatten())
            .map(|secret| secret.trim().to_string())
            .filter(|secret| !secret.is_empty())
        {
            inst.api_key = Some(secret);
            tracing::info!(provider = %inst.id, "系统凭据 API Key 已同步到配置文件");
            changed = true;
        }
    }
    changed
}

pub fn set_provider_secret(inst: &mut ProviderInstance, secret: &str) {
    let secret = secret.trim();
    if secret.is_empty() {
        return;
    }

    if inst.secret_ref.is_none() {
        inst.secret_ref = Some(artait_config::secret_store::ref_key(&inst.id, "api_key"));
    }
    inst.api_key = Some(secret.to_string());
}

pub fn provider_display_api_key(inst: &ProviderInstance) -> Option<String> {
    if let Some(secret) = inst
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|secret| !secret.is_empty())
    {
        return Some(secret.to_string());
    }

    inst.secret_ref
        .as_deref()
        .and_then(|key| artait_config::secret_store::get(key).ok().flatten())
        .map(|secret| secret.trim().to_string())
        .filter(|secret| !secret.is_empty())
}

pub fn load_provider_secret(
    inst: &ProviderInstance,
    ctx: Option<&artait_task::TaskContext>,
) -> Result<Option<String>, TaskError> {
    if let Some(secret) = inst
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(ctx) = ctx {
            ctx.info("已读取配置文件 API Key");
        }
        return Ok(Some(secret.to_string()));
    }

    if let Some(key) = inst.secret_ref.as_deref() {
        match artait_config::secret_store::get(key) {
            Ok(Some(secret)) if !secret.trim().is_empty() => {
                if let Some(ctx) = ctx {
                    ctx.info(format!("已读取系统凭据 API Key · {}", key));
                }
                tracing::debug!(key, "provider credential secret loaded");
                return Ok(Some(secret));
            }
            Ok(_) => {
                tracing::warn!(provider = %inst.id, secret_ref = %key, "provider secret missing in credential manager");
            }
            Err(e) => {
                tracing::warn!(provider = %inst.id, secret_ref = %key, error = %e, "provider secret read failed");
            }
        }
    } else {
        tracing::warn!(provider = %inst.id, "provider missing secret_ref");
    }

    let msg = if let Some(key) = inst.secret_ref.as_deref() {
        format!(
            "配置文件里没有 API Key：{}。请编辑节点重新填入 API Key 后保存",
            key
        )
    } else {
        format!("{} 没有 API Key，请在设置里编辑节点并保存", inst.name)
    };
    if let Some(ctx) = ctx {
        ctx.warn(&msg);
    }
    if let Some(key) = inst.secret_ref.as_deref() {
        tracing::warn!(provider = %inst.id, secret_ref = %key, "provider secret missing");
    }
    Err(TaskError::Failed(msg))
}

// ── 模型选项 ──────────────────────────────────────────────────────────────

pub fn parse_model_options(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in raw.split(['\n', '\r', ',', ';', '，', '；']) {
        let model = part.trim();
        if model.is_empty() || out.iter().any(|item: &String| item == model) {
            continue;
        }
        out.push(model.to_string());
    }
    out
}

pub fn ensure_model_option(options: &mut Vec<String>, model: &str) {
    let model = model.trim();
    if model.is_empty() || options.iter().any(|item| item == model) {
        return;
    }
    options.push(model.to_string());
}

pub fn merge_model_options(options: &mut Vec<String>, fetched: &[String]) {
    for model in fetched {
        ensure_model_option(options, model);
    }
}

pub fn format_model_options(default: Option<&str>, options: &[String]) -> String {
    let mut models = Vec::new();
    if let Some(default) = default.map(str::trim).filter(|s| !s.is_empty()) {
        models.push(default.to_string());
    }
    for model in options.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !models.iter().any(|item| item == model) {
            models.push(model.to_string());
        }
    }
    models.join("\n")
}

pub fn apply_model_options(
    inst: &mut ProviderInstance,
    generation: Vec<String>,
    analysis: Vec<String>,
) {
    inst.models.generation_model = generation.first().cloned();
    inst.models.generation_model_options = generation;
    inst.models.analysis_model = analysis.first().cloned();
    inst.models.analysis_model_options = analysis;
    sync_provider_model_extra(inst);
}

// ── Scope 转换 ────────────────────────────────────────────────────────────

pub fn scopes_from_node_kind(kind: &str) -> Vec<ProviderScope> {
    match kind {
        "generation" => vec![ProviderScope::Generation],
        "analysis" => vec![ProviderScope::Analysis],
        _ => vec![ProviderScope::Generation, ProviderScope::Analysis],
    }
}

pub fn node_kind_from_scopes(scopes: &[ProviderScope]) -> &'static str {
    let has_generation = scopes.contains(&ProviderScope::Generation);
    let has_analysis = scopes.contains(&ProviderScope::Analysis);
    match (has_generation, has_analysis) {
        (true, false) => "generation",
        (false, true) => "analysis",
        _ => "both",
    }
}

// ── API 风格 ──────────────────────────────────────────────────────────────

pub fn normalized_api_style(style: &str) -> &'static str {
    match style.trim().to_ascii_lowercase().as_str() {
        "cpa" | "cpa_api" | "toapis" | "to_apis" | "toapis_gpt_image_2" => "cpa",
        "sub2api" | "sub2_api" => "sub2api",
        "newapi" | "new_api" => "newapi",
        "gemini" => "gemini",
        "images" | "openai_images" | "openai_image" => "images",
        "responses" => "responses",
        "messages" => "messages",
        "chat" => "chat",
        _ => "auto",
    }
}

pub fn provider_api_style(inst: &ProviderInstance) -> &'static str {
    inst.extra
        .get("api_style")
        .and_then(|v| v.as_str())
        .map(normalized_api_style)
        .unwrap_or("auto")
}

pub fn apply_provider_api_style(inst: &mut ProviderInstance, style: &str) {
    let mut extra = inst.extra.as_object().cloned().unwrap_or_default();
    match normalized_api_style(style) {
        "auto" => {
            extra.remove("api_style");
        }
        value => {
            extra.insert("api_style".into(), serde_json::Value::String(value.into()));
        }
    }
    inst.extra = serde_json::Value::Object(extra);
}

// ── Provider 规范化 ───────────────────────────────────────────────────────

pub fn normalize_provider_for_scopes(inst: &mut ProviderInstance) {
    if !inst.scopes.contains(&ProviderScope::Generation) {
        inst.models.generation_model = None;
        inst.models.generation_model_options.clear();
    }
    if !inst.scopes.contains(&ProviderScope::Analysis) {
        inst.models.analysis_model = None;
        inst.models.analysis_model_options.clear();
    }
    sync_provider_model_extra(inst);
}

pub fn fix_provider_defaults(cfg: &mut AppConfig) {
    if !provider_default_valid(
        cfg,
        cfg.provider_defaults.generation.as_deref(),
        ProviderScope::Generation,
    ) {
        cfg.provider_defaults.generation = first_provider_for_scope(cfg, ProviderScope::Generation);
    }
    if !provider_default_valid(
        cfg,
        cfg.provider_defaults.analysis.as_deref(),
        ProviderScope::Analysis,
    ) {
        cfg.provider_defaults.analysis = first_provider_for_scope(cfg, ProviderScope::Analysis);
    }
}

pub fn sync_provider_model_extra(inst: &mut ProviderInstance) {
    let mut extra = inst.extra.as_object().cloned().unwrap_or_default();
    if let Some(ref model) = inst.models.generation_model {
        extra.insert(
            "generation_model".into(),
            serde_json::Value::String(model.clone()),
        );
    } else {
        extra.remove("generation_model");
    }
    if let Some(ref model) = inst.models.analysis_model {
        extra.insert(
            "analysis_model".into(),
            serde_json::Value::String(model.clone()),
        );
    } else {
        extra.remove("analysis_model");
    }
    inst.extra = serde_json::Value::Object(extra);
}

fn provider_default_valid(cfg: &AppConfig, id: Option<&str>, scope: ProviderScope) -> bool {
    let Some(id) = id else {
        return false;
    };
    cfg.providers
        .iter()
        .any(|p| p.id == id && p.show_in_main_ui && p.scopes.contains(&scope))
}

fn first_provider_for_scope(cfg: &AppConfig, scope: ProviderScope) -> Option<String> {
    cfg.providers
        .iter()
        .find(|p| p.show_in_main_ui && p.scopes.contains(&scope))
        .map(|p| p.id.clone())
}

/// 执行一次 Provider 连接测试。供 TaskRunner closure 使用。
pub async fn run_connection_test(
    inst: &ProviderInstance,
    registry: &artait_provider::ProviderRegistry,
    http: std::sync::Arc<dyn artait_provider::HttpClient>,
    ctx: &artait_task::TaskContext,
) -> Result<String, artait_task::TaskError> {
    use artait_provider::ProviderContext;

    ctx.progress(0.2);

    let provider = registry
        .get(&inst.provider_id)
        .ok_or_else(|| artait_task::TaskError::Failed("未注册 provider".into()))?;

    let mut pctx = ProviderContext::with_http(inst.id.clone(), inst.provider_id.clone(), http);
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();
    pctx.cancellation = ctx.cancel.clone();
    pctx.secret = load_provider_secret(inst, Some(ctx))?;

    ctx.progress(0.5);
    ctx.info("调用 GET /models");

    match provider.test_connection(&pctx).await {
        Ok(s) if s.ok => {
            ctx.info(format!("OK: {}", s.message));
            ctx.progress(1.0);
            Ok(s.message)
        }
        Ok(s) => Err(artait_task::TaskError::Failed(format!(
            "失败: {}",
            s.message
        ))),
        Err(e) => Err(artait_task::TaskError::Failed(format!("{e}"))),
    }
}

/// 执行一次推理分析（Analyzer 调用）。供 TaskRunner closure 使用。
pub async fn run_analysis(
    inst: &ProviderInstance,
    req: artait_provider::request::AnalysisRequest,
    registry: &artait_provider::ProviderRegistry,
    http: std::sync::Arc<dyn artait_provider::HttpClient>,
    ctx: &artait_task::TaskContext,
) -> Result<artait_provider::request::AnalysisOutput, artait_task::TaskError> {
    use artait_provider::ProviderContext;

    let provider = registry.get(&inst.provider_id).ok_or_else(|| {
        artait_task::TaskError::Failed(format!("未找到 provider {}", inst.provider_id))
    })?;
    let analyzer = provider.as_analyzer().ok_or_else(|| {
        artait_task::TaskError::Failed(format!("{} 不支持推理分析", inst.provider_id))
    })?;

    let mut pctx = ProviderContext::with_http(inst.id.clone(), inst.provider_id.clone(), http);
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();
    pctx.cancellation = ctx.cancel.clone();
    pctx.secret = load_provider_secret(inst, Some(ctx))?;

    ctx.info("调用推理 provider");
    ctx.progress(0.3);

    analyzer
        .analyze(req, &pctx)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("分析失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use artait_model::ProviderModelConfig;

    fn make_openai_instance(id: &str, name: &str) -> ProviderInstance {
        ProviderInstance {
            id: id.into(),
            name: name.into(),
            provider_id: "openai-compatible".into(),
            family: ProviderFamily::OpenAiCompatible,
            scopes: vec![ProviderScope::Generation, ProviderScope::Analysis],
            show_in_main_ui: true,
            models: ProviderModelConfig::default(),
            endpoint: Some("https://api.openai.com/v1".into()),
            secret_ref: None,
            api_key: None,
            extra: serde_json::Value::Null,
        }
    }

    #[test]
    fn set_provider_secret_sets_key_and_ref() {
        let mut inst = make_openai_instance("o1", "test");
        set_provider_secret(&mut inst, "sk-test-key");
        assert_eq!(inst.api_key.as_deref(), Some("sk-test-key"));
        assert!(inst.secret_ref.is_some());
    }

    #[test]
    fn set_provider_secret_ignores_empty() {
        let mut inst = make_openai_instance("o1", "test");
        set_provider_secret(&mut inst, "   ");
        assert_eq!(inst.api_key, None);
    }

    #[test]
    fn provider_display_api_key_returns_memory_value() {
        let mut inst = make_openai_instance("o1", "test");
        inst.api_key = Some("sk-display-test".into());
        assert_eq!(
            provider_display_api_key(&inst).as_deref(),
            Some("sk-display-test")
        );
    }

    #[test]
    fn parse_model_options_splits_multi_format() {
        let v = parse_model_options("gpt-4o\nclaude-3,gemini-pro;llama-3");
        assert!(v.contains(&"gpt-4o".into()));
        assert!(v.contains(&"claude-3".into()));
        assert!(v.contains(&"gemini-pro".into()));
        assert!(v.contains(&"llama-3".into()));
    }

    #[test]
    fn parse_model_options_dedupes() {
        let v = parse_model_options("gpt-4o\ngpt-4o,claude-3");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn scopes_from_node_kind_generation() {
        let s = scopes_from_node_kind("generation");
        assert_eq!(s, vec![ProviderScope::Generation]);
    }

    #[test]
    fn scopes_from_node_kind_analysis() {
        let s = scopes_from_node_kind("analysis");
        assert_eq!(s, vec![ProviderScope::Analysis]);
    }

    #[test]
    fn scopes_from_node_kind_both_by_default() {
        let s = scopes_from_node_kind("unknown");
        assert!(s.contains(&ProviderScope::Generation));
        assert!(s.contains(&ProviderScope::Analysis));
    }

    #[test]
    fn node_kind_from_scopes_roundtrip() {
        assert_eq!(
            node_kind_from_scopes(&[ProviderScope::Generation]),
            "generation"
        );
        assert_eq!(
            node_kind_from_scopes(&[ProviderScope::Analysis]),
            "analysis"
        );
        assert_eq!(
            node_kind_from_scopes(&[ProviderScope::Generation, ProviderScope::Analysis]),
            "both"
        );
    }

    #[test]
    fn normalized_api_style_maps_variants() {
        assert_eq!(normalized_api_style("CPA"), "cpa");
        assert_eq!(normalized_api_style("ToAPIs"), "cpa");
        assert_eq!(normalized_api_style("sub2API"), "sub2api");
        assert_eq!(normalized_api_style("Gemini"), "gemini");
        assert_eq!(normalized_api_style("images"), "images");
        assert_eq!(normalized_api_style("responses"), "responses");
        assert_eq!(normalized_api_style("chat"), "chat");
        assert_eq!(normalized_api_style("unknown"), "auto");
    }

    #[test]
    fn normalize_secrets_adds_missing_ref() {
        let mut cfg = AppConfig::default();
        cfg.providers.push(make_openai_instance("o1", "test"));
        // api_key is None, secret_ref is None → should add ref
        let changed = normalize_provider_secrets(&mut cfg);
        assert!(changed);
        assert!(cfg.providers[0].secret_ref.is_some());
    }

    #[test]
    fn normalize_secrets_skips_non_openai() {
        let mut cfg = AppConfig::default();
        cfg.providers.push(ProviderInstance {
            family: ProviderFamily::Mock,
            ..make_openai_instance("m1", "mock")
        });
        let changed = normalize_provider_secrets(&mut cfg);
        assert!(!changed);
    }

    #[test]
    fn format_model_options_dedupes_with_default() {
        let options = vec!["gpt-4o".into(), "claude-3".into()];
        let formatted = format_model_options(Some("gpt-4o"), &options);
        assert_eq!(formatted, "gpt-4o\nclaude-3");
    }

    #[test]
    fn apply_provider_api_style_stores_value() {
        let mut inst = make_openai_instance("o1", "test");
        apply_provider_api_style(&mut inst, "gemini");
        let style = provider_api_style(&inst);
        assert_eq!(style, "gemini");
    }

    #[test]
    fn normalize_provider_for_scopes_clears_unscoped() {
        let mut inst = make_openai_instance("o1", "test");
        inst.scopes = vec![ProviderScope::Generation];
        inst.models.analysis_model = Some("claude-3".into());
        inst.models.analysis_model_options = vec!["claude-3".into()];
        normalize_provider_for_scopes(&mut inst);
        assert!(inst.models.analysis_model.is_none());
        assert!(inst.models.analysis_model_options.is_empty());
    }
}
