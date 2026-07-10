//! 生图任务构建、元数据持久化、模式配置映射。

use std::sync::Arc;

use artait_model::{CreationMode, DirectorControls, ImageUploadConfig, ProviderInstance};
use artait_task::SavedAsset;

use crate::task_history::TaskHistory;
use crate::TaskMeta;

// ── 任务元数据 ────────────────────────────────────────────────────────────

pub fn generation_task_meta(
    inst: &ProviderInstance,
    prompt: &str,
    output_dir: &std::path::Path,
    mode: CreationMode,
    aspect: &str,
    quality: &str,
    count: u32,
    upload_cfg: &ImageUploadConfig,
    request_metadata: &serde_json::Value,
) -> TaskMeta {
    let mut extra = serde_json::json!({
        "mode": mode.route_id(),
        "aspect": aspect,
        "quality": quality,
        "count": count,
        "request": request_metadata,
    });
    if let Some(ref url) = upload_cfg.api_url {
        extra["image_upload_api_url"] = serde_json::Value::String(url.clone());
    }
    if let Some(ref key) = upload_cfg.api_key {
        extra["image_upload_api_key"] = serde_json::Value::String(key.clone());
    }
    TaskMeta {
        output_path: output_dir.display().to_string(),
        provider_instance_id: inst.id.clone(),
        provider_id: inst.provider_id.clone(),
        model: inst.models.generation_model.clone().unwrap_or_default(),
        prompt: prompt.to_string(),
        provider_task_id: String::new(),
        endpoint: inst.endpoint.clone().unwrap_or_default(),
        extra_json: extra.to_string(),
        retry_source_url: String::new(),
    }
}

pub struct GeneratedMetadataInput<'a> {
    pub saved: &'a SavedAsset,
    pub inst: &'a ProviderInstance,
    pub prompt: &'a str,
    pub mode: CreationMode,
    pub aspect: &'a str,
    pub quality: &'a str,
    pub count: u32,
    pub image_index: u32,
    pub source_task_id: Option<&'a str>,
    pub template_file: Option<&'a str>,
    pub reference_count: usize,
    pub request_metadata: &'a serde_json::Value,
}

pub fn persist_generated_metadata(input: GeneratedMetadataInput<'_>) {
    let item = artait_asset::GeneratedAssetMetadata {
        path: input.saved.path.clone(),
        domain: input.mode.domain_str().to_string(),
        kind: "image".into(),
        mime: input.saved.mime.clone(),
        bytes: input.saved.bytes,
        width: None,
        height: None,
        source_task_id: input.source_task_id.map(str::to_owned),
        batch_id: None,
        prompt: input.prompt.to_string(),
        negative_prompt: None,
        mode: input.mode.route_id().to_string(),
        quality: Some(input.quality.to_string()),
        aspect_ratio: Some(input.aspect.to_string()),
        count: Some(input.count),
        image_index: Some(input.image_index),
        provider_instance_id: Some(input.inst.id.clone()),
        provider_id: Some(input.inst.provider_id.clone()),
        provider_name: Some(input.inst.name.clone()),
        model: input.inst.models.generation_model.clone(),
        endpoint: input.inst.endpoint.clone(),
        template_file: input.template_file.map(str::to_owned),
        reference_images_json: serde_json::json!({ "count": input.reference_count }).to_string(),
        provider_metadata_json: input.saved.provider_metadata.to_string(),
        request_metadata_json: input.request_metadata.to_string(),
    };
    match artait_asset::AssetMetadataStore::default() {
        Ok(store) => {
            if let Err(e) = store.upsert_generated(&item) {
                tracing::warn!(error = %e, path = %input.saved.path.display(), "写入图片元数据失败");
            }
        }
        Err(e) => tracing::warn!(error = %e, "打开图片元数据索引失败"),
    }
}

/// 从错误信息中提取最后一个 HTTP URL（用于 URL 保存失败时重试）
pub fn extract_url_from_error(error: &str) -> Option<String> {
    let patterns = ["from http://", "from https://"];
    let mut last_url: Option<String> = None;
    for pat in &patterns {
        if let Some(pos) = error.rfind(pat) {
            let start = pos + 5; // skip "from "
            let end = error[start..]
                .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
                .map(|e| start + e)
                .unwrap_or(error.len());
            last_url = Some(error[start..end].to_string());
        }
    }
    if last_url.is_none() {
        for pat in &["http://", "https://"] {
            if let Some(start) = error.rfind(pat) {
                let end = error[start..]
                    .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
                    .map(|e| start + e)
                    .unwrap_or(error.len());
                last_url = Some(error[start..end].to_string());
                break;
            }
        }
    }
    last_url
}

