//! 提示词模板：读写、扫描、分类、生成拼接 + 提示词优化。

use std::path::{Path, PathBuf};

use artait_model::AppConfig;
use artait_model::CreationMode;

// ── 数据结构 ──────────────────────────────────────────────────────────────

pub struct PromptTemplateDraft {
    pub name: String,
    pub category: String,
    pub format: String,
    pub positive: String,
    pub negative: String,
}

#[derive(Clone)]
pub struct WorkspaceDraft {
    pub prompt: String,
    pub negative: String,
    pub aspect: String,
    pub quality: String,
    pub count: i32,
    pub template_file: String,
    pub template_name: String,
    pub template_category: String,
    pub template_active_category: String,
    pub asset_purpose: String,
    pub color_mood: String,
    pub game_view: String,
    pub weather: String,
    pub time_of_day: String,
    pub lighting: String,
    pub advanced_open: bool,
    pub prompt_preview_open: bool,
    pub final_prompt_preview: String,
    pub ref_images: Vec<artait_model::ReferenceImage>,
}

pub struct PromptOptimizationResult {
    pub optimized_prompt: String,
    pub summary: String,
    pub changes: String,
}

// ── 常量 / 默认值 ─────────────────────────────────────────────────────────

pub fn default_template_category() -> &'static str {
    "默认"
}

pub fn default_prompt_template_content(page: &str) -> &'static str {
    match page {
        "ui_concept" => "用json反推一下参考图的界面美术风格",
        "character" | "animation_character" | "character_turnaround" => {
            "用json反推一下参考图的美术风格和人物特征"
        }
        "effect" => "用json反推一下参考图的特效美术风格",
        _ => "用json反推一下参考图的场景美术风格和特征",
    }
}

// ── 目录 / 路径 ───────────────────────────────────────────────────────────

pub fn prompt_template_dir(cfg: &AppConfig, page: &str) -> PathBuf {
    cfg.paths.prompt_dir.join(prompt_template_subdir(page))
}

fn prompt_template_subdir(page: &str) -> &'static str {
    match page {
        "ui_concept" => "ui_prompt",
        "character" | "animation_character" => "create_character_prompt",
        "character_turnaround" => "character_turnaround_prompt",
        "effect" => "effect_prompt",
        "storyboard" => "storyboard_prompt",
        _ => "scene_prompt",
    }
}

