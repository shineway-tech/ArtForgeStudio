//! Provider Registry 装配 + AppState ↔ Registry 绑定。

use std::sync::Arc;

use artait_model::{
    AppConfig, ProviderFamily, ProviderInstance, ProviderModelConfig, ProviderScope,
};
use artait_provider::{HttpClient, ProviderRegistry, ReqwestClient};
use artait_providers::{
    is_gpt_image_2_model, MemefastSeedanceProvider, MockProvider, OpenAiCompatibleProvider,
    VolcengineSeedanceProvider,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::ui::{
    AppShell, AppState, ProviderInfo, ProviderInstanceView, ProviderModelOption,
    WorkspaceSelectOption,
};

/// 全局共享 HTTP client（reqwest 池复用）。
pub fn shared_http() -> Arc<dyn HttpClient> {
    Arc::new(ReqwestClient::new())
}

/// 启动时构建 Registry：注册所有内置协议族实现。
pub fn build_registry() -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();
    for result in [
        reg.register(Arc::new(MockProvider::default())),
        reg.register(Arc::new(OpenAiCompatibleProvider::default())),
        reg.register(Arc::new(VolcengineSeedanceProvider::new())),
        reg.register(Arc::new(MemefastSeedanceProvider::new())),
    ] {
        if let artait_provider::RegisterResult::Replaced = result {
            tracing::warn!("ProviderRegistry: 同名 provider 被覆盖，检查编译期注册是否重复");
        }
    }
    reg
}

pub fn push_providers(app: &AppShell, cfg: &AppConfig) {
    let state = app.global::<AppState>();
    state.set_providers(build_provider_view_model(cfg));
    state.set_generation(provider_info(
        cfg,
        cfg.provider_defaults.generation.as_deref(),
        ProviderScope::Generation,
    ));
    state.set_analysis(provider_info(
        cfg,
        cfg.provider_defaults.analysis.as_deref(),
        ProviderScope::Analysis,
    ));
    push_workspace_provider_options(&state, cfg);
}

fn push_workspace_provider_options(state: &AppState, cfg: &AppConfig) {
    let generation_provider = active_provider(
        cfg,
        cfg.provider_defaults.generation.as_deref(),
        ProviderScope::Generation,
    );
    let generation_provider_id = generation_provider.map(|p| p.id.as_str()).unwrap_or("");
    let generation_model = generation_provider
        .and_then(|p| p.models.generation_model.as_deref())
        .unwrap_or("");

    state.set_ws_generation_provider_id(generation_provider_id.into());
    state.set_ws_generation_model(generation_model.into());
    state.set_ws_generation_provider_options(ModelRc::new(VecModel::from(build_provider_options(
        cfg,
        ProviderScope::Generation,
    ))));
    state.set_ws_generation_model_options(ModelRc::new(VecModel::from(build_model_options(
        generation_provider,
        ProviderScope::Generation,
    ))));

    let analysis_provider = active_provider(
        cfg,
        cfg.provider_defaults.analysis.as_deref(),
        ProviderScope::Analysis,
    );
    let analysis_provider_id = analysis_provider.map(|p| p.id.as_str()).unwrap_or("");
    let analysis_model = analysis_provider
        .and_then(|p| p.models.analysis_model.as_deref())
        .unwrap_or("");

    state.set_ws_analysis_provider_id(analysis_provider_id.into());
    state.set_ws_analysis_model(analysis_model.into());
    state.set_ws_analysis_provider_options(ModelRc::new(VecModel::from(build_provider_options(
        cfg,
        ProviderScope::Analysis,
    ))));
    state.set_ws_analysis_model_options(ModelRc::new(VecModel::from(build_model_options(
        analysis_provider,
        ProviderScope::Analysis,
    ))));

    let generation_api_style = generation_provider.map(api_style_id).unwrap_or("auto");
    let quality_options =
        quality_options_for_model_and_style(generation_model, generation_api_style);
    let quality_ids: Vec<String> = quality_options.iter().map(|o| o.id.to_string()).collect();
    let current_quality = state.get_ws_quality().to_string();
    state.set_ws_quality_options(ModelRc::new(VecModel::from(quality_options)));
    let selected_quality = if quality_ids.iter().any(|id| id == &current_quality) {
        current_quality
    } else {
        quality_ids.first().cloned().unwrap_or_else(|| "1K".into())
    };
    state.set_ws_quality(selected_quality.clone().into());

    let aspect_options = aspect_options_for_model_and_quality(
        generation_model,
        generation_api_style,
        &selected_quality,
    );
    let aspect_ids: Vec<String> = aspect_options.iter().map(|o| o.id.to_string()).collect();
    let current_aspect = state.get_ws_aspect().to_string();
    state.set_ws_aspect_options(ModelRc::new(VecModel::from(aspect_options)));
    if !aspect_ids.iter().any(|id| id == &current_aspect) {
        state.set_ws_aspect(
            aspect_ids
                .first()
                .cloned()
                .unwrap_or_else(|| "1:1".into())
                .into(),
        );
    }
}

