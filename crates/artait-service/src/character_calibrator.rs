//! 角色校准服务。
//!
//! 从剧本中提取的角色名和统计数据出发，通过 AI 完成：
//! 1. 角色去重与合并（识别同一角色的不同称呼）
//! 2. 重要度分级（主角/重要配角/配角/群演）
//! 3. 属性补全（性别、年龄、身份、人际关系等）
//! 4. 视觉锚点生成（6 层身份锚点 + 视觉提示词）
//!
//! 依赖 Analyzer trait（文本推理 AI），供 Phase 2 剧本系统集成。

use anyhow::{Context, Result};
use artait_model::{
    CalibratedCharacter, CharacterCalibrationResult, CharacterIdentityAnchors,
    CharacterNegativePrompt, CharacterStats, FilteredCharacterRecord, Importance,
};
use artait_provider::{
    request::{AnalysisRequest, AnalysisResponseFormat},
    Analyzer, ProviderContext,
};
use serde_json::Value;
use tracing::{info, warn};

// ============================================================================
// 公共 API
// ============================================================================

/// 校准严格度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationStrictness {
    /// 严格模式：过滤群演，只保留有名有姓且出场≥2 次的角色
    Strict,
    /// 普通模式：保留所有具名角色
    Normal,
    /// 宽松模式：保留所有角色（含无名群演）
    Loose,
}

impl Default for CalibrationStrictness {
    fn default() -> Self {
        Self::Normal
    }
}

/// 角色校准的完整输入。
pub struct CalibrationInput {
    /// 角色统计列表（从剧本扫描）
    pub stats: Vec<CharacterStats>,
    /// 严格度
    pub strictness: CalibrationStrictness,
    /// 每批发送给 AI 的最大角色数（默认 30）
    pub batch_size: usize,
    /// 剧本主题/背景（用于 AI 上下文）
    pub script_context: Option<String>,
}

impl CalibrationInput {
    pub fn new(stats: Vec<CharacterStats>) -> Self {
        Self {
            stats,
            strictness: CalibrationStrictness::default(),
            batch_size: 30,
            script_context: None,
        }
    }
}

/// 运行完整的角色校准流水线（4 步）。
///
/// # 参数
/// - `analyzer` — AI 文本推理能力
/// - `pctx` — Provider 上下文
/// - `input` — 校准输入（角色统计 + 配置）
///
/// # 返回
/// `CharacterCalibrationResult` — 包含校准后的角色列表、过滤/合并记录
pub async fn calibrate_characters(
    analyzer: &dyn Analyzer,
    pctx: &ProviderContext,
    input: CalibrationInput,
) -> Result<CharacterCalibrationResult> {
    info!(
        count = input.stats.len(),
        strictness = ?input.strictness,
        "开始角色校准"
    );

    // Step 1 & 2: 排序和筛选（本地处理）
    let candidates = sort_and_filter(&input.stats, input.strictness);

    if candidates.is_empty() {
        return Ok(CharacterCalibrationResult {
            characters: vec![],
            filtered_words: vec![],
            filtered_characters: vec![],
            merge_records: vec![],
            analysis_notes: "无有效角色".into(),
        });
    }

    // Step 3: 批量 AI 校准（去重、分级、补全属性）
    let calibrated = batch_calibrate(
        analyzer,
        pctx,
        &candidates,
        input.batch_size,
        &input.script_context,
    )
    .await?;

    // Step 4: 视觉锚点补全（仅主角/重要配角）
    let enriched = enrich_with_visual_anchors(analyzer, pctx, &calibrated).await?;

    // 组装结果
    let characters = enriched;

    // 提取过滤和合并记录（从 AI 返回中解析，简化为基于统计生成）
    let filtered_characters = candidates
        .iter()
        .filter(|s| {
            !characters
                .iter()
                .any(|c| c.name == s.name || c.name_variants.iter().any(|v| v == &s.name))
        })
        .map(|s| FilteredCharacterRecord {
            name: s.name.clone(),
            reason: "合并或过滤".into(),
        })
        .collect();

    let result = CharacterCalibrationResult {
        characters,
        filtered_words: vec![],
        filtered_characters,
        merge_records: vec![],
        analysis_notes: String::new(),
    };

    info!(calibrated = result.characters.len(), "角色校准完成");
    Ok(result)
}