/// 更新历史中指定任务的完成状态
pub async fn update_history_completed(
    history: &Arc<tokio::sync::Mutex<TaskHistory>>,
    id: &str,
    output_path: &str,
) {
    let now = chrono::Utc::now().to_rfc3339();
    let mut hg = history.lock().await;
    if let Some(mut existing) = hg.get(id).cloned() {
        existing.status = "completed".to_string();
        existing.output_path = output_path.to_string();
        existing.finished_at = now;
        existing.error = String::new();
        existing.progress = 1.0;
        hg.upsert(existing);
    }
}

// ── 核心生图流程 ──────────────────────────────────────────────────────────

/// 执行一次图片生成：provider 调用 → 保存 → 元数据。供 TaskRunner closure 使用。
pub async fn run_image_generation(
    inst: &artait_model::ProviderInstance,
    prompt: &str,
    output_dir: &std::path::Path,
    file_prefix: &str,
    mode: CreationMode,
    aspect: &str,
    quality: &str,
    count: u32,
    image_index: u32,
    controls: &DirectorControls,
    refs: &[artait_model::ReferenceImage],
    registry: &artait_provider::ProviderRegistry,
    http: std::sync::Arc<dyn artait_provider::HttpClient>,
    ctx: &artait_task::TaskContext,
) -> Result<SavedAssetInfo, artait_task::TaskError> {
    use crate::provider_helpers::load_provider_secret;
    use artait_provider::ProviderContext;

    ctx.progress(0.05);

    let provider = registry.get(&inst.provider_id).ok_or_else(|| {
        artait_task::TaskError::Failed(format!("未找到 provider {}", inst.provider_id))
    })?;
    let generator = provider.as_image_generator().ok_or_else(|| {
        artait_task::TaskError::Failed(format!("{} 不支持图片生成", inst.provider_id))
    })?;
    let request_metadata = serde_json::json!({
        "director_controls": controls,
    });

    let mut pctx =
        ProviderContext::with_http(inst.id.clone(), inst.provider_id.clone(), http.clone());
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();
    pctx.output_path = output_dir.to_path_buf();
    pctx.cancellation = ctx.cancel.clone();
    pctx.secret = load_provider_secret(inst, Some(ctx))?;

    for reference in refs {
        ctx.info(format!("参考图 · {}", reference.local_path.display()));
        if !reference.local_path.is_file() {
            return Err(artait_task::TaskError::Failed(format!(
                "参考图不是有效文件: {}",
                reference.local_path.display()
            )));
        }
    }

    let req = artait_provider::request::ImageGenerationRequest {
        prompt: prompt.to_string(),
        negative_prompt: None,
        reference_images: refs.to_vec(),
        aspect_ratio: Some(aspect.to_string()),
        resolution: None,
        size: None,
        quality: Some(quality.to_string()),
        count: 1,
        mode,
        action_name: None,
        metadata: request_metadata.clone(),
    };

    ctx.info(format!(
        "调用生图 provider · 比例 {aspect} · 品质 {quality}"
    ));
    ctx.progress(0.2);

    let output = generator
        .generate(req, &pctx)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("provider: {e}")))?;

    ctx.info("provider 返回，保存中…");
    ctx.progress(0.7);

    let saver = artait_task::ResultSaver::new(
        output_dir.to_path_buf(),
        file_prefix.to_string(),
        http.clone(),
    );
    let saved = saver
        .save(output)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("保存失败: {e}")))?;

    persist_generated_metadata(GeneratedMetadataInput {
        saved: &saved,
        inst,
        prompt,
        mode,
        aspect,
        quality,
        count,
        image_index,
        source_task_id: Some(ctx.id.as_str()),
        template_file: None,
        reference_count: refs.len(),
        request_metadata: &request_metadata,
    });

    ctx.info(format!(
        "已保存 {} ({} bytes, {})",
        saved.path.display(),
        saved.bytes,
        saved.mime
    ));

    Ok(SavedAssetInfo {
        path: saved.path,
        bytes: saved.bytes,
        mime: saved.mime,
    })
}

/// 生图保存结果摘要。
pub struct SavedAssetInfo {
    pub path: std::path::PathBuf,
    pub bytes: u64,
    pub mime: String,
}

// ── 视频生成 ──────────────────────────────────────────────────────────────