pub fn sanitize_template_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric()
                || c == '-'
                || c == '_'
                || ('\u{4e00}'..='\u{9fff}').contains(&c)
            {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

pub fn sanitize_template_category(category: &str) -> String {
    let safe = sanitize_template_name(category);
    if safe.is_empty() {
        default_template_category().to_string()
    } else {
        safe
    }
}

pub fn template_category_from_file(file_name: &str) -> String {
    let path = Path::new(file_name);
    path.parent()
        .and_then(|p| {
            if p.as_os_str().is_empty() {
                None
            } else {
                p.file_name()?.to_str()
            }
        })
        .unwrap_or(default_template_category())
        .to_string()
}

pub fn template_label_from_file(file_name: &str) -> String {
    let category = template_category_from_file(file_name);
    let name = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    if category == default_template_category() {
        name.to_string()
    } else {
        format!("{category} / {name}")
    }
}

pub fn prompt_template_relative_path(cfg: &AppConfig, page: &str, path: &Path) -> String {
    let root = prompt_template_dir(cfg, page);
    path.strip_prefix(&root)
        .ok()
        .and_then(|p| p.to_str())
        .unwrap_or_else(|| {
            path.file_name()
                .and_then(|v| v.to_str())
                .unwrap_or_default()
        })
        .replace('\\', "/")
}

fn safe_template_path(root: &Path, file_name: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in Path::new(file_name).components() {
        if let std::path::Component::Normal(value) = part {
            path.push(value);
        }
    }
    path
}

// ── 读写 ──────────────────────────────────────────────────────────────────

pub fn read_prompt_template(
    cfg: &AppConfig,
    page: &str,
    file_name: &str,
) -> std::io::Result<PromptTemplateDraft> {
    let root = prompt_template_dir(cfg, page);
    let path = safe_template_path(&root, file_name);
    let raw = std::fs::read_to_string(&path)?;
    let format = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt")
        .to_ascii_lowercase();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let category = template_category_from_file(file_name);

    if format == "json" {
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::Value::String(raw.clone()));
        let positive = parsed
            .get("ai_prompts")
            .and_then(|v| v.get("positive_prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or(raw.as_str())
            .to_string();
        let negative = parsed
            .get("ai_prompts")
            .and_then(|v| v.get("negative_prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(PromptTemplateDraft {
            name,
            category,
            format,
            positive,
            negative,
        })
    } else {
        Ok(PromptTemplateDraft {
            name,
            category,
            format: "txt".into(),
            positive: raw,
            negative: String::new(),
        })
    }
}

pub fn write_prompt_template(
    cfg: &AppConfig,
    page: &str,
    category: &str,
    name: &str,
    format: &str,
    positive: &str,
    negative: &str,
    original_file: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let safe = sanitize_template_name(name);
    let safe_category = sanitize_template_category(category);
    anyhow::ensure!(!safe.is_empty(), "提示词名称不能为空");
    anyhow::ensure!(!positive.trim().is_empty(), "提示词内容不能为空");

    let fmt = if format.eq_ignore_ascii_case("txt") {
        "txt"
    } else {
        "json"
    };
    let root = prompt_template_dir(cfg, page);
    let dir = if safe_category == default_template_category() {
        root.clone()
    } else {
        root.join(&safe_category)
    };
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(format!("{safe}.{fmt}"));

    if let Some(old) = original_file {
        let old_path = safe_template_path(&root, old);
        if old_path != dest && old_path.exists() {
            let _ = std::fs::remove_file(old_path);
        }
    }

    if fmt == "json" {
        let data = serde_json::json!({
            "ai_prompts": {
                "positive_prompt": positive.trim(),
                "negative_prompt": negative.trim(),
            }
        });
        std::fs::write(&dest, serde_json::to_string_pretty(&data)?)?;
    } else {
        std::fs::write(&dest, positive.trim())?;
    }

    Ok(dest)
}

// ── 生成用提示词拼接 ─────────────────────────────────────────────────────

pub fn build_generation_prompt(
    cfg: &AppConfig,
    page: &str,
    template_file: &str,
    manual_prompt: &str,
) -> anyhow::Result<String> {
    let mut parts = Vec::new();

    if !template_file.trim().is_empty() {
        let template = read_prompt_template(cfg, page, template_file)?;
        let template_text = prompt_template_generation_text(&template);
        if !template_text.trim().is_empty() {
            parts.push(template_text);
        }
    }

    if !manual_prompt.trim().is_empty() {
        parts.push(manual_prompt.trim().to_string());
    }

    Ok(parts.join("\n\n").trim().to_string())
}

pub fn prompt_template_generation_text(template: &PromptTemplateDraft) -> String {
    let mut parts = Vec::new();
    if !template.positive.trim().is_empty() {
        parts.push(template.positive.trim().to_string());
    }
    if !template.negative.trim().is_empty() {
        parts.push(format!("Negative prompt: {}", template.negative.trim()));
    }
    parts.join("\n\n")
}

// ── 提示词优化 ───────────────────────────────────────────────────────────

/// 用户可覆盖的提示词优化模板。
#[derive(Debug, Clone, serde::Deserialize)]
struct PromptOptimizationTemplate {
    system: String,
    user: String,
}

/// 内置默认模板——与之前硬编码内容一致。
fn builtin_prompt_optimization_template() -> PromptOptimizationTemplate {
    PromptOptimizationTemplate {
        system: concat!(
            "你是专业 AI 生图提示词优化师。{image_instruction}",
            "你的任务是把\"预设提示词\"、\"用户输入提示词\"和游戏开发导演控制融合为更稳定、更具体、可直接生图的提示词。",
            "导演控制是硬约束，不得删除、替换、弱化用途、游戏视角、天气、时间、光照或色彩氛围。",
            "optimized_prompt 只用于替换用户输入框内容，不要原样复制整段最终生成 Prompt 预览。",
            "不要输出 CFG、Steps、Sampler、Scheduler、Clip Skip、Denoise 等底层采样参数。",
            "不要输出私密思维链。请输出 JSON，字段必须为：",
            "optimized_prompt：最终替换用户输入框的提示词；",
            "summary：一句话说明优化依据；",
            "changes：用中文短句概括关键改写点，最多 80 字。",
            "optimized_prompt 优先使用英文美术描述；summary 和 changes 使用中文。",
        )
        .to_string(),
        user: concat!(
            "创作类型：{page}\n",
            "是否包含参考图：{has_images}\n\n",
            "预设提示词：\n{preset_prompt}\n\n",
            "用户输入提示词：\n{user_prompt}\n\n",
            "导演控制（硬约束）：\n{director_controls}\n\n",
            "当前最终生成 Prompt 预览（仅用于理解上下文，不要整段照抄）：\n{final_prompt_preview}\n\n",
            "请返回严格 JSON，不要 Markdown，不要解释 JSON 之外的内容。",
        )
        .to_string(),
    }
}

fn user_prompt_optimization_template_path() -> std::path::PathBuf {
    artait_model::portable_data_dir()
        .join("prompts")
        .join("prompt-optimization.toml")
}

/// 加载提示词优化模板：优先用户自定义，失败 / 不存在则用内置默认。
fn load_prompt_optimization_template() -> PromptOptimizationTemplate {
    let path = user_prompt_optimization_template_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => match toml::from_str::<PromptOptimizationTemplate>(&raw) {
            Ok(t) => {
                tracing::info!(path = %path.display(), "已加载自定义提示词优化模板");
                t
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "解析自定义提示词优化模板失败，回退内置");
                builtin_prompt_optimization_template()
            }
        },
        Err(_) => builtin_prompt_optimization_template(),
    }
}