pub fn select_workspace_quality(
    state: &AppState,
    generation_model: &str,
    api_style: &str,
    quality: &str,
) {
    state.set_ws_quality(quality.into());
    let aspect_options = aspect_options_for_model_and_quality(generation_model, api_style, quality);
    let aspect_ids: Vec<String> = aspect_options.iter().map(|o| o.id.to_string()).collect();
    let current_aspect = state.get_ws_aspect().to_string();
    state.set_ws_aspect_options(ModelRc::new(VecModel::from(aspect_options)));
    if !aspect_ids.iter().any(|id| id == &current_aspect) {
        state.set_ws_aspect(
            aspect_ids
                .first()
                .cloned()
                .unwrap_or_else(|| "1:1".into())
                .into(),
        );
    }
}

fn active_provider<'a>(
    cfg: &'a AppConfig,
    default_id: Option<&str>,
    scope: ProviderScope,
) -> Option<&'a ProviderInstance> {
    default_id
        .and_then(|id| {
            cfg.providers
                .iter()
                .find(|p| p.id == id && p.show_in_main_ui && p.scopes.contains(&scope))
        })
        .or_else(|| {
            cfg.providers
                .iter()
                .find(|p| p.show_in_main_ui && p.scopes.contains(&scope))
        })
}

fn build_provider_options(cfg: &AppConfig, scope: ProviderScope) -> Vec<WorkspaceSelectOption> {
    cfg.providers
        .iter()
        .filter(|p| p.show_in_main_ui && p.scopes.contains(&scope))
        .map(|p| {
            let model = match scope {
                ProviderScope::Generation => p.models.generation_model.as_deref(),
                ProviderScope::Analysis => p.models.analysis_model.as_deref(),
                ProviderScope::Video => p.models.video_model.as_deref(),
            };
            select_option(&p.id, &p.name, model.unwrap_or("未设"))
        })
        .collect()
}

fn build_model_options(
    provider: Option<&ProviderInstance>,
    scope: ProviderScope,
) -> Vec<ProviderModelOption> {
    let Some(provider) = provider else {
        return Vec::new();
    };
    let (selected, options) = match scope {
        ProviderScope::Generation => (
            provider.models.generation_model.as_deref(),
            &provider.models.generation_model_options,
        ),
        ProviderScope::Analysis => (
            provider.models.analysis_model.as_deref(),
            &provider.models.analysis_model_options,
        ),
        ProviderScope::Video => (
            provider.models.video_model.as_deref(),
            &provider.models.video_model_options,
        ),
    };
    options
        .iter()
        .filter(|model| !model.trim().is_empty())
        .map(|model| ProviderModelOption {
            provider_id: provider.id.clone().into(),
            model: model.clone().into(),
            label: model.clone().into(),
            hint: if selected == Some(model.as_str()) {
                "当前".into()
            } else {
                "".into()
            },
        })
        .collect()
}

