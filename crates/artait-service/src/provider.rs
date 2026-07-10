//! Provider 管理服务：实例创建、编辑、验证。

use crate::provider_helpers;
use artait_model::{
    AppConfig, ProviderFamily, ProviderInstance, ProviderModelConfig, ProviderScope,
};

/// 新增 Provider 实例的参数。
pub struct CreateProviderParams {
    pub name: String,
    pub endpoint: String,
    pub api_key: String,
    pub api_style: String,
    pub node_kind: String,
    pub generation_model: Option<String>,
    pub analysis_model: Option<String>,
}

/// 编辑 Provider 实例的参数。
pub struct EditProviderParams {
    pub name: String,
    pub endpoint: String,
    pub api_key: String,
    pub api_style: String,
    pub node_kind: String,
    pub generation_model: Option<String>,
    pub analysis_model: Option<String>,
}

/// 创建/编辑操作的返回结果。
#[derive(Debug)]
pub enum ProviderOpResult {
    Created {
        instance: ProviderInstance,
        should_test: bool,
    },
    Updated {
        display_name: String,
    },
}

/// 验证端点 URL。
pub fn validate_endpoint(endpoint: &str) -> Result<(), String> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() || (!trimmed.starts_with("http://") && !trimmed.starts_with("https://")) {
        Err("端点必须以 http:// 或 https:// 开头".into())
    } else {
        Ok(())
    }
}

/// 创建新 Provider 实例。
pub fn create_provider(
    cfg: &mut AppConfig,
    params: CreateProviderParams,
) -> Result<ProviderOpResult, String> {
    validate_endpoint(&params.endpoint)?;
    if params.api_key.trim().is_empty() {
        return Err("API Key 不能为空".into());
    }

    let scopes = provider_helpers::scopes_from_node_kind(&params.node_kind);
    let gen_models = if scopes.contains(&ProviderScope::Generation) {
        provider_helpers::parse_model_options(params.generation_model.as_deref().unwrap_or(""))
    } else {
        Vec::new()
    };
    let ana_models = if scopes.contains(&ProviderScope::Analysis) {
        provider_helpers::parse_model_options(params.analysis_model.as_deref().unwrap_or(""))
    } else {
        Vec::new()
    };

    let family = match params.api_style.as_str() {
        "volcengine" => ProviderFamily::VolcengineSeedance,
        "gemini" => ProviderFamily::GeminiCompatible,
        _ => ProviderFamily::OpenAiCompatible,
    };
    let provider_id = match family {
        ProviderFamily::VolcengineSeedance => "volcengine-seedance",
        _ => "openai-compatible",
    };

    let n = cfg.providers.iter().filter(|p| p.family == family).count() + 1;
    let family_prefix = match family {
        ProviderFamily::VolcengineSeedance => "volcengine",
        ProviderFamily::GeminiCompatible => "gemini",
        ProviderFamily::DeepSeek => "deepseek",
        _ => "openai",
    };
    let id = format!("{family_prefix}-{n}");

    let mut models = ProviderModelConfig::default();
    if let Some(m) = gen_models.first() {
        models.generation_model = Some(m.clone());
    }
    if let Some(m) = ana_models.first() {
        models.analysis_model = Some(m.clone());
    }
    models.generation_model_options = gen_models.clone();
    models.analysis_model_options = ana_models.clone();

    let mut inst = ProviderInstance {
        id: id.clone(),
        name: if params.name.is_empty() {
            "Provider".into()
        } else {
            params.name
        },
        provider_id: provider_id.into(),
        family,
        scopes: scopes.clone(),
        show_in_main_ui: true,
        models,
        endpoint: Some(params.endpoint.trim().to_string()),
        secret_ref: Some(artait_config::secret_store::ref_key(&id, "api_key")),
        api_key: None,
        extra: serde_json::Value::Null,
    };

    provider_helpers::apply_model_options(&mut inst, gen_models, ana_models);
    provider_helpers::normalize_provider_for_scopes(&mut inst);
    provider_helpers::apply_provider_api_style(&mut inst, &params.api_style);
    provider_helpers::set_provider_secret(&mut inst, params.api_key.trim());

    cfg.providers.push(inst.clone());
    provider_helpers::fix_provider_defaults(cfg);

    Ok(ProviderOpResult::Created {
        instance: inst,
        should_test: true,
    })
}

/// 编辑已有 Provider 实例。
pub fn edit_provider(
    cfg: &mut AppConfig,
    edit_id: &str,
    params: EditProviderParams,
) -> Result<ProviderOpResult, String> {
    validate_endpoint(&params.endpoint)?;
    if params.api_key.trim().is_empty() {
        return Err("API Key 不能为空".into());
    }

    let scopes = provider_helpers::scopes_from_node_kind(&params.node_kind);
    let gen_models = if scopes.contains(&ProviderScope::Generation) {
        provider_helpers::parse_model_options(params.generation_model.as_deref().unwrap_or(""))
    } else {
        Vec::new()
    };
    let ana_models = if scopes.contains(&ProviderScope::Analysis) {
        provider_helpers::parse_model_options(params.analysis_model.as_deref().unwrap_or(""))
    } else {
        Vec::new()
    };

    let inst = cfg
        .providers
        .iter_mut()
        .find(|p| p.id == edit_id)
        .ok_or_else(|| "未找到要编辑的 Provider 实例".to_string())?;

    if !params.name.is_empty() {
        inst.name = params.name.clone();
    }
    inst.endpoint = Some(params.endpoint.trim().to_string());
    inst.scopes = scopes;
    provider_helpers::apply_model_options(inst, gen_models, ana_models);
    provider_helpers::normalize_provider_for_scopes(inst);
    provider_helpers::apply_provider_api_style(inst, &params.api_style);
    provider_helpers::set_provider_secret(inst, params.api_key.trim());

    let display_name = inst.name.clone();
    provider_helpers::fix_provider_defaults(cfg);

    Ok(ProviderOpResult::Updated { display_name })
}
