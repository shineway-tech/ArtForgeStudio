//! 角色 Prompt 构建器。
//!
//! 将角色数据模型（特别是 6 层身份锚点）编译为 AI 图像生成的提示词。
//! 纯文本处理，无 IO、无异步。

use artait_model::{Character, CharacterIdentityAnchors, CharacterVariation};

// ============================================================================
// 公共接口
// ============================================================================

/// 角色生成配置（决定 prompt 语言、角色表元素）。
#[derive(Debug, Clone)]
pub struct CharacterPromptConfig {
    /// 提示词语言偏好
    pub language: PromptLanguage,
    /// 是否包含三视图
    pub include_three_view: bool,
    /// 是否包含表情集
    pub include_expressions: bool,
    /// 是否包含比例参考
    pub include_proportions: bool,
    /// 是否包含姿势集
    pub include_poses: bool,
    /// 是否有参考图（影响锚点使用策略）
    pub has_reference_images: bool,
    /// 视觉风格 Token（如 "anime", "realistic"）
    pub style_tokens: Vec<String>,
    /// 是否为动漫风格（影响分支措辞）
    pub is_anime: bool,
}

impl Default for CharacterPromptConfig {
    fn default() -> Self {
        Self {
            language: PromptLanguage::English,
            include_three_view: true,
            include_expressions: true,
            include_proportions: false,
            include_poses: false,
            has_reference_images: false,
            style_tokens: vec![],
            is_anime: true,
        }
    }
}

/// 提示词语言偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptLanguage {
    /// 仅英文
    #[default]
    English,
    /// 仅中文
    Chinese,
    /// 中英双语（英文为主，中文为辅）
    Bilingual,
}

/// 角色表生成的结果提示词。
#[derive(Debug, Clone)]
pub struct CharacterSheetPrompt {
    /// 正向提示词
    pub positive: String,
    /// 负面提示词
    pub negative: String,
}

// ============================================================================
// 主入口
// ============================================================================

/// 构建角色表（Character Sheet）的完整提示词。
///
/// 这是角色图像生成的主入口，整合了角色基础描述、6 层锚点、
/// 角色表元素、风格和质量关键词。
pub fn build_character_sheet_prompt(
    character: &Character,
    config: &CharacterPromptConfig,
) -> CharacterSheetPrompt {
    let positive = build_positive_prompt(character, config);
    let negative = build_negative_prompt(character);
    CharacterSheetPrompt { positive, negative }
}

/// 构建服装变体的提示词。
///
/// 用于衣柜系统中的单个变体生成。当有服装参考图时，
/// 需要在 prompt 中加入融合指引。
pub fn build_variation_prompt(
    character: &Character,
    variation: &CharacterVariation,
    _has_clothing_references: bool,
    config: &CharacterPromptConfig,
) -> CharacterSheetPrompt {
    // 变体描述覆盖基础描述
    let var_desc = match config.language {
        PromptLanguage::English => variation.visual_prompt.clone(),
        PromptLanguage::Chinese => variation
            .visual_prompt_zh
            .clone()
            .unwrap_or_else(|| variation.visual_prompt.clone()),
        PromptLanguage::Bilingual => {
            if let Some(ref zh) = variation.visual_prompt_zh {
                format!("{}, {}", zh, variation.visual_prompt)
            } else {
                variation.visual_prompt.clone()
            }
        }
    };

    let mut parts: Vec<String> = Vec::new();

    // 角色表头部
    if config.is_anime {
        parts.push(format!(
            "professional anime character sheet for \"{}\"",
            character.name
        ));
    } else {
        parts.push(format!(
            "professional character reference sheet for \"{}\"",
            character.name
        ));
    }

    // 基础描述（锚点策略与 build_anchor_prompt 一致）
    let anchor_text = build_anchor_prompt(character, config.has_reference_images);
    if !anchor_text.is_empty() {
        parts.push(anchor_text);
    }

    // 变体描述
    parts.push(var_desc);

    // 角色表元素
    let sheet_elements = build_sheet_elements(config);
    if !sheet_elements.is_empty() {
        parts.push(sheet_elements);
    }

    // 白底要求
    parts.push("white background, clean background".into());

    // 风格和质量
    let quality = build_quality_modifiers(config.is_anime);
    parts.push(quality);

    let positive = parts.join(", ");
    let negative = build_negative_prompt(character);

    CharacterSheetPrompt { positive, negative }
}

