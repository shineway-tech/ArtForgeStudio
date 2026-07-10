//! 角色图像生成服务。
//!
//! 整合角色 Prompt 构建、Provider 调用、图片保存和角色存储更新。
//! 供 UI callback handler 和 TaskRunner 使用。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use artait_model::{
    Character, CharacterView, CreationMode, ProviderInstance, ReferenceImage, ViewType,
};
use artait_provider::ProviderContext;
use artait_task::{ResultSaver, TaskContext};

use crate::character_prompt::{
    build_character_sheet_prompt, build_variation_prompt, CharacterPromptConfig,
};
use crate::generation::persist_generated_metadata;
use crate::provider_helpers::load_provider_secret;

// ============================================================================
// 角色生成请求
// ============================================================================

/// 角色图像生成请求。
pub struct CharacterGenerationRequest {
    /// 要生成图像的角色
    pub character: Character,
    /// 生成配置
    pub prompt_config: CharacterPromptConfig,
    /// 要生成的视图类型（空 = 生成角色表，非空 = 生成特定视角）
    pub views: Vec<ViewType>,
    /// 参考图片
    pub reference_images: Vec<ReferenceImage>,
    /// 输出目录
    pub output_dir: PathBuf,
    /// 文件前缀
    pub file_prefix: String,
}

/// 角色变体生成请求（衣柜）。
pub struct VariationGenerationRequest {
    /// 所属角色
    pub character: Character,
    /// 变体索引（variations 列表中的位置）
    pub variation_index: usize,
    /// 生成配置
    pub prompt_config: CharacterPromptConfig,
    /// 输出目录
    pub output_dir: PathBuf,
    /// 文件前缀
    pub file_prefix: String,
}

// ============================================================================
// 主生成函数
// ============================================================================

/// 执行一次角色图像生成（通过 TaskRunner 调用的版本）。
///
/// 支持角色表生成和单视图生成。
/// 返回保存的图片路径。
pub async fn run_character_generation(
    inst: &ProviderInstance,
    request: CharacterGenerationRequest,
    registry: &artait_provider::ProviderRegistry,
    http: Arc<dyn artait_provider::HttpClient>,
    ctx: &TaskContext,
) -> std::result::Result<SavedImageInfo, artait_task::TaskError> {
    ctx.progress(0.05);
    ctx.check_cancelled()
        .map_err(|_| artait_task::TaskError::Cancelled)?;

    let provider = registry.get(&inst.provider_id).ok_or_else(|| {
        artait_task::TaskError::Failed(format!("未找到 provider {}", inst.provider_id))
    })?;

    // 优先使用 CharacterGenerator trait，回退到 ImageGenerator
    let _generator = provider.as_character_generator().or_else(|| {
        // 回退：使用 ImageGenerator
        None
    });

    // 构建 Prompt
    ctx.info("构建角色提示词…");
    let sheet = build_character_sheet_prompt(&request.character, &request.prompt_config);

    let ref_count = request.reference_images.len();

    let mut pctx =
        ProviderContext::with_http(inst.id.clone(), inst.provider_id.clone(), http.clone());
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();
    pctx.cancellation = ctx.cancel.clone();
    pctx.secret = load_provider_secret(inst, Some(ctx))?;

    // 当前通过 ImageGenerator 生成（CharacterGenerator trait 等待实现）
    let image_gen = provider.as_image_generator().ok_or_else(|| {
        artait_task::TaskError::Failed(format!("{} 不支持图片生成", inst.provider_id))
    })?;

    let req = artait_provider::request::ImageGenerationRequest {
        prompt: sheet.positive.clone(),
        negative_prompt: Some(sheet.negative.clone()),
        reference_images: request.reference_images,
        aspect_ratio: Some("1:1".into()),
        resolution: None,
        size: None,
        quality: Some("2K".into()),
        count: 1,
        mode: CreationMode::Character,
        action_name: None,
        metadata: serde_json::Value::Null,
    };

    ctx.info(format!(
        "调用生图 provider · prompt长度={}",
        sheet.positive.len()
    ));
    ctx.progress(0.3);

    let output = image_gen
        .generate(req, &pctx)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("provider: {e}")))?;

    ctx.info("provider 返回，保存中…");
    ctx.progress(0.7);

    let saver = ResultSaver::new(
        request.output_dir.clone(),
        request.file_prefix.clone(),
        http.clone(),
    );
    let saved = saver
        .save(output)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("保存失败: {e}")))?;

    persist_generated_metadata(crate::generation::GeneratedMetadataInput {
        saved: &saved,
        inst,
        prompt: &sheet.positive,
        mode: CreationMode::Character,
        aspect: "1:1",
        quality: "2K",
        count: 1,
        image_index: 0,
        source_task_id: Some(ctx.id.as_str()),
        template_file: None,
        reference_count: ref_count,
        request_metadata: &serde_json::Value::Null,
    });

    ctx.info(format!(
        "已保存 {} ({} bytes, {})",
        saved.path.display(),
        saved.bytes,
        saved.mime
    ));

    Ok(SavedImageInfo {
        path: saved.path,
        bytes: saved.bytes,
        mime: saved.mime,
    })
}

