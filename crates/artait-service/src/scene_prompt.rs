//! 场景提示词生成器。
//!
//! 将场景数据编译为 AI 图像生成的提示词，
//! 包括多视角联合图的提示词构建。

use artait_model::scene::{ContactSheetLayout, Scene};

/// 场景生成配置。
#[derive(Debug, Clone)]
pub struct ScenePromptConfig {
    /// 是否为动漫风格
    pub is_anime: bool,
    /// 视觉风格 tokens
    pub style_tokens: Vec<String>,
}

impl Default for ScenePromptConfig {
    fn default() -> Self {
        Self {
            is_anime: true,
            style_tokens: vec![],
        }
    }
}

/// 场景生成结果提示词。
pub struct ScenePrompt {
    pub positive: String,
    pub negative: String,
}

/// 构建单场景提示词。
pub fn build_scene_prompt(scene: &Scene, config: &ScenePromptConfig) -> ScenePrompt {
    let mut parts: Vec<String> = Vec::new();

    // 场景描述
    if let Some(ref en) = scene.visual_prompt_en {
        parts.push(en.clone());
    } else if let Some(ref zh) = scene.visual_prompt_zh {
        parts.push(zh.clone());
    }

    // 地点
    if !scene.location.is_empty() {
        parts.push(format!("location: {}", scene.location));
    }

    // 时间
    if let Some(ref tod) = scene.time_of_day {
        let time_en = match tod.as_str() {
            "日" => "daytime",
            "夜" => "night",
            "晨" => "morning",
            "暮" => "dusk",
            _ => tod,
        };
        parts.push(format!("time of day: {}", time_en));
    }

    // 氛围
    if let Some(ref atm) = scene.atmosphere {
        parts.push(format!("atmosphere: {}", atm));
    }

    // 建筑风格
    if let Some(ref arch) = scene.architecture_style {
        parts.push(format!("architecture: {}", arch));
    }

    // 光影
    if let Some(ref light) = scene.lighting_design {
        parts.push(format!("lighting: {}", light));
    }

    // 色彩
    if let Some(ref color) = scene.color_palette {
        parts.push(format!("color palette: {}", color));
    }

    // 风格 + 质量
    if !config.style_tokens.is_empty() {
        parts.push(config.style_tokens.join(", "));
    }
    parts.push(build_quality_modifiers(config.is_anime));

    let positive = parts.join(", ");
    let negative = build_negative_prompt();

    ScenePrompt { positive, negative }
}

/// 构建多视角联合图的提示词。
///
/// 联合图包含多个视角，AI 一次生成后自动切割。
pub fn build_contact_sheet_prompt(
    scene: &Scene,
    layout: ContactSheetLayout,
    config: &ScenePromptConfig,
) -> ScenePrompt {
    let mut parts: Vec<String> = Vec::new();

    // 场景描述（英文优先）
    let desc = scene
        .visual_prompt_en
        .as_deref()
        .or(scene.visual_prompt_zh.as_deref())
        .unwrap_or(&scene.name);
    parts.push(desc.to_string());

    // 地点 + 时间
    if !scene.location.is_empty() {
        parts.push(scene.location.clone());
    }
    if let Some(ref tod) = scene.time_of_day {
        parts.push(tod.clone());
    }

    // 视角列表
    if !scene.viewpoints.is_empty() {
        let vp_descs: Vec<String> = scene
            .viewpoints
            .iter()
            .map(|v| {
                if v.name_en.is_empty() {
                    v.name.clone()
                } else {
                    format!("{} ({})", v.name_en, v.name)
                }
            })
            .collect();
        parts.push(format!(
            "{} viewpoints: {}",
            layout.total_cells(),
            vp_descs.join("; ")
        ));
    } else {
        parts.push(format!(
            "{} different camera angles of the same scene",
            layout.total_cells()
        ));
    }

    // 联合图布局指令
    parts.push(format!(
        "contact sheet layout: {} columns × {} rows, total {} images",
        layout.columns(),
        layout.rows(),
        layout.total_cells()
    ));
    parts.push("each cell shows a different viewpoint of the same location, consistent lighting and style across all cells".into());

    // 风格
    if !config.style_tokens.is_empty() {
        parts.push(config.style_tokens.join(", "));
    }
    parts.push(build_quality_modifiers(config.is_anime));

    let positive = parts.join(", ");
    let negative = build_negative_prompt();

    ScenePrompt { positive, negative }
}

fn build_quality_modifiers(is_anime: bool) -> String {
    if is_anime {
        "high quality anime background art, detailed environment, clean composition, painterly style"
            .into()
    } else {
        "highly detailed photorealistic environment, 8k, architectural photography, sharp focus"
            .into()
    }
}

fn build_negative_prompt() -> String {
    [
        "blurry",
        "low quality",
        "worst quality",
        "watermark",
        "text",
        "people",
        "characters",
        "person",
        "human",
        "figure",
        "bad anatomy",
        "deformed",
        "disfigured",
    ]
    .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use artait_model::scene::Scene;

    #[test]
    fn scene_prompt_includes_location_and_time() {
        let mut s = Scene::new("s1".into(), "酒馆".into());
        s.location = "古代酒馆大厅".into();
        s.time_of_day = Some("夜".into());
        s.visual_prompt_en = Some("a rustic medieval tavern interior".into());

        let prompt = build_scene_prompt(&s, &ScenePromptConfig::default());
        assert!(prompt.positive.contains("rustic medieval tavern"));
        assert!(prompt.positive.contains("古代")); // location field is Chinese
        assert!(prompt.positive.contains("night"));
        assert!(prompt.negative.contains("people"));
    }

    #[test]
    fn contact_sheet_includes_layout() {
        use artait_model::scene::SceneViewpoint;
        let mut s = Scene::new("s1".into(), "会议室".into());
        s.visual_prompt_en = Some("modern meeting room".into());
        s.viewpoints.push(SceneViewpoint {
            id: "door".into(),
            name: "门口".into(),
            name_en: "Door".into(),
            shot_ids: vec![],
            key_props: vec![],
            grid_index: 0,
            image_url: None,
            generated_at: None,
        });

        let prompt = build_contact_sheet_prompt(
            &s,
            ContactSheetLayout::Grid3x2,
            &ScenePromptConfig::default(),
        );
        assert!(prompt.positive.contains("3 columns"));
        assert!(prompt.positive.contains("2 rows"));
        assert!(prompt.positive.contains("Door"));
    }
}