/// 视频任务元数据。
pub fn video_task_meta(
    inst: &artait_model::ProviderInstance,
    prompt: &str,
    output_dir: &std::path::Path,
    resolution: &str,
    aspect: &str,
    duration_secs: u32,
    enable_audio: bool,
) -> TaskMeta {
    let extra = serde_json::json!({
        "mode": "video",
        "resolution": resolution,
        "aspect_ratio": aspect,
        "duration_secs": duration_secs,
        "enable_audio": enable_audio,
    });
    TaskMeta {
        output_path: output_dir.display().to_string(),
        provider_instance_id: inst.id.clone(),
        provider_id: inst.provider_id.clone(),
        model: inst.models.video_model.clone().unwrap_or_default(),
        prompt: prompt.to_string(),
        provider_task_id: String::new(),
        endpoint: inst.endpoint.clone().unwrap_or_default(),
        extra_json: extra.to_string(),
        retry_source_url: String::new(),
    }
}

/// 视频元数据持久化。
pub struct VideoMetadataInput<'a> {
    pub saved: &'a SavedAsset,
    pub inst: &'a artait_model::ProviderInstance,
    pub prompt: &'a str,
    pub resolution: &'a str,
    pub aspect: &'a str,
    pub duration_secs: u32,
    pub enable_audio: bool,
    pub camera_fixed: bool,
    pub source_task_id: Option<&'a str>,
    pub reference_count: usize,
}

pub fn persist_video_metadata(input: VideoMetadataInput<'_>) {
    let item = artait_asset::GeneratedAssetMetadata {
        path: input.saved.path.clone(),
        domain: "video".into(),
        kind: "video".into(),
        mime: input.saved.mime.clone(),
        bytes: input.saved.bytes,
        width: None,
        height: None,
        source_task_id: input.source_task_id.map(str::to_owned),
        batch_id: None,
        prompt: input.prompt.to_string(),
        negative_prompt: None,
        mode: "video".to_string(),
        quality: Some(input.resolution.to_string()),
        aspect_ratio: Some(input.aspect.to_string()),
        count: Some(1),
        image_index: None,
        provider_instance_id: Some(input.inst.id.clone()),
        provider_id: Some(input.inst.provider_id.clone()),
        provider_name: Some(input.inst.name.clone()),
        model: input.inst.models.video_model.clone(),
        endpoint: input.inst.endpoint.clone(),
        template_file: None,
        reference_images_json: serde_json::json!({ "count": input.reference_count }).to_string(),
        provider_metadata_json: input.saved.provider_metadata.to_string(),
        request_metadata_json: serde_json::json!({
            "duration_secs": input.duration_secs,
            "enable_audio": input.enable_audio,
            "camera_fixed": input.camera_fixed,
        })
        .to_string(),
    };
    match artait_asset::AssetMetadataStore::default() {
        Ok(store) => {
            if let Err(e) = store.upsert_generated(&item) {
                tracing::warn!(error = %e, path = %input.saved.path.display(), "写入视频元数据失败");
            }
        }
        Err(e) => tracing::warn!(error = %e, "打开视频元数据索引失败"),
    }
}