// ============================================================================
// 正向提示词
// ============================================================================

fn build_positive_prompt(character: &Character, config: &CharacterPromptConfig) -> String {
    let mut parts: Vec<String> = Vec::new();

    // 1. 角色表头部
    if config.is_anime {
        parts.push(format!(
            "professional anime character sheet for \"{}\"",
            character.name
        ));
    } else {
        parts.push(format!(
            "professional character reference sheet for \"{}\"",
            character.name
        ));
    }

    // 2. 基础视觉描述（按语言偏好选择）
    let base_desc = select_primary_prompt(character, config.language);
    parts.push(base_desc);

    // 3. 6 层锚点（根据是否有参考图切换策略）
    let anchor_text = build_anchor_prompt(character, config.has_reference_images);
    if !anchor_text.is_empty() {
        parts.push(anchor_text);
    }

    // 4. 角色表元素
    let sheet_elements = build_sheet_elements(config);
    if !sheet_elements.is_empty() {
        parts.push(sheet_elements);
    }

    // 5. 白底
    parts.push("white background, clean background".into());

    // 6. 风格 Tokens
    if !config.style_tokens.is_empty() {
        let style_part = config.style_tokens.join(", ");
        parts.push(style_part);
    }

    // 7. 质量修饰词
    let quality = build_quality_modifiers(config.is_anime);
    parts.push(quality);

    parts.join(", ")
}

fn select_primary_prompt(character: &Character, language: PromptLanguage) -> String {
    match language {
        PromptLanguage::English => character
            .visual_prompt_en
            .clone()
            .or_else(|| character.description.clone())
            .or_else(|| character.visual_prompt_zh.clone())
            .unwrap_or_else(|| character.name.clone()),
        PromptLanguage::Chinese => character
            .visual_prompt_zh
            .clone()
            .or_else(|| character.description.clone())
            .or_else(|| character.visual_prompt_en.clone())
            .unwrap_or_else(|| character.name.clone()),
        PromptLanguage::Bilingual => {
            let en = character.visual_prompt_en.as_deref().unwrap_or("");
            let zh = character.visual_prompt_zh.as_deref().unwrap_or("");
            if !en.is_empty() && !zh.is_empty() {
                format!("{}, {}", en, zh)
            } else if !en.is_empty() {
                en.to_string()
            } else if !zh.is_empty() {
                zh.to_string()
            } else {
                character.name.clone()
            }
        }
    }
}

// ============================================================================
// 6 层锚点 → Prompt 翻译
// ============================================================================

/// 将 6 层身份锚点翻译为 prompt 文本。
///
/// 策略：
/// - **有参考图**：只用第③层（独特标记）和第④层（色彩锚点）—— 最强锚点
/// - **无参考图**：完整的 ①-⑥ 六层锁定
pub fn build_anchor_prompt(character: &Character, has_reference_images: bool) -> String {
    let anchors = match &character.identity_anchors {
        Some(a) => a,
        None => return String::new(),
    };

    if anchors.is_empty() {
        return String::new();
    }

    if has_reference_images {
        build_anchors_with_reference(anchors)
    } else {
        build_anchors_full(anchors)
    }
}

/// 有参考图模式：仅最强锚点（独特标记 + 色彩）
fn build_anchors_with_reference(anchors: &CharacterIdentityAnchors) -> String {
    let mut parts: Vec<String> = Vec::new();

    // ③ 辨识标记（最强锚点）
    if !anchors.unique_marks.is_empty() {
        let marks = anchors.unique_marks.join(", ");
        parts.push(format!("distinguishing features: {}", marks));
    }

    // ④ 色彩锚点
    if let Some(ref colors) = anchors.color_anchors {
        let mut color_parts: Vec<String> = Vec::new();
        if let Some(ref iris) = colors.iris {
            color_parts.push(format!("eye color {}", iris));
        }
        if let Some(ref hair) = colors.hair {
            color_parts.push(format!("hair color {}", hair));
        }
        if let Some(ref skin) = colors.skin {
            color_parts.push(format!("skin tone {}", skin));
        }
        if let Some(ref lips) = colors.lips {
            color_parts.push(format!("lip color {}", lips));
        }
        if !color_parts.is_empty() {
            parts.push(format!("color anchors: {}", color_parts.join(", ")));
        }
    }

    parts.join(". ")
}