pub fn prompt_optimization_system_prompt(with_images: bool) -> String {
    let template = load_prompt_optimization_template();
    let image_instruction = if with_images {
        "你还会收到参考图，必须结合参考图中的主体、风格、构图、材质、色彩和光影。"
    } else {
        "本次只基于文本信息优化，不要虚构不存在的参考图细节。"
    };
    template
        .system
        .replace("{image_instruction}", image_instruction)
}

pub fn build_prompt_optimization_user_prompt(
    page: &str,
    preset_prompt: &str,
    user_prompt: &str,
    with_images: bool,
) -> String {
    build_prompt_optimization_user_prompt_with_context(PromptOptimizationContext {
        page,
        preset_prompt,
        user_prompt,
        director_controls: "",
        final_prompt_preview: "",
        with_images,
    })
}

pub struct PromptOptimizationContext<'a> {
    pub page: &'a str,
    pub preset_prompt: &'a str,
    pub user_prompt: &'a str,
    pub director_controls: &'a str,
    pub final_prompt_preview: &'a str,
    pub with_images: bool,
}

pub fn build_prompt_optimization_user_prompt_with_context(
    ctx: PromptOptimizationContext<'_>,
) -> String {
    let template = load_prompt_optimization_template();
    let has_images = if ctx.with_images { "是" } else { "否" };
    let preset = if ctx.preset_prompt.trim().is_empty() {
        "无"
    } else {
        ctx.preset_prompt.trim()
    };
    let user = if ctx.user_prompt.trim().is_empty() {
        "无"
    } else {
        ctx.user_prompt.trim()
    };
    let director_controls = if ctx.director_controls.trim().is_empty() {
        "无"
    } else {
        ctx.director_controls.trim()
    };
    let final_prompt_preview = if ctx.final_prompt_preview.trim().is_empty() {
        "无"
    } else {
        ctx.final_prompt_preview.trim()
    };
    let rendered = template
        .user
        .replace("{page}", CreationMode::from_route(ctx.page).display_name())
        .replace("{has_images}", has_images)
        .replace("{preset_prompt}", preset)
        .replace("{user_prompt}", user)
        .replace("{director_controls}", director_controls)
        .replace("{final_prompt_preview}", final_prompt_preview);
    append_missing_optimization_context(rendered, director_controls, final_prompt_preview)
}

fn append_missing_optimization_context(
    mut rendered: String,
    director_controls: &str,
    final_prompt_preview: &str,
) -> String {
    if !rendered.contains("导演控制") {
        rendered.push_str("\n\n导演控制（硬约束）：\n");
        rendered.push_str(director_controls);
    }
    if !rendered.contains("最终生成 Prompt 预览") {
        rendered.push_str("\n\n当前最终生成 Prompt 预览（仅用于理解上下文，不要整段照抄）：\n");
        rendered.push_str(final_prompt_preview);
    }
    rendered
}