// ============================================================================
// Step 1 & 2: 排序与筛选
// ============================================================================

/// 按优先级排序角色并应用严格度筛选。
///
/// 排序规则：
/// - 具名角色（有实际名称，非"路人甲"类）权重 +1000
/// - 无名角色按出场次数排序
/// - 群演类角色（含"群演""路人""士兵"等关键词）权重 -1000
fn sort_and_filter(
    stats: &[CharacterStats],
    strictness: CalibrationStrictness,
) -> Vec<CharacterStats> {
    let mut filtered: Vec<CharacterStats> = stats
        .iter()
        .filter(|s| match strictness {
            CalibrationStrictness::Strict => {
                // 必须有名有姓且出场 ≥ 2 次
                is_named_character(&s.name) && s.scene_count >= 2
            }
            CalibrationStrictness::Normal => {
                // 保留所有具名角色
                is_named_character(&s.name)
            }
            CalibrationStrictness::Loose => {
                // 保留全部
                true
            }
        })
        .cloned()
        .collect();

    // 排序：具名优先，然后按出场次数降序
    filtered.sort_by(|a, b| {
        let a_priority = if is_named_character(&a.name) { 1000 } else { 0 } + a.scene_count as i32;
        let b_priority = if is_named_character(&b.name) { 1000 } else { 0 } + b.scene_count as i32;
        b_priority.cmp(&a_priority)
    });

    // 限制最多 150 个发给 AI
    filtered.truncate(150);

    filtered
}

/// 判断是否为有名有姓的角色（非路人/群演类）。
fn is_named_character(name: &str) -> bool {
    let lower = name.to_lowercase();
    let generic_keywords = [
        "路人",
        "群演",
        "士兵",
        "侍卫",
        "侍女",
        "丫鬟",
        "太监",
        "百姓",
        "观众",
        "同学",
        "同事",
        "客人",
        "路人甲",
        "路人乙",
        "群众",
        "passerby",
        "extra",
        "crowd",
        "guard",
        "soldier",
        "servant",
        "citizen",
        "bystander",
        "background",
    ];
    !generic_keywords.iter().any(|kw| lower.contains(kw)) && name.chars().count() >= 2
}

// ============================================================================
// Step 3: 批量 AI 校准
// ============================================================================

/// 将角色分批发送给 AI，合并去重 + 分级 + 属性补全。
async fn batch_calibrate(
    analyzer: &dyn Analyzer,
    pctx: &ProviderContext,
    candidates: &[CharacterStats],
    batch_size: usize,
    script_context: &Option<String>,
) -> Result<Vec<CalibratedCharacter>> {
    let mut all_calibrated: Vec<CalibratedCharacter> = Vec::new();
    let batch_size = batch_size.max(1).min(50);

    for (batch_idx, batch) in candidates.chunks(batch_size).enumerate() {
        let batch_len = batch.len();
        info!(batch = batch_idx, size = batch_len, "发送批次校准");

        match calibrate_batch(analyzer, pctx, batch, script_context).await {
            Ok(mut chars) => {
                // 为每个角色赋予唯一 ID
                for c in &mut chars {
                    if c.id.is_empty() {
                        c.id = uuid::Uuid::new_v4().to_string();
                    }
                }
                all_calibrated.append(&mut chars);
            }
            Err(e) => {
                warn!(batch = batch_idx, error = %e, "批次校准失败，使用统计回退");
                // 优雅降级：基于统计生成基础结果
                let fallback: Vec<CalibratedCharacter> =
                    batch.iter().map(|s| fallback_calibrated(s)).collect();
                all_calibrated.extend(fallback);
            }
        }
    }

    Ok(all_calibrated)
}