/// 无参考图模式：完整六层锁定
fn build_anchors_full(anchors: &CharacterIdentityAnchors) -> String {
    let mut parts: Vec<String> = Vec::new();

    // ① 骨相
    let mut bone_parts: Vec<String> = Vec::new();
    if let Some(ref v) = anchors.face_shape {
        bone_parts.push(format!("{} face shape", v));
    }
    if let Some(ref v) = anchors.jawline {
        bone_parts.push(format!("{} jawline", v));
    }
    if let Some(ref v) = anchors.cheekbones {
        bone_parts.push(format!("{} cheekbones", v));
    }
    if !bone_parts.is_empty() {
        parts.push(bone_parts.join(", "));
    }

    // ② 五官
    let mut feature_parts: Vec<String> = Vec::new();
    if let Some(ref v) = anchors.eye_shape {
        feature_parts.push(format!("{} eyes", v));
    }
    if let Some(ref v) = anchors.eye_details {
        feature_parts.push(v.clone());
    }
    if let Some(ref v) = anchors.nose_shape {
        feature_parts.push(v.clone());
    }
    if let Some(ref v) = anchors.lip_shape {
        feature_parts.push(v.clone());
    }
    if !feature_parts.is_empty() {
        parts.push(feature_parts.join(", "));
    }

    // ③ 辨识标记
    if !anchors.unique_marks.is_empty() {
        let marks = anchors.unique_marks.join(", ");
        parts.push(format!("distinctive marks: {}", marks));
    }

    // ④ 色彩锚点
    if let Some(ref colors) = anchors.color_anchors {
        let mut color_parts: Vec<String> = Vec::new();
        if let Some(ref iris) = colors.iris {
            color_parts.push(format!("eye color {}", iris));
        }
        if let Some(ref hair) = colors.hair {
            color_parts.push(format!("hair color {}", hair));
        }
        if let Some(ref skin) = colors.skin {
            color_parts.push(format!("skin tone {}", skin));
        }
        if let Some(ref lips) = colors.lips {
            color_parts.push(format!("lip color {}", lips));
        }
        if !color_parts.is_empty() {
            parts.push(format!("color anchors: {}", color_parts.join(", ")));
        }
    }

    // ⑤ 皮肤纹理
    if let Some(ref v) = anchors.skin_texture {
        parts.push(v.clone());
    }

    // ⑥ 发型
    let mut hair_parts: Vec<String> = Vec::new();
    if let Some(ref v) = anchors.hair_style {
        hair_parts.push(v.clone());
    }
    if let Some(ref v) = anchors.hairline_details {
        hair_parts.push(format!("hairline: {}", v));
    }
    if !hair_parts.is_empty() {
        parts.push(hair_parts.join(", "));
    }

    parts.join(". ")
}

// ============================================================================
// 角色表元素
// ============================================================================

/// 根据配置生成角色表元素描述。
fn build_sheet_elements(config: &CharacterPromptConfig) -> String {
    let mut elements: Vec<&str> = Vec::new();

    if config.include_three_view {
        elements.push("three-view turnarounds (front, side, back views)");
    }
    if config.include_expressions {
        elements.push("facial expression sheet with 5 different emotions");
    }
    if config.include_proportions {
        elements.push("body proportion reference with height chart");
    }
    if config.include_poses {
        elements.push("dynamic action pose sheet with 4 different stances");
    }

    if elements.is_empty() {
        String::new()
    } else {
        format!("character sheet with {}", elements.join(", "))
    }
}

// ============================================================================
// 质量修饰词
// ============================================================================

/// 生成质量修饰词，根据 realistic/anime 分支采用不同措辞。
fn build_quality_modifiers(is_anime: bool) -> String {
    if is_anime {
        "high quality anime illustration, clean linework, solid flat colors, \
         consistent character design, masterwork"
            .into()
    } else {
        "highly detailed, photorealistic rendering, sharp focus, \
         professional photography lighting, 8k, masterpiece"
            .into()
    }
}

// ============================================================================
// 负面提示词
// ============================================================================