/// 首次启动时把内置默认模板写入用户数据目录，方便用户直接编辑。
pub fn install_sample_prompt_optimization_template() {
    let path = user_prompt_optimization_template_path();
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, "创建 prompts/ 目录失败");
            return;
        }
    }
    let content = format!(
        "# 提示词优化模板\n\
         # 占位符：{{image_instruction}} {{has_images}} {{page}} {{preset_prompt}} {{user_prompt}} {{director_controls}} {{final_prompt_preview}}\n\
         # 程序自动替换占位符后发送给 LLM。修改保存后下次优化即刻生效，无需重启。\n\n\
         system = \"\"\"\n{}\"\"\"\n\n\
         user = \"\"\"\n{}\"\"\"\n",
        builtin_prompt_optimization_template().system,
        builtin_prompt_optimization_template().user,
    );
    if let Err(e) = std::fs::write(&path, &content) {
        tracing::warn!(error = %e, path = %path.display(), "写入提示词优化模板样例失败");
    } else {
        tracing::info!("已写入提示词优化模板样例 → {}", path.display());
    }
}

pub fn parse_prompt_optimization_output(
    output: &artait_provider::request::AnalysisOutput,
) -> PromptOptimizationResult {
    let value = output
        .structured
        .clone()
        .or_else(|| extract_json_value(&output.text));
    if let Some(value) = value {
        let optimized = value
            .get("optimized_prompt")
            .or_else(|| value.get("prompt"))
            .or_else(|| value.get("positive_prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if !optimized.is_empty() {
            return PromptOptimizationResult {
                optimized_prompt: optimized,
                summary: value
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("已根据预设和用户输入完成优化。")
                    .trim()
                    .to_string(),
                changes: value
                    .get("changes")
                    .map(json_short_text)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "增强主体、风格、构图和生成约束。".into()),
            };
        }
    }

    PromptOptimizationResult {
        optimized_prompt: strip_code_fence(&output.text).trim().to_string(),
        summary: "模型返回了非结构化内容，已直接作为优化提示词填入。".into(),
        changes: "建议检查提示词是否符合预期。".into(),
    }
}

fn extract_json_value(text: &str) -> Option<serde_json::Value> {
    let trimmed = strip_code_fence(text);
    serde_json::from_str::<serde_json::Value>(&trimmed)
        .ok()
        .or_else(|| {
            let start = trimmed.find('{')?;
            let end = trimmed.rfind('}')?;
            if end <= start {
                return None;
            }
            serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]).ok()
        })
}

fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let without_open = trimmed.lines().skip(1).collect::<Vec<_>>().join("\n");
    without_open
        .strip_suffix("```")
        .unwrap_or(&without_open)
        .trim()
        .to_string()
}

fn json_short_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join("；"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_optimization_context_includes_director_controls() {
        let prompt = build_prompt_optimization_user_prompt_with_context(PromptOptimizationContext {
            page: "scene",
            preset_prompt: "fantasy game environment",
            user_prompt: "rainy forest village",
            director_controls: "用途: TileSet · 视角: 2.5D · 天气: 雨天 · 时间: 深夜",
            final_prompt_preview: "rainy forest village\n\n[Game Art Direction]\nPurpose: TileSet\nGame view: 2.5D",
            with_images: true,
        });

        assert!(prompt.contains("创作类型：创建场景"));
        assert!(prompt.contains("是否包含参考图：是"));
        assert!(prompt.contains("用途: TileSet"));
        assert!(prompt.contains("视角: 2.5D"));
        assert!(prompt.contains("当前最终生成 Prompt 预览"));
        assert!(prompt.contains("Purpose: TileSet"));
    }

    #[test]
    fn prompt_optimization_system_prompt_marks_director_as_constraint() {
        let prompt = prompt_optimization_system_prompt(false);

        assert!(prompt.contains("导演控制是硬约束"));
        assert!(prompt.contains("CFG"));
        assert!(prompt.contains("不要输出"));
    }
}