fn quality_options_for_model_and_style(
    model: &str,
    _api_style: &str,
) -> Vec<WorkspaceSelectOption> {
    let model = model.to_ascii_lowercase();
    if is_gemini31_flash_image_model(&model) || is_gemini3_pro_image_model(&model) {
        let mut options = Vec::new();
        if is_gemini31_flash_image_model(&model) {
            options.push(select_option("512", "512", "0.5K"));
        }
        options.extend([
            select_option("1K", "1K", "1024+"),
            select_option("2K", "2K", "2048+"),
            select_option("4K", "4K", "4096+"),
        ]);
        options
    } else if is_gemini25_flash_image_model(&model) {
        vec![select_option("1K", "1K", "固定像素")]
    } else if model.contains("gemini") || model.contains("nano-banana") || model.contains("imagen")
    {
        vec![
            select_option("1K", "1K", "标准"),
            select_option("2K", "2K", "高清"),
            select_option("4K", "4K", "超清"),
        ]
    } else if is_gpt_image_2_model(&model) {
        vec![
            select_option("1K", "1K", "基础"),
            select_option("2K", "2K", "完整"),
            select_option("4K", "4K", "宽高"),
        ]
    } else if model.contains("dall-e-3") {
        vec![
            select_option("1K", "标准", "standard"),
            select_option("2K", "高清", "hd"),
        ]
    } else if model.contains("dall-e-2") {
        vec![select_option("1K", "1K", "固定")]
    } else if model.contains("gpt-image") {
        vec![
            select_option("1K", "1K", "低质量"),
            select_option("2K", "2K", "中质量"),
            select_option("4K", "4K", "高质量"),
        ]
    } else {
        vec![
            select_option("1K", "1K", "标准"),
            select_option("2K", "2K", "高清"),
            select_option("4K", "4K", "超清"),
        ]
    }
}

fn aspect_options_for_model_and_quality(
    model: &str,
    api_style: &str,
    quality: &str,
) -> Vec<WorkspaceSelectOption> {
    let model = model.to_ascii_lowercase();
    if is_gpt_image_2_model(&model) {
        gpt_image_2_aspect_options(api_style, quality)
    } else if is_gemini31_flash_image_model(&model) {
        vec![
            select_option("1:1", "1:1", "方图"),
            select_option("2:3", "2:3", "竖构图"),
            select_option("3:2", "3:2", "横构图"),
            select_option("3:4", "3:4", "竖图"),
            select_option("4:3", "4:3", "横图"),
            select_option("4:5", "4:5", "竖海报"),
            select_option("5:4", "5:4", "横海报"),
            select_option("9:16", "9:16", "竖屏"),
            select_option("16:9", "16:9", "横屏"),
            select_option("21:9", "21:9", "宽屏"),
            select_option("1:4", "1:4", "长竖"),
            select_option("4:1", "4:1", "长横"),
            select_option("1:8", "1:8", "极竖"),
            select_option("8:1", "8:1", "极横"),
        ]
    } else if model.contains("nano-banana") {
        [
            "1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "5:4", "4:5", "21:9", "9:21", "2:1",
            "1:2", "3:1", "1:3",
        ]
        .into_iter()
        .map(|aspect| image_option(aspect, aspect, nano_banana_pixel_size(quality, aspect)))
        .collect()
    } else if is_gemini3_pro_image_model(&model) || is_gemini25_flash_image_model(&model) {
        vec![
            select_option("1:1", "1:1", "方图"),
            select_option("2:3", "2:3", "竖构图"),
            select_option("3:2", "3:2", "横构图"),
            select_option("3:4", "3:4", "竖图"),
            select_option("4:3", "4:3", "横图"),
            select_option("4:5", "4:5", "竖海报"),
            select_option("5:4", "5:4", "横海报"),
            select_option("9:16", "9:16", "竖屏"),
            select_option("16:9", "16:9", "横屏"),
            select_option("21:9", "21:9", "宽屏"),
        ]
    } else if model.contains("dall-e-3") {
        vec![
            select_option("1:1", "1:1", "方图"),
            select_option("16:9", "16:9", "横图"),
            select_option("9:16", "9:16", "竖图"),
        ]
    } else {
        vec![
            select_option("1:1", "1:1", "方图"),
            select_option("16:9", "16:9", "横图"),
            select_option("9:16", "9:16", "竖图"),
        ]
    }
}

fn gpt_image_2_aspect_options(api_style: &str, quality: &str) -> Vec<WorkspaceSelectOption> {
    let option = |aspect: &'static str| {
        let hint = if matches!(api_style, "cpa" | "toapis" | "to_apis" | "auto") {
            match quality {
                "1K" => format!("CPA size={aspect}"),
                "4K" => "CPA resolution=4K".to_string(),
                _ => "CPA resolution=2K".to_string(),
            }
        } else {
            gpt_image_2_pixel_size(quality, aspect).to_string()
        };
        WorkspaceSelectOption {
            id: aspect.into(),
            label: aspect.into(),
            hint: hint.into(),
        }
    };
    match quality {
        "1K" => vec![option("1:1"), option("3:2"), option("2:3")],
        "4K" => vec![
            option("16:9"),
            option("9:16"),
            option("2:1"),
            option("1:2"),
            option("21:9"),
            option("9:21"),
        ],
        _ => vec![
            option("1:1"),
            option("3:2"),
            option("2:3"),
            option("4:3"),
            option("3:4"),
            option("5:4"),
            option("4:5"),
            option("16:9"),
            option("9:16"),
            option("2:1"),
            option("1:2"),
            option("21:9"),
            option("9:21"),
        ],
    }
}