/// 构建角色生成的负面提示词。
///
/// 合并基础通用负面提示词 + 角色自定义负面提示词。
fn build_negative_prompt(character: &Character) -> String {
    let mut parts: Vec<String> = Vec::new();

    // 基础通用负面提示词
    parts.extend_from_slice(&[
        "blurry".into(),
        "low quality".into(),
        "worst quality".into(),
        "watermark".into(),
        "text".into(),
        "signature".into(),
        "bad anatomy".into(),
        "deformed".into(),
        "disfigured".into(),
        "extra limbs".into(),
        "fused fingers".into(),
        "poorly drawn hands".into(),
        "poorly drawn face".into(),
        "mutation".into(),
        "mutated".into(),
        "ugly".into(),
        "bad proportions".into(),
        "cloned face".into(),
        "gross proportions".into(),
        "missing arms".into(),
        "missing legs".into(),
        "extra arms".into(),
        "extra legs".into(),
        "fused together".into(),
        "out of frame".into(),
        "cropped".into(),
    ]);

    // 角色自定义负面提示词
    if let Some(ref np) = character.negative_prompt {
        parts.extend(np.avoid.iter().cloned());
        parts.extend(np.style_exclusions.iter().cloned());
    }

    parts.join(", ")
}

// ============================================================================
// 时代服装指导（后续由剧本元数据驱动）
// ============================================================================

/// 中国朝代/年代 → 服装风格指导文本。
///
/// 当剧本设定了具体朝代/年代时，注入对应的服装指导。
/// 目前为英文（直接用于 prompt）。
pub fn era_fashion_guidance(era_key: &str) -> Option<&'static str> {
    match era_key.to_lowercase().as_str() {
        "tang" | "唐朝" | "唐代" => Some(
            "Tang Dynasty attire: flowing silk robes with wide sleeves, \
             round-collar gowns, elaborate headdresses, vibrant colors",
        ),
        "song" | "宋朝" | "宋代" => Some(
            "Song Dynasty attire: elegant beizi jackets, narrow sleeves, \
             light colors, simple jade ornaments, literati aesthetic",
        ),
        "ming" | "明朝" | "明代" => Some(
            "Ming Dynasty attire: high-waisted mamian skirts, cross-collar \
             robes, intricate embroidery, gold and jade hairpins",
        ),
        "qing" | "清朝" | "清代" => Some(
            "Qing Dynasty attire: Manchurian-style qipao, changshan robes, \
             elaborate embroidery, dragon motifs, mandarin jackets",
        ),
        "republican" | "民国" => Some(
            "Republican era attire: tailored suits, qipao dresses, \
             western-influenced fashion, Shanghai style of 1920s-1930s",
        ),
        "modern" | "现代" => Some(
            "Modern contemporary fashion: casual streetwear, \
             modern professional attire, current trends",
        ),
        "ancient" | "古代" | "mythological" | "神话" => Some(
            "Ancient Chinese mythological attire: flowing celestial robes, \
             floating ribbons, ornate hairpieces with pearls and gold",
        ),
        "wuxia" | "武侠" => Some(
            "Wuxia martial arts attire: layered hanfu with practical cuts, \
             sword sashes, leather vambraces, wind-swept silhouette",
        ),
        "xianxia" | "仙侠" => Some(
            "Xianxia immortal attire: ethereal layered silk robes, \
             floating sashes, ornate guan headpieces, jade pendants, \
             glowing embroidered patterns",
        ),
        _ => None,
    }
}