/// 执行一次视频生成：provider 调用 → 保存 → 元数据。供 TaskRunner closure 使用。
///
/// `seedance_params` 包含完整的 Seedance 视频参数（模型、分辨率、时长、音频、多模态引用等）。
pub async fn run_video_generation(
    inst: &artait_model::ProviderInstance,
    prompt: &str,
    output_dir: &std::path::Path,
    file_prefix: &str,
    seedance_params: artait_model::seedance::SeedanceVideoParams,
    registry: &artait_provider::ProviderRegistry,
    http: std::sync::Arc<dyn artait_provider::HttpClient>,
    ctx: &artait_task::TaskContext,
) -> Result<SavedAssetInfo, artait_task::TaskError> {
    use crate::provider_helpers::load_provider_secret;
    use artait_provider::ProviderContext;

    ctx.progress(0.05);

    let provider = registry.get(&inst.provider_id).ok_or_else(|| {
        artait_task::TaskError::Failed(format!("未找到 provider {}", inst.provider_id))
    })?;
    let generator = provider.as_video_generator().ok_or_else(|| {
        artait_task::TaskError::Failed(format!("{} 不支持视频生成", inst.provider_id))
    })?;

    let mut pctx =
        ProviderContext::with_http(inst.id.clone(), inst.provider_id.clone(), http.clone());
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();
    pctx.cancellation = ctx.cancel.clone();
    pctx.secret = load_provider_secret(inst, Some(ctx))?;

    // 构建 VideoGenerationRequest
    let req = artait_provider::request::VideoGenerationRequest {
        prompt: seedance_params.prompt.clone(),
        image: None,
        duration: Some(seedance_params.duration_secs as f32),
        aspect_ratio: Some(seedance_params.aspect_ratio.clone()),
        resolution: None,
        generate_audio: seedance_params.enable_audio,
        metadata: serde_json::Value::Null,
        seedance_params: Some(seedance_params.clone()),
    };

    ctx.info(format!(
        "调用视频 provider · 比例 {} · 分辨率 {} · 时长 {}s",
        req.aspect_ratio.as_deref().unwrap_or("?"),
        seedance_params.resolution,
        seedance_params.duration_secs,
    ));
    ctx.progress(0.2);

    let output = generator
        .generate_video(req, &pctx)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("provider: {e}")))?;

    ctx.info("provider 返回，保存中…");
    ctx.progress(0.7);

    let saver = artait_task::ResultSaver::new(
        output_dir.to_path_buf(),
        file_prefix.to_string(),
        http.clone(),
    );
    let saved = saver
        .save(output.kind)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("保存失败: {e}")))?;

    let ref_count = seedance_params.references.len();
    persist_video_metadata(VideoMetadataInput {
        saved: &saved,
        inst,
        prompt,
        resolution: &seedance_params.resolution,
        aspect: &seedance_params.aspect_ratio,
        duration_secs: seedance_params.duration_secs,
        enable_audio: seedance_params.enable_audio,
        camera_fixed: seedance_params.camera_fixed,
        source_task_id: Some(ctx.id.as_str()),
        reference_count: ref_count,
    });

    ctx.info(format!(
        "已保存视频 {} ({} bytes, {})",
        saved.path.display(),
        saved.bytes,
        saved.mime
    ));

    Ok(SavedAssetInfo {
        path: saved.path,
        bytes: saved.bytes,
        mime: saved.mime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_scene() {
        assert_eq!(CreationMode::from_route("scene"), CreationMode::Scene);
    }

    #[test]
    fn parse_mode_character() {
        assert_eq!(
            CreationMode::from_route("character"),
            CreationMode::Character
        );
    }

    #[test]
    fn parse_mode_unknown_defaults_to_scene() {
        assert_eq!(CreationMode::from_route("unknown"), CreationMode::Scene);
    }

    #[test]
    fn output_subdir_maps_correctly() {
        assert_eq!(CreationMode::Scene.output_subdir(), "scenes");
        assert_eq!(CreationMode::Character.output_subdir(), "creations");
        assert_eq!(CreationMode::Effect.output_subdir(), "effects");
        assert_eq!(CreationMode::Storyboard.output_subdir(), "storyboards");
        assert_eq!(CreationMode::ActionSequence.output_subdir(), "batch");
    }

    #[test]
    fn display_mode_labels() {
        assert_eq!(CreationMode::Scene.display_name(), "创建场景");
        assert_eq!(CreationMode::Character.display_name(), "创建角色");
        assert_eq!(CreationMode::ActionSequence.display_name(), "动作序列");
    }

    #[test]
    fn extract_url_http() {
        let err = "download failed from http://example.com/image.png after retry";
        let url = extract_url_from_error(err);
        assert_eq!(url, Some("http://example.com/image.png".into()));
    }

    #[test]
    fn extract_url_https() {
        let err = "error from https://cdn.example.com/output.webp connection";
        let url = extract_url_from_error(err);
        assert_eq!(url, Some("https://cdn.example.com/output.webp".into()));
    }

    #[test]
    fn extract_url_none() {
        assert_eq!(extract_url_from_error("no url here"), None);
    }

    #[test]
    fn generation_task_meta_builds_correctly() {
        let inst = artait_model::ProviderInstance {
            id: "test-1".into(),
            name: "test".into(),
            provider_id: "openai-compatible".into(),
            family: artait_model::ProviderFamily::OpenAiCompatible,
            scopes: vec![],
            show_in_main_ui: false,
            models: artait_model::ProviderModelConfig {
                generation_model: Some("gpt-image-1".into()),
                ..Default::default()
            },
            endpoint: Some("https://api.test.com".into()),
            secret_ref: None,
            api_key: None,
            extra: serde_json::Value::Null,
        };

        let meta = generation_task_meta(
            &inst,
            "a cat",
            std::path::Path::new("/tmp/out"),
            CreationMode::Scene,
            "16:9",
            "2K",
            1,
            &Default::default(),
            &serde_json::Value::Null,
        );

        assert_eq!(meta.provider_instance_id, "test-1");
        assert_eq!(meta.provider_id, "openai-compatible");
        assert_eq!(meta.model, "gpt-image-1");
        assert_eq!(meta.prompt, "a cat");
        assert!(meta.extra_json.contains("16:9"));
    }
}