fn gpt_image_2_pixel_size(quality: &str, aspect: &str) -> &'static str {
    match (quality, aspect) {
        ("1K", "3:2") => "1536x1024",
        ("1K", "2:3") => "1024x1536",
        ("1K", _) => "1024x1024",
        ("4K", "9:16") => "2160x3840",
        ("4K", "2:1") => "3840x1920",
        ("4K", "1:2") => "1920x3840",
        ("4K", "21:9") => "3840x1648",
        ("4K", "9:21") => "1648x3840",
        ("4K", _) => "3840x2160",
        (_, "3:2") => "2048x1360",
        (_, "2:3") => "1360x2048",
        (_, "4:3") => "2048x1536",
        (_, "3:4") => "1536x2048",
        (_, "5:4") => "2560x2048",
        (_, "4:5") => "2048x2560",
        (_, "16:9") => "2048x1152",
        (_, "9:16") => "1152x2048",
        (_, "2:1") => "2688x1344",
        (_, "1:2") => "1344x2688",
        (_, "21:9") => "2688x1152",
        (_, "9:21") => "1152x2688",
        _ => "2048x2048",
    }
}

fn nano_banana_pixel_size(quality: &str, aspect: &str) -> &'static str {
    match (quality, aspect) {
        ("1K", "16:9") => "1280x720",
        ("1K", "9:16") => "720x1280",
        ("1K", "4:3") => "1152x864",
        ("1K", "3:4") => "864x1152",
        ("1K", "3:2") => "1536x1024",
        ("1K", "2:3") => "1024x1536",
        ("1K", "5:4") => "1120x896",
        ("1K", "4:5") => "896x1120",
        ("1K", "21:9") => "1456x624",
        ("1K", "9:21") => "624x1456",
        ("1K", "1:3") => "688x2048",
        ("1K", "3:1") => "2048x688",
        ("1K", "2:1") => "1536x768",
        ("1K", "1:2") => "768x1536",
        ("1K", _) => "1024x1024",
        ("4K", "16:9") => "3840x2160",
        ("4K", "9:16") => "2160x3840",
        ("4K", "4:3") => "3264x2448",
        ("4K", "3:4") => "2448x3264",
        ("4K", "3:2") => "3504x2336",
        ("4K", "2:3") => "2336x3504",
        ("4K", "5:4") => "3200x2560",
        ("4K", "4:5") => "2560x3200",
        ("4K", "21:9") => "3840x1648",
        ("4K", "9:21") => "1648x3840",
        ("4K", "1:3") => "1280x3840",
        ("4K", "3:1") => "3840x1280",
        ("4K", "2:1") => "3840x1920",
        ("4K", "1:2") => "1920x3840",
        ("4K", _) => "2880x2880",
        (_, "16:9") => "2048x1152",
        (_, "9:16") => "1152x2048",
        (_, "4:3") => "2304x1728",
        (_, "3:4") => "1728x2304",
        (_, "3:2") => "2048x1360",
        (_, "2:3") => "1360x2048",
        (_, "5:4") => "2240x1792",
        (_, "4:5") => "1792x2240",
        (_, "21:9") => "2912x1248",
        (_, "9:21") => "1248x2912",
        (_, "1:3") => "688x2048",
        (_, "3:1") => "2048x688",
        (_, "2:1") => "3072x1536",
        (_, "1:2") => "1536x3072",
        _ => "2048x2048",
    }
}

fn is_gemini31_flash_image_model(model: &str) -> bool {
    model.contains("gemini-3.1-flash-image") || model.contains("nano-banana-2")
}

fn is_gemini3_pro_image_model(model: &str) -> bool {
    model.contains("gemini-3-pro-image") || model.contains("nano-banana-pro")
}

fn is_gemini25_flash_image_model(model: &str) -> bool {
    model.contains("gemini-2.5-flash-image")
        || (model.contains("nano-banana")
            && !model.contains("nano-banana-2")
            && !model.contains("nano-banana-pro"))
}