/// 获取时代服装提示词的便捷方法。
///
/// 如果 era_key 匹配到已知时代，返回 "era fashion: {guidance}" 格式的文本；
/// 否则返回空字符串。
pub fn era_fashion_prompt(era_key: Option<&str>) -> String {
    match era_key.and_then(era_fashion_guidance) {
        Some(guidance) => format!("era fashion: {}", guidance),
        None => String::new(),
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use artait_model::{
        Character, CharacterIdentityAnchors, CharacterNegativePrompt, ColorAnchors,
    };

    fn make_test_character() -> Character {
        let mut c = Character::new("c1".into(), "云中鹤".into());
        c.visual_prompt_en = Some("a tall elegant swordsman with flowing white hair".into());
        c.visual_prompt_zh = Some("一位飘逸白发的修长剑客".into());
        c.identity_anchors = Some(CharacterIdentityAnchors {
            face_shape: Some("oval".into()),
            jawline: Some("sharp angular".into()),
            eye_shape: Some("almond".into()),
            unique_marks: vec!["thin scar across left eyebrow".into()],
            color_anchors: Some(ColorAnchors {
                iris: Some("#3D2314".into()),
                hair: Some("#F0F0F0".into()),
                skin: Some("#E8C4A0".into()),
                lips: None,
            }),
            hair_style: Some("waist-length straight, center-parted".into()),
            ..Default::default()
        });
        c.negative_prompt = Some(CharacterNegativePrompt {
            avoid: vec!["glasses".into(), "beard".into()],
            style_exclusions: vec![],
        });
        c
    }

    #[test]
    fn full_anchor_prompt_includes_all_layers() {
        let c = make_test_character();
        let prompt = build_anchor_prompt(&c, false);
        assert!(prompt.contains("oval face shape"));
        assert!(prompt.contains("almond eyes"));
        assert!(prompt.contains("scar across left eyebrow"));
        assert!(prompt.contains("eye color #3D2314"));
        assert!(prompt.contains("waist-length straight"));
    }

    #[test]
    fn ref_mode_anchor_prompt_only_marks_and_colors() {
        let c = make_test_character();
        let prompt = build_anchor_prompt(&c, true);
        // 有参考图：只包含 ③ marks + ④ colors
        assert!(prompt.contains("scar across left eyebrow"));
        assert!(prompt.contains("color anchors"));
        // 不包含骨相、五官、发型
        assert!(!prompt.contains("oval face shape"));
        assert!(!prompt.contains("almond eyes"));
        assert!(!prompt.contains("waist-length straight"));
    }

    #[test]
    fn empty_anchors_return_empty() {
        let c = Character::new("c2".into(), "无名".into());
        let prompt = build_anchor_prompt(&c, false);
        assert!(prompt.is_empty());
    }

    #[test]
    fn sheet_prompt_includes_name_and_quality() {
        let c = make_test_character();
        let config = CharacterPromptConfig::default();
        let result = build_character_sheet_prompt(&c, &config);
        assert!(result.positive.contains("云中鹤"));
        assert!(result.positive.contains("anime"));
        assert!(result.positive.contains("white background"));
        assert!(result.negative.contains("blurry"));
    }

    #[test]
    fn negative_prompt_includes_custom_avoids() {
        let c = make_test_character();
        let result = build_character_sheet_prompt(&c, &CharacterPromptConfig::default());
        assert!(result.negative.contains("glasses"));
        assert!(result.negative.contains("beard"));
    }

    #[test]
    fn variation_prompt_uses_variation_description() {
        let c = make_test_character();
        let var = CharacterVariation {
            id: "v1".into(),
            name: "战斗装".into(),
            visual_prompt: "wearing black combat armor with silver trim".into(),
            visual_prompt_zh: Some("身穿黑底银边战斗铠甲".into()),
            ..make_empty_variation()
        };
        let config = CharacterPromptConfig::default();
        let result = build_variation_prompt(&c, &var, false, &config);
        assert!(result.positive.contains("combat armor"));
        assert!(result.positive.contains("character sheet"));
        assert!(result.positive.contains("white background"));
    }

    #[test]
    fn era_fashion_returns_correct_guidance() {
        assert!(era_fashion_guidance("tang")
            .unwrap()
            .contains("Tang Dynasty"));
        assert!(era_fashion_guidance("宋朝")
            .unwrap()
            .contains("Song Dynasty"));
        assert!(era_fashion_guidance("modern").unwrap().contains("Modern"));
        assert!(era_fashion_guidance("unknown").is_none());
    }

    #[test]
    fn bilingual_selects_both_languages() {
        let c = make_test_character();
        let config = CharacterPromptConfig {
            language: PromptLanguage::Bilingual,
            ..Default::default()
        };
        let result = build_character_sheet_prompt(&c, &config);
        assert!(result.positive.contains("swordsman"));
        assert!(result.positive.contains("剑客"));
    }

    fn make_empty_variation() -> CharacterVariation {
        CharacterVariation {
            id: String::new(),
            name: String::new(),
            visual_prompt: String::new(),
            visual_prompt_zh: None,
            reference_image: None,
            clothing_reference_images: vec![],
            generated_at: None,
            is_stage_variation: false,
            episode_range: None,
            age_description: None,
            stage_description: None,
        }
    }
}