/// 单批次 AI 校准。
async fn calibrate_batch(
    analyzer: &dyn Analyzer,
    pctx: &ProviderContext,
    batch: &[CharacterStats],
    script_context: &Option<String>,
) -> Result<Vec<CalibratedCharacter>> {
    let user_prompt = build_calibration_user_prompt(batch, script_context);

    let req = AnalysisRequest {
        system_prompt: Some(calibration_system_prompt()),
        user_prompt,
        images: vec![],
        model: None,
        response_format: AnalysisResponseFormat::Json,
    };

    let output = analyzer
        .analyze(req, pctx)
        .await
        .map_err(|e| anyhow::anyhow!("AI 校准失败: {e}"))?;

    // 解析 AI 返回的 JSON
    parse_calibration_response(&output.text).with_context(|| "解析 AI 校准结果失败")
}

fn calibration_system_prompt() -> String {
    r#"你是一个专业的影视角色分析专家。你的任务是分析剧本中的角色列表，完成以下工作：

1. **去重与合并**：识别同一角色的不同称呼（如"王总"和"投资人王总"是同一人），保留最正式的名称
2. **重要度分级**：
   - protagonist（主角）：故事的核心人物，出场最多，驱动剧情
   - supporting（重要配角）：频繁出场，对剧情有重要影响
   - minor（配角）：有名字但出场有限
   - extra（群演）：无名角色或仅作为背景
3. **属性补全**：根据上下文推断每个角色的性别、年龄范围、身份定位、人际关系
4. **名字变体记录**：记录同一角色在剧本中的不同称呼

请返回 JSON 格式，结构如下：
```json
{
  "characters": [
    {
      "name": "正式名称",
      "importance": "protagonist|supporting|minor|extra",
      "name_variants": ["变体1", "变体2"],
      "gender": "男|女|其他",
      "age": "年龄范围",
      "role": "身份/角色定位",
      "relationships": "人际关系描述"
    }
  ]
}
```

注意：
- 只返回 JSON，不要有其他文字
- 合并角色时，将被合并的名字放在 name_variants 中
- 确保每个角色名称只出现一次（去重后）
"#
    .into()
}

fn build_calibration_user_prompt(
    batch: &[CharacterStats],
    script_context: &Option<String>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ref ctx) = script_context {
        parts.push(format!("## 剧本背景\n{ctx}\n\n---\n"));
    }

    parts.push("## 角色统计列表\n\n".into());

    for s in batch {
        let episode_range = if s.first_episode == s.last_episode {
            format!("第{}集", s.first_episode + 1)
        } else {
            format!("第{}–{}集", s.first_episode + 1, s.last_episode + 1)
        };
        let dialogue_preview: String = s.dialogue_samples.first().cloned().unwrap_or_default();
        let scene_preview: String = s.scene_samples.first().cloned().unwrap_or_default();

        parts.push(format!(
            "- **{}**：出场 {} 次，对白 {} 次，范围 {}。",
            s.name, s.scene_count, s.dialogue_count, episode_range
        ));
        if !dialogue_preview.is_empty() {
            parts.push(format!(
                "  对白示例：「{}」",
                truncate_str(&dialogue_preview, 60)
            ));
        }
        if !scene_preview.is_empty() {
            parts.push(format!("  场次示例：{}", truncate_str(&scene_preview, 60)));
        }
    }

    parts.join("\n")
}