/// 生成角色变体图像（衣柜）。
pub async fn run_variation_generation(
    inst: &ProviderInstance,
    request: VariationGenerationRequest,
    registry: &artait_provider::ProviderRegistry,
    http: Arc<dyn artait_provider::HttpClient>,
    ctx: &TaskContext,
) -> std::result::Result<SavedImageInfo, artait_task::TaskError> {
    ctx.progress(0.05);
    ctx.check_cancelled()
        .map_err(|_| artait_task::TaskError::Cancelled)?;

    let provider = registry.get(&inst.provider_id).ok_or_else(|| {
        artait_task::TaskError::Failed(format!("未找到 provider {}", inst.provider_id))
    })?;

    let image_gen = provider.as_image_generator().ok_or_else(|| {
        artait_task::TaskError::Failed(format!("{} 不支持图片生成", inst.provider_id))
    })?;

    let variation = request
        .character
        .variations
        .get(request.variation_index)
        .ok_or_else(|| artait_task::TaskError::Failed("变体索引越界".into()))?;

    let has_clothing_refs = !variation.clothing_reference_images.is_empty();
    let sheet = build_variation_prompt(
        &request.character,
        variation,
        has_clothing_refs,
        &request.prompt_config,
    );

    let mut pctx =
        ProviderContext::with_http(inst.id.clone(), inst.provider_id.clone(), http.clone());
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();
    pctx.cancellation = ctx.cancel.clone();
    pctx.secret = load_provider_secret(inst, Some(ctx))?;

    let req = artait_provider::request::ImageGenerationRequest {
        prompt: sheet.positive.clone(),
        negative_prompt: Some(sheet.negative.clone()),
        reference_images: vec![], // 服装参考图由 prompt 中的融合指令处理
        aspect_ratio: Some("1:1".into()),
        resolution: None,
        size: None,
        quality: Some("2K".into()),
        count: 1,
        mode: CreationMode::Character,
        action_name: Some(format!("变体:{}", variation.name)),
        metadata: serde_json::Value::Null,
    };

    ctx.info(format!(
        "生成变体「{}」· prompt长度={}",
        variation.name,
        sheet.positive.len()
    ));
    ctx.progress(0.3);

    let output = image_gen
        .generate(req, &pctx)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("provider: {e}")))?;

    ctx.progress(0.7);

    let saver = ResultSaver::new(
        request.output_dir.clone(),
        request.file_prefix.clone(),
        http.clone(),
    );
    let saved = saver
        .save(output)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("保存失败: {e}")))?;

    ctx.info(format!("变体已保存 {}", saved.path.display()));

    Ok(SavedImageInfo {
        path: saved.path,
        bytes: saved.bytes,
        mime: saved.mime,
    })
}

// ============================================================================
// 生成结果
// ============================================================================

/// 图片保存结果摘要。
#[derive(Debug, Clone)]
pub struct SavedImageInfo {
    pub path: PathBuf,
    pub bytes: u64,
    pub mime: String,
}

// ============================================================================
// 便捷函数（供 UI callback handler 非 TaskRunner 场景使用）
// ============================================================================

/// 生成角色表图片并更新角色存储。
///
/// 此函数在 TaskRunner 外调用（由 UI 直接触发），
/// 完成 prompt 构建 → provider 调用 → 保存 → 更新 store 的全流程。
pub async fn generate_and_update_character(
    inst: &ProviderInstance,
    character: &mut Character,
    config: &CharacterPromptConfig,
    output_dir: &Path,
    registry: &artait_provider::ProviderRegistry,
    http: Arc<dyn artait_provider::HttpClient>,
) -> Result<PathBuf> {
    let provider = registry.get(&inst.provider_id).context("未找到 provider")?;

    let image_gen = provider
        .as_image_generator()
        .context("provider 不支持图片生成")?;

    // 构建 prompt
    let sheet = build_character_sheet_prompt(character, config);

    let mut pctx =
        ProviderContext::with_http(inst.id.clone(), inst.provider_id.clone(), http.clone());
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();

    let req = artait_provider::request::ImageGenerationRequest {
        prompt: sheet.positive.clone(),
        negative_prompt: Some(sheet.negative.clone()),
        reference_images: vec![],
        aspect_ratio: Some("1:1".into()),
        resolution: None,
        size: None,
        quality: Some("2K".into()),
        count: 1,
        mode: CreationMode::Character,
        action_name: None,
        metadata: serde_json::Value::Null,
    };

    let output = image_gen
        .generate(req, &pctx)
        .await
        .map_err(|e| anyhow::anyhow!("生成失败: {e}"))?;

    // 保存图片
    let saver = ResultSaver::new(
        output_dir.to_path_buf(),
        format!("char_{}", character.id),
        http.clone(),
    );
    let saved = saver.save(output).await.context("保存图片失败")?;

    // 添加到角色视图
    let view = CharacterView {
        view_type: ViewType::Front,
        image_url: saved.path.display().to_string(),
        generated_at: chrono::Utc::now(),
    };
    character.views.push(view.clone());
    if character.thumbnail_url.is_none() {
        character.thumbnail_url = Some(view.image_url.clone());
    }

    Ok(saved.path)
}