fn select_option(id: &str, label: &str, hint: &str) -> WorkspaceSelectOption {
    WorkspaceSelectOption {
        id: id.into(),
        label: label.into(),
        hint: hint.into(),
    }
}

fn image_option(id: &str, label: &str, size: &str) -> WorkspaceSelectOption {
    WorkspaceSelectOption {
        id: id.into(),
        label: label.into(),
        hint: size.into(),
    }
}

fn build_provider_view_model(cfg: &AppConfig) -> ModelRc<ProviderInstanceView> {
    let default_gen = cfg.provider_defaults.generation.as_deref();
    let default_ana = cfg.provider_defaults.analysis.as_deref();
    let items: Vec<ProviderInstanceView> = cfg
        .providers
        .iter()
        .map(|p| ProviderInstanceView {
            id: p.id.clone().into(),
            name: p.name.clone().into(),
            family: family_label(p.family).into(),
            scopes: scopes_label(&p.scopes).into(),
            endpoint: p.endpoint.clone().unwrap_or_default().into(),
            models: models_label(p).into(),
            api_style: api_style_label(p).into(),
            has_secret: provider_has_secret(p),
            show_in_main_ui: p.show_in_main_ui,
            has_generation_scope: p.scopes.contains(&ProviderScope::Generation),
            has_analysis_scope: p.scopes.contains(&ProviderScope::Analysis),
            is_default_generation: default_gen.is_some_and(|id| id == p.id),
            is_default_analysis: default_ana.is_some_and(|id| id == p.id),
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

pub fn provider_info(
    cfg: &AppConfig,
    instance_id: Option<&str>,
    scope: ProviderScope,
) -> ProviderInfo {
    instance_id
        .and_then(|id| cfg.providers.iter().find(|p| p.id == id))
        .map(|p| {
            let model = match scope {
                ProviderScope::Generation => p.models.generation_model.clone(),
                ProviderScope::Analysis => p.models.analysis_model.clone(),
                ProviderScope::Video => p.models.video_model.clone(),
            };
            ProviderInfo {
                has_provider: true,
                instance_name: p.name.clone().into(),
                model: model.unwrap_or_else(|| "—".into()).into(),
            }
        })
        .unwrap_or(ProviderInfo {
            has_provider: false,
            instance_name: SharedString::from("未配置"),
            model: SharedString::from("—"),
        })
}

fn family_label(f: ProviderFamily) -> &'static str {
    match f {
        ProviderFamily::OpenAiCompatible => "openai-compatible",
        ProviderFamily::GeminiCompatible => "gemini-compatible",
        ProviderFamily::WavespeedCompatible => "wavespeed-compatible",
        ProviderFamily::VolcengineSeedance => "volcengine-seedance",
        ProviderFamily::DeepSeek => "deepseek",
        ProviderFamily::Ikuncode => "ikuncode",
        ProviderFamily::Rembg => "rembg",
        ProviderFamily::PhotoRoom => "photoroom",
        ProviderFamily::Mock => "mock",
        ProviderFamily::Custom => "custom",
    }
}

fn scopes_label(scopes: &[ProviderScope]) -> String {
    if scopes.is_empty() {
        return "无范围".into();
    }
    scopes
        .iter()
        .map(|s| match s {
            ProviderScope::Generation => "生图",
            ProviderScope::Analysis => "推理",
            ProviderScope::Video => "视频",
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

fn models_label(p: &ProviderInstance) -> String {
    let gen_count = p.models.generation_model_options.len();
    let ana_count = p.models.analysis_model_options.len();
    let gen = p.models.generation_model.as_deref().unwrap_or("未配置");
    let ana = p.models.analysis_model.as_deref().unwrap_or("未配置");
    format!("生图 {gen_count} 个：{gen} · 推理 {ana_count} 个：{ana}")
}

fn api_style_label(p: &ProviderInstance) -> &'static str {
    match api_style_id(p) {
        "cpa" => "CPA",
        "sub2api" => "Sub2API",
        "newapi" => "NewAPI",
        "gemini" => "Gemini",
        "images" => "OpenAI Images",
        "responses" => "Responses",
        "messages" => "Messages",
        "chat" => "Chat",
        _ => "自动",
    }
}

fn api_style_id(p: &ProviderInstance) -> &'static str {
    match p
        .extra
        .get("api_style")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
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

/// 创建一个新的 Mock 实例并加入 cfg。
pub fn make_mock_instance(existing: &[ProviderInstance]) -> ProviderInstance {
    let n = existing
        .iter()
        .filter(|p| p.family == ProviderFamily::Mock)
        .count()
        + 1;
    let id = format!("mock-{n}");
    ProviderInstance {
        id: id.clone(),
        name: format!("Mock #{n}"),
        provider_id: "mock".into(),
        family: ProviderFamily::Mock,
        scopes: vec![ProviderScope::Generation, ProviderScope::Analysis],
        show_in_main_ui: true,
        models: ProviderModelConfig {
            generation_model: Some("mock-img-1".into()),
            generation_model_options: vec!["mock-img-1".into()],
            analysis_model: Some("mock-text-1".into()),
            analysis_model_options: vec!["mock-text-1".into()],
            ..Default::default()
        },
        endpoint: Some("mock://local".into()),
        secret_ref: None,
        api_key: None,
        extra: serde_json::Value::Object(serde_json::Map::new()),
    }
}

fn provider_has_secret(p: &ProviderInstance) -> bool {
    p.api_key
        .as_deref()
        .is_some_and(|secret| !secret.trim().is_empty())
        || p.secret_ref
            .as_deref()
            .and_then(|key| artait_config::secret_store::get(key).ok().flatten())
            .is_some_and(|secret| !secret.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(options: Vec<WorkspaceSelectOption>) -> Vec<String> {
        options.iter().map(|o| o.id.to_string()).collect()
    }

    fn quality_ids(model: &str) -> Vec<String> {
        ids(quality_options_for_model_and_style(model, "auto"))
    }

    fn aspect_ids(model: &str, quality: &str) -> Vec<String> {
        ids(aspect_options_for_model_and_quality(model, "auto", quality))
    }

    fn aspect_ids_for_style(model: &str, style: &str, quality: &str) -> Vec<String> {
        ids(aspect_options_for_model_and_quality(model, style, quality))
    }

    #[test]
    fn gemini_quality_options_match_model_family() {
        assert_eq!(
            quality_ids("gemini-3.1-flash-image"),
            ["512", "1K", "2K", "4K"]
        );
        assert_eq!(quality_ids("nano-banana-pro"), ["1K", "2K", "4K"]);
        assert_eq!(quality_ids("gemini-2.5-flash-image"), ["1K"]);
    }

    #[test]
    fn gemini_aspect_options_match_model_family() {
        assert!(aspect_ids("gemini-3.1-flash-image", "2K").contains(&"1:8".to_string()));
        assert!(!aspect_ids("gemini-3-pro-image", "2K").contains(&"1:8".to_string()));
        assert!(aspect_ids("nano-banana-pro-vt", "4K").contains(&"9:21".to_string()));
        assert!(aspect_ids("nano-banana-pro-vt", "4K").contains(&"1:3".to_string()));
        assert!(!aspect_ids("gemini-3-pro-image", "4K").contains(&"1:3".to_string()));
        assert!(aspect_ids("gemini-2.5-flash-image", "1K").contains(&"21:9".to_string()));
    }

    #[test]
    fn openai_image_options_stay_separate_from_gemini() {
        assert_eq!(quality_ids("gpt-image-1"), ["1K", "2K", "4K"]);
        assert_eq!(aspect_ids("gpt-image-1", "2K"), ["1:1", "16:9", "9:16"]);
        assert!(!aspect_ids("gpt-image-1", "2K").contains(&"21:9".to_string()));
        assert!(!aspect_ids("gpt-image-1", "2K").contains(&"2:3".to_string()));
    }

    #[test]
    fn gpt_image_2_aspect_options_follow_resolution() {
        assert_eq!(aspect_ids("gpt-image-2", "1K"), ["1:1", "3:2", "2:3"]);
        assert_eq!(
            aspect_ids("gpt-image-2", "4K"),
            ["16:9", "9:16", "2:1", "1:2", "21:9", "9:21"]
        );
        assert!(aspect_ids("gpt-image-2", "2K").contains(&"21:9".to_string()));
        assert!(aspect_ids("gpt-image-2", "2K").contains(&"4:5".to_string()));
        assert_eq!(
            aspect_ids_for_style("gpt-image-2", "newapi", "4K"),
            ["16:9", "9:16", "2:1", "1:2", "21:9", "9:21"]
        );
    }
}