fn parse_calibration_response(json: &str) -> Result<Vec<CalibratedCharacter>> {
    // 尝试从 AI 返回中提取 JSON（可能包裹在 ```json 中）
    let json_str = extract_json_block(json);

    let v: Value = serde_json::from_str(&json_str).context("JSON 解析失败")?;

    let arr = v["characters"].as_array().context("缺少 characters 数组")?;

    let mut chars = Vec::new();
    for item in arr {
        let importance = match item["importance"].as_str().unwrap_or("minor") {
            "protagonist" => Importance::Protagonist,
            "supporting" => Importance::Supporting,
            "extra" => Importance::Extra,
            _ => Importance::Minor,
        };

        let name_variants: Vec<String> = item["name_variants"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        chars.push(CalibratedCharacter {
            id: String::new(), // 调用方填充
            name: item["name"].as_str().unwrap_or("").to_string(),
            importance,
            episode_range: None,
            appearance_count: 0,
            role: item["role"].as_str().map(String::from),
            age: item["age"].as_str().map(String::from),
            gender: item["gender"].as_str().map(String::from),
            relationships: item["relationships"].as_str().map(String::from),
            name_variants,
            visual_prompt_en: None,
            visual_prompt_zh: None,
            facial_features: None,
            unique_marks: None,
            clothing_style: None,
            identity_anchors: None,
            negative_prompt: None,
        });
    }

    Ok(chars)
}

// ============================================================================
// Step 4: 视觉锚点补全
// ============================================================================

/// 对主角和重要配角逐个调用 AI，生成 6 层视觉锚点。
async fn enrich_with_visual_anchors(
    analyzer: &dyn Analyzer,
    pctx: &ProviderContext,
    characters: &[CalibratedCharacter],
) -> Result<Vec<CalibratedCharacter>> {
    let mut enriched: Vec<CalibratedCharacter> = Vec::new();

    for c in characters {
        // 只对主角和重要配角做视觉锚点补全
        if !matches!(
            c.importance,
            Importance::Protagonist | Importance::Supporting
        ) {
            enriched.push(c.clone());
            continue;
        }

        info!(name = %c.name, "生成视觉锚点");

        match generate_anchors_for_character(analyzer, pctx, c).await {
            Ok(updated) => enriched.push(updated),
            Err(e) => {
                warn!(name = %c.name, error = %e, "视觉锚点生成失败，保留基础数据");
                enriched.push(c.clone());
            }
        }
    }

    Ok(enriched)
}

async fn generate_anchors_for_character(
    analyzer: &dyn Analyzer,
    pctx: &ProviderContext,
    character: &CalibratedCharacter,
) -> Result<CalibratedCharacter> {
    let user_prompt = build_anchor_request_prompt(character);

    let req = AnalysisRequest {
        system_prompt: Some(anchor_system_prompt()),
        user_prompt,
        images: vec![],
        model: None,
        response_format: AnalysisResponseFormat::Json,
    };

    let output = analyzer
        .analyze(req, pctx)
        .await
        .map_err(|e| anyhow::anyhow!("锚点生成失败: {e}"))?;

    let json_str = extract_json_block(&output.text);
    let v: Value = serde_json::from_str(&json_str).context("锚点 JSON 解析失败")?;

    let mut updated = character.clone();

    // 解析 6 层锚点
    let anchors = parse_anchors_from_json(&v);
    updated.identity_anchors = Some(anchors);

    // 视觉提示词
    updated.visual_prompt_en = v["visual_prompt_en"].as_str().map(String::from);
    updated.visual_prompt_zh = v["visual_prompt_zh"].as_str().map(String::from);
    updated.facial_features = v["facial_features"].as_str().map(String::from);
    updated.unique_marks = v["unique_marks"].as_str().map(String::from);
    updated.clothing_style = v["clothing_style"].as_str().map(String::from);

    // 负面提示词
    if let Some(avoids) = v["negative_prompt"]["avoid"].as_array() {
        updated.negative_prompt = Some(CharacterNegativePrompt {
            avoid: avoids
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            style_exclusions: v["negative_prompt"]["style_exclusions"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        });
    }

    Ok(updated)
}

fn anchor_system_prompt() -> String {
    r##"你是一个专业的角色视觉设计师。根据角色的基本信息和剧本定位，为角色生成详细的 6 层视觉锚点系统。

6 层锚点结构（用于 AI 图像生成中的角色一致性）：

① 骨相层 (Bone Structure):
  - faceShape: oval/square/heart/round/diamond/oblong
  - jawline: sharp angular/soft rounded/prominent
  - cheekbones: high prominent/subtle/wide set

② 五官层 (Facial Features):
  - eyeShape: almond/round/hooded/monolid/upturned
  - eyeDetails: 眼部细节描述（双眼皮、内眦赘皮等，用英文）
  - noseShape: 鼻型描述
  - lipShape: 唇型描述

③ 辨识标记层 (Distinctive Marks — 最强锚点):
  - uniqueMarks: 1-3 个独特标记，每个用英文精确描述位置和特征
    例如: "small mole 2cm below left eye", "faint diagonal scar on right eyebrow"

④ 色彩锚点层 (Color Anchors):
  - iris: 虹膜颜色 Hex（如 "#3D2314"）
  - hair: 发色 Hex（如 "#1A1A1A"）
  - skin: 肤色 Hex（如 "#E8C4A0"）
  - lips: 唇色 Hex（如 "#C4727E"）

⑤ 皮肤纹理层 (Skin Texture):
  - skinTexture: 英文描述皮肤质感

⑥ 发型锚点层 (Hairstyle):
  - hairStyle: 发型英文描述
  - hairlineDetails: 发际线细节英文描述

同时生成：
  - visual_prompt_en: 角色完整英文视觉描述（50-100 词）
  - visual_prompt_zh: 角色完整中文视觉描述
  - facial_features: 面部特征摘要
  - unique_marks: 独特标记摘要
  - clothing_style: 服装风格建议
  - negative_prompt: { avoid: [要避免的特征], style_exclusions: [要排除的风格] }

返回 JSON，不要任何其他文字。
"##
    .into()
}

fn build_anchor_request_prompt(character: &CalibratedCharacter) -> String {
    let mut parts = vec![
        format!("角色名：{}", character.name),
        format!("重要度：{:?}", character.importance),
    ];
    if let Some(ref gender) = character.gender {
        parts.push(format!("性别：{gender}"));
    }
    if let Some(ref age) = character.age {
        parts.push(format!("年龄：{age}"));
    }
    if let Some(ref role) = character.role {
        parts.push(format!("身份：{role}"));
    }
    if let Some(ref clothing) = character.clothing_style {
        parts.push(format!("服装风格：{clothing}"));
    }
    parts.join("\n")
}

fn parse_anchors_from_json(v: &Value) -> CharacterIdentityAnchors {
    let a = &v["identity_anchors"];

    CharacterIdentityAnchors {
        face_shape: str_val(a, "faceShape"),
        jawline: str_val(a, "jawline"),
        cheekbones: str_val(a, "cheekbones"),
        eye_shape: str_val(a, "eyeShape"),
        eye_details: str_val(a, "eyeDetails"),
        nose_shape: str_val(a, "noseShape"),
        lip_shape: str_val(a, "lipShape"),
        unique_marks: arr_val(a, "uniqueMarks"),
        color_anchors: {
            let ca = &a["colorAnchors"];
            if ca.is_object() {
                Some(artait_model::ColorAnchors {
                    iris: str_val(ca, "iris"),
                    hair: str_val(ca, "hair"),
                    skin: str_val(ca, "skin"),
                    lips: str_val(ca, "lips"),
                })
            } else {
                None
            }
        },
        skin_texture: str_val(a, "skinTexture"),
        hair_style: str_val(a, "hairStyle"),
        hairline_details: str_val(a, "hairlineDetails"),
    }
}

// ============================================================================
// 优雅降级：当 AI 调用失败时的回退
// ============================================================================

fn fallback_calibrated(stats: &CharacterStats) -> CalibratedCharacter {
    let importance = if stats.scene_count >= 10 {
        Importance::Protagonist
    } else if stats.scene_count >= 5 {
        Importance::Supporting
    } else if stats.scene_count >= 2 {
        Importance::Minor
    } else {
        Importance::Extra
    };

    CalibratedCharacter {
        id: uuid::Uuid::new_v4().to_string(),
        name: stats.name.clone(),
        importance,
        episode_range: Some((stats.first_episode, stats.last_episode)),
        appearance_count: stats.scene_count,
        role: None,
        age: None,
        gender: None,
        relationships: None,
        name_variants: vec![],
        visual_prompt_en: None,
        visual_prompt_zh: None,
        facial_features: None,
        unique_marks: None,
        clothing_style: None,
        identity_anchors: None,
        negative_prompt: None,
    }
}

// ============================================================================
// 工具函数
// ============================================================================

/// 从 AI 返回文本中提取 JSON（可能包裹在 ```json ... ``` 中）。
fn extract_json_block(text: &str) -> String {
    let text = text.trim();

    // 尝试提取 ```json ... ``` 包裹的内容
    if let Some(start) = text.find("```json") {
        let after_start = &text[start + 7..];
        if let Some(end) = after_start.find("```") {
            return after_start[..end].trim().to_string();
        }
    }
    if let Some(start) = text.find("```") {
        let after_start = &text[start + 3..];
        if let Some(end) = after_start.find("```") {
            return after_start[..end].trim().to_string();
        }
    }

    // 如果没有包裹，直接返回（假设是纯 JSON）
    text.to_string()
}

fn str_val(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn arr_val(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_len).collect::<String>())
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_stat(name: &str, scenes: u32, dialogues: u32, first: u32, last: u32) -> CharacterStats {
        CharacterStats {
            name: name.into(),
            scene_count: scenes,
            dialogue_count: dialogues,
            episodes: (first..=last).collect(),
            first_episode: first,
            last_episode: last,
            dialogue_samples: vec![],
            scene_samples: vec![],
        }
    }

    #[test]
    fn is_named_detects_generic_roles() {
        assert!(is_named_character("云中鹤"));
        assert!(is_named_character("李白"));
        assert!(!is_named_character("路人甲"));
        assert!(!is_named_character("士兵"));
        assert!(!is_named_character("群演1"));
    }

    #[test]
    fn sort_and_filter_strict_removes_minor() {
        let stats = vec![
            make_stat("主角", 20, 30, 0, 9),
            make_stat("路人甲", 1, 0, 0, 0),
            make_stat("配角", 5, 10, 2, 7),
        ];
        let result = sort_and_filter(&stats, CalibrationStrictness::Strict);
        // 路人甲被过滤（无实名 + 出场<2），主角排第一
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "主角");
        assert_eq!(result[1].name, "配角");
    }

    #[test]
    fn sort_and_filter_normal_keeps_named_only() {
        let stats = vec![
            make_stat("主角", 10, 20, 0, 5),
            make_stat("群演", 3, 0, 0, 1),
        ];
        let result = sort_and_filter(&stats, CalibrationStrictness::Normal);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "主角");
    }

    #[test]
    fn sort_and_filter_loose_keeps_all() {
        let stats = vec![
            make_stat("主角", 10, 20, 0, 5),
            make_stat("群演", 3, 0, 0, 1),
        ];
        let result = sort_and_filter(&stats, CalibrationStrictness::Loose);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn extract_json_from_code_block() {
        let text = "```json\n{\"a\":1}\n```";
        let result = extract_json_block(text);
        assert_eq!(result, "{\"a\":1}");
    }

    #[test]
    fn extract_plain_json() {
        let text = "{\"a\":1}";
        let result = extract_json_block(text);
        assert_eq!(result, "{\"a\":1}");
    }

    #[test]
    fn fallback_generates_basic_character() {
        let stat = make_stat("测试", 15, 25, 0, 9);
        let c = fallback_calibrated(&stat);
        assert_eq!(c.name, "测试");
        assert_eq!(c.importance, Importance::Protagonist);
        assert_eq!(c.appearance_count, 15);
    }

    #[test]
    fn parse_calibration_json_extracts_characters() {
        let json = r#"{
          "characters": [
            {"name": "云中鹤", "importance": "protagonist", "gender": "男", "age": "25岁", "role": "剑客", "relationships": "师从无名道长", "name_variants": ["小鹤"]}
          ]
        }"#;
        let result = parse_calibration_response(json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "云中鹤");
        assert_eq!(result[0].importance, Importance::Protagonist);
        assert_eq!(result[0].name_variants, vec!["小鹤"]);
    }
}
