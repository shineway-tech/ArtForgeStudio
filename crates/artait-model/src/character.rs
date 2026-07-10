//! 角色系统数据模型。
//!
//! 包含角色的完整定义、6 层身份锚点、视图、变体（衣柜）、
//! 文件夹组织以及 AI 角色校准相关的所有类型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================================
// 6 层身份锚点 —— 跨场景角色一致性的核心机制
// ============================================================================

/// 6 层身份锚点，用于在 AI 图像生成中保持角色外观一致性。
///
/// 锚点分为 6 层，按强度递减：
/// ① 骨相 → ② 五官 → ③ 辨识标记（最强） → ④ 色彩 → ⑤ 皮肤纹理 → ⑥ 发型
///
/// 有参考图时只用 ③ + ④，无参考图时六层全锁。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterIdentityAnchors {
    // ① 骨相层
    /// 脸型：oval / square / heart / round / diamond / oblong
    #[serde(default)]
    pub face_shape: Option<String>,
    /// 下颌：sharp angular / soft rounded / prominent
    #[serde(default)]
    pub jawline: Option<String>,
    /// 颧骨：high prominent / subtle / wide set
    #[serde(default)]
    pub cheekbones: Option<String>,

    // ② 五官层
    /// 眼型：almond / round / hooded / monolid / upturned
    #[serde(default)]
    pub eye_shape: Option<String>,
    /// 眼部细节："double eyelids, slight epicanthic fold"
    #[serde(default)]
    pub eye_details: Option<String>,
    /// 鼻型："straight bridge, rounded tip"
    #[serde(default)]
    pub nose_shape: Option<String>,
    /// 唇型："full lips, defined cupid's bow"
    #[serde(default)]
    pub lip_shape: Option<String>,

    // ③ 辨识标记层 —— 最强锚点
    /// 独特标记，例如 ["small mole 2cm below left eye", "faint scar on right eyebrow"]
    #[serde(default)]
    pub unique_marks: Vec<String>,

    // ④ 色彩锚点层
    /// 色彩锚点（Hex 值），例如 iris: "#3D2314"
    #[serde(default)]
    pub color_anchors: Option<ColorAnchors>,

    // ⑤ 皮肤纹理层
    /// 皮肤纹理："visible pores, light smile lines"
    #[serde(default)]
    pub skin_texture: Option<String>,

    // ⑥ 发型锚点层
    /// 发型："shoulder-length layered, side-parted"
    #[serde(default)]
    pub hair_style: Option<String>,
    /// 发际线细节："natural hairline, slight widow's peak"
    #[serde(default)]
    pub hairline_details: Option<String>,
}

impl CharacterIdentityAnchors {
    /// 检查是否所有锚点字段都为空。
    pub fn is_empty(&self) -> bool {
        self.face_shape.is_none()
            && self.jawline.is_none()
            && self.cheekbones.is_none()
            && self.eye_shape.is_none()
            && self.eye_details.is_none()
            && self.nose_shape.is_none()
            && self.lip_shape.is_none()
            && self.unique_marks.is_empty()
            && self.color_anchors.is_none()
            && self.skin_texture.is_none()
            && self.hair_style.is_none()
            && self.hairline_details.is_none()
    }

    /// 是否有唯一标记（第③层，最强锚点）。
    pub fn has_unique_marks(&self) -> bool {
        !self.unique_marks.is_empty()
    }
}

/// 色彩锚点 —— 角色的固定色调参考（Hex 颜色值）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ColorAnchors {
    /// 虹膜颜色，如 "#3D2314" (dark brown)
    #[serde(default)]
    pub iris: Option<String>,
    /// 发色，如 "#1A1A1A" (jet black)
    #[serde(default)]
    pub hair: Option<String>,
    /// 肤色，如 "#E8C4A0" (warm beige)
    #[serde(default)]
    pub skin: Option<String>,
    /// 唇色，如 "#C4727E" (dusty rose)
    #[serde(default)]
    pub lips: Option<String>,
}

// ============================================================================
// 角色视图 — 多角度生成图片
// ============================================================================

/// 角色视图类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewType {
    /// 正面
    Front,
    /// 侧面
    Side,
    /// 背面
    Back,
    /// 四分之三侧面
    ThreeQuarter,
}

impl ViewType {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Front => "正面",
            Self::Side => "侧面",
            Self::Back => "背面",
            Self::ThreeQuarter => "3/4 侧",
        }
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::Front => "front view",
            Self::Side => "side view",
            Self::Back => "back view",
            Self::ThreeQuarter => "three-quarter view",
        }
    }
}

/// 角色生成视图 —— 记录一次多角度或单视角生成结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterView {
    /// 视图类型
    pub view_type: ViewType,
    /// 图片 URL 或本地路径
    pub image_url: String,
    /// 生成时间
    pub generated_at: DateTime<Utc>,
}

// ============================================================================
// 角色变体（衣柜系统）
// ============================================================================

/// 角色变体 —— 衣柜 / 阶段造型 / 服装变体。
///
/// 用于管理同一角色在不同场景/阶段/服装下的外观变化。
/// 支持阶段变体（随剧情发展角色造型变化）和服装变体（换装）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterVariation {
    /// 唯一 ID
    pub id: String,
    /// 变体名称，如 "日常装"、"战斗装"、"少年时期"
    pub name: String,
    /// 英文视觉提示词
    pub visual_prompt: String,
    /// 中文视觉提示词
    #[serde(default)]
    pub visual_prompt_zh: Option<String>,
    /// 生成的参考图 URL
    #[serde(default)]
    pub reference_image: Option<String>,
    /// 服装参考图 URL 列表（最多 3 张）
    #[serde(default)]
    pub clothing_reference_images: Vec<String>,
    /// 生成时间
    #[serde(default)]
    pub generated_at: Option<DateTime<Utc>>,

    // 阶段变体字段
    /// 是否为阶段变体（剧本时间跨度导致的造型变化）
    #[serde(default)]
    pub is_stage_variation: bool,
    /// 集数范围 (start, end)，从第 start 集到第 end 集
    #[serde(default)]
    pub episode_range: Option<(u32, u32)>,
    /// 该阶段的年龄描述
    #[serde(default)]
    pub age_description: Option<String>,
    /// 该阶段的造型描述
    #[serde(default)]
    pub stage_description: Option<String>,
}

// ============================================================================
// 负面提示词
// ============================================================================

/// 角色生成负面提示词，指定不希望出现在生成图中的元素。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterNegativePrompt {
    /// 需要避免的特征，如 ["glasses", "beard", "hat"]
    #[serde(default)]
    pub avoid: Vec<String>,
    /// 需要排除的风格，如 ["photorealistic", "3d render"]
    #[serde(default)]
    pub style_exclusions: Vec<String>,
}

// ============================================================================
// 角色主结构
// ============================================================================

/// 角色状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CharacterStatus {
    /// 草稿：尚未关联剧本
    #[default]
    Draft,
    /// 已关联：通过 AI 校准关联到剧本角色
    Linked,
}

impl CharacterStatus {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Draft => "草稿",
            Self::Linked => "已关联",
        }
    }
}

/// 角色 —— 角色库主数据结构。
///
/// 包含角色的完整信息：基本属性、视觉描述、6 层锚点、
/// 生成的视图、变体（衣柜）、组织归属等。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    /// 唯一 ID
    pub id: String,
    /// 角色名
    pub name: String,

    // ---- 基本属性 ----
    /// 性别
    #[serde(default)]
    pub gender: Option<String>,
    /// 年龄 / 年龄范围
    #[serde(default)]
    pub age: Option<String>,
    /// 性格描写
    #[serde(default)]
    pub personality: Option<String>,
    /// 角色身份 / 背景
    #[serde(default)]
    pub role: Option<String>,
    /// 核心特质
    #[serde(default)]
    pub traits: Option<String>,
    /// 技能 / 能力
    #[serde(default)]
    pub skills: Option<String>,
    /// 关键行为 / 事迹
    #[serde(default)]
    pub key_actions: Option<String>,
    /// 外观描述
    #[serde(default)]
    pub appearance: Option<String>,
    /// 主要人际关系
    #[serde(default)]
    pub relationships: Option<String>,
    /// 标签列表，如 ["#武侠", "#男主", "#剑客"]
    #[serde(default)]
    pub tags: Vec<String>,
    /// 备注
    #[serde(default)]
    pub notes: Option<String>,

    // ---- 视觉描述 ----
    /// 英文视觉提示词
    #[serde(default)]
    pub visual_prompt_en: Option<String>,
    /// 中文视觉提示词
    #[serde(default)]
    pub visual_prompt_zh: Option<String>,
    /// 综合描述
    #[serde(default)]
    pub description: Option<String>,

    // ---- 一致性系统 ----
    /// 6 层身份锚点
    #[serde(default)]
    pub identity_anchors: Option<CharacterIdentityAnchors>,
    /// 负面提示词
    #[serde(default)]
    pub negative_prompt: Option<CharacterNegativePrompt>,

    // ---- 关联数据 ----
    /// 已生成的视图列表（多角度图片）
    #[serde(default)]
    pub views: Vec<CharacterView>,
    /// 变体列表（衣柜 / 阶段造型）
    #[serde(default)]
    pub variations: Vec<CharacterVariation>,
    /// 主缩略图 URL
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    /// 用户上传的参考图 URL 列表
    #[serde(default)]
    pub reference_images: Vec<String>,
    /// 视觉风格预设 ID
    #[serde(default)]
    pub style_id: Option<String>,

    // ---- 组织 ----
    /// 所属文件夹 ID
    #[serde(default)]
    pub folder_id: Option<String>,
    /// 所属项目 ID
    #[serde(default)]
    pub project_id: Option<String>,
    /// 角色状态
    #[serde(default)]
    pub status: CharacterStatus,
    /// 关联的剧本集 ID（Linked 状态下关联到特定集）
    #[serde(default)]
    pub linked_episode_id: Option<String>,

    // ---- 时间戳 ----
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后更新时间
    pub updated_at: DateTime<Utc>,
}

impl Character {
    /// 创建一个新的草稿角色。
    pub fn new(id: String, name: String) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            gender: None,
            age: None,
            personality: None,
            role: None,
            traits: None,
            skills: None,
            key_actions: None,
            appearance: None,
            relationships: None,
            tags: Vec::new(),
            notes: None,
            visual_prompt_en: None,
            visual_prompt_zh: None,
            description: None,
            identity_anchors: None,
            negative_prompt: None,
            views: Vec::new(),
            variations: Vec::new(),
            thumbnail_url: None,
            reference_images: Vec::new(),
            style_id: None,
            folder_id: None,
            project_id: None,
            status: CharacterStatus::Draft,
            linked_episode_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// 获取角色被引用的基础视觉提示词（英文优先，回退中文）。
    pub fn primary_visual_prompt(&self) -> Option<&str> {
        self.visual_prompt_en
            .as_deref()
            .or(self.visual_prompt_zh.as_deref())
    }

    /// 获取变体数量。
    pub fn variation_count(&self) -> usize {
        self.variations.len()
    }

    /// 获取视图数量。
    pub fn view_count(&self) -> usize {
        self.views.len()
    }
}

// ============================================================================
// 角色文件夹
// ============================================================================

/// 角色文件夹 —— 用于在角色库中组织角色。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterFolder {
    /// 唯一 ID
    pub id: String,
    /// 文件夹名称
    pub name: String,
    /// 父文件夹 ID（None 表示根级）
    #[serde(default)]
    pub parent_id: Option<String>,
    /// 所属项目 ID（None 表示跨项目共享）
    #[serde(default)]
    pub project_id: Option<String>,
    /// 是否为自动创建的（如按项目自动生成）
    #[serde(default)]
    pub is_auto_created: bool,
    /// 创建时间
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// 角色阶段信息
// ============================================================================

/// 角色阶段信息 —— 描述角色在某段剧情中的状态。
///
/// 与 CharacterVariation 不同，这是轻量级的阶段描述，
/// 用于 ScriptCharacter 中的角色在特定集数范围的属性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterStageInfo {
    /// 阶段名称，如 "少年时期"、"成为掌门后"
    #[serde(default)]
    pub stage_name: Option<String>,
    /// 集数范围 (start, end)
    #[serde(default)]
    pub episode_range: Option<(u32, u32)>,
    /// 该阶段的年龄描述
    #[serde(default)]
    pub age_description: Option<String>,
}

/// 跨阶段共享的一致性元素。
///
/// 这些特征在角色的所有阶段变体中保持一致，
/// 是确保同一角色跨场次外观统一的关键。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterConsistencyElements {
    /// 面部特征描述
    #[serde(default)]
    pub facial_features: Option<String>,
    /// 体型描述
    #[serde(default)]
    pub body_type: Option<String>,
    /// 独特标记描述
    #[serde(default)]
    pub unique_marks: Option<String>,
}

// ============================================================================
// AI 角色校准相关类型
// ============================================================================

/// 角色重要度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    /// 群演 / 背景角色
    Extra,
    /// 配角
    Minor,
    /// 重要配角
    Supporting,
    /// 主角
    Protagonist,
}

impl Importance {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Extra => "群演",
            Self::Minor => "配角",
            Self::Supporting => "重要配角",
            Self::Protagonist => "主角",
        }
    }
}

/// AI 角色校准的完整结果。
///
/// 由 `character_calibrator` 的 4 步流水线产出：
/// 提取 → 统计 → 批量校准 → 视觉锚点补全
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterCalibrationResult {
    /// 校准后的角色列表
    pub characters: Vec<CalibratedCharacter>,
    /// 被过滤的非角色词汇
    #[serde(default)]
    pub filtered_words: Vec<String>,
    /// 被过滤的角色及其原因
    #[serde(default)]
    pub filtered_characters: Vec<FilteredCharacterRecord>,
    /// 角色合并记录（如 "王总" + "投资人王总" → "王总"）
    #[serde(default)]
    pub merge_records: Vec<MergeRecord>,
    /// 分析备注
    #[serde(default)]
    pub analysis_notes: String,
}

/// AI 校准后的角色信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibratedCharacter {
    /// 唯一 ID
    pub id: String,
    /// 角色名
    pub name: String,
    /// 重要度
    pub importance: Importance,
    /// 出场集数范围
    #[serde(default)]
    pub episode_range: Option<(u32, u32)>,
    /// 出场次数
    #[serde(default)]
    pub appearance_count: u32,
    /// 身份 / 角色定位
    #[serde(default)]
    pub role: Option<String>,
    /// 年龄
    #[serde(default)]
    pub age: Option<String>,
    /// 性别
    #[serde(default)]
    pub gender: Option<String>,
    /// 人际关系
    #[serde(default)]
    pub relationships: Option<String>,
    /// 名字变体（同一角色的不同称呼）
    #[serde(default)]
    pub name_variants: Vec<String>,
    /// 英文视觉提示词
    #[serde(default)]
    pub visual_prompt_en: Option<String>,
    /// 中文视觉提示词
    #[serde(default)]
    pub visual_prompt_zh: Option<String>,
    /// 面部特征
    #[serde(default)]
    pub facial_features: Option<String>,
    /// 独特标记
    #[serde(default)]
    pub unique_marks: Option<String>,
    /// 服装风格
    #[serde(default)]
    pub clothing_style: Option<String>,
    /// 6 层身份锚点（由 AI 补全）
    #[serde(default)]
    pub identity_anchors: Option<CharacterIdentityAnchors>,
    /// 负面提示词
    #[serde(default)]
    pub negative_prompt: Option<CharacterNegativePrompt>,
}

/// 被过滤的角色记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilteredCharacterRecord {
    /// 被过滤的名称
    pub name: String,
    /// 过滤原因
    pub reason: String,
}

/// 角色合并记录 —— 将多个名称变体合并为一个角色。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeRecord {
    /// 合并前的多个名称
    pub from: Vec<String>,
    /// 合并后的名称
    pub to: String,
    /// 合并原因
    pub reason: String,
}

/// 角色统计信息 —— 从剧本中收集的角色出场数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterStats {
    /// 角色名
    pub name: String,
    /// 出场场次数量
    pub scene_count: u32,
    /// 对白数量
    pub dialogue_count: u32,
    /// 出场的集数列表
    #[serde(default)]
    pub episodes: Vec<u32>,
    /// 首次出场集数
    pub first_episode: u32,
    /// 最后出场集数
    pub last_episode: u32,
    /// 对白采样（前 3 条）
    #[serde(default)]
    pub dialogue_samples: Vec<String>,
    /// 场次采样（前 3 条）
    #[serde(default)]
    pub scene_samples: Vec<String>,
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_character_defaults() {
        let c = Character::new("c1".into(), "小明".into());
        assert_eq!(c.name, "小明");
        assert_eq!(c.status, CharacterStatus::Draft);
        assert!(c.identity_anchors.is_none());
        assert!(c.views.is_empty());
        assert!(c.variations.is_empty());
    }

    #[test]
    fn anchors_empty_by_default() {
        let a = CharacterIdentityAnchors::default();
        assert!(a.is_empty());
        assert!(!a.has_unique_marks());
    }

    #[test]
    fn anchors_with_marks() {
        let a = CharacterIdentityAnchors {
            unique_marks: vec!["mole on chin".into()],
            ..Default::default()
        };
        assert!(!a.is_empty());
        assert!(a.has_unique_marks());
    }

    #[test]
    fn primary_visual_prompt_prefers_en() {
        let c = Character {
            visual_prompt_en: Some("a young swordsman".into()),
            visual_prompt_zh: Some("年轻剑客".into()),
            ..new_test_char()
        };
        assert_eq!(c.primary_visual_prompt(), Some("a young swordsman"));
    }

    #[test]
    fn primary_visual_prompt_fallback_zh() {
        let c = Character {
            visual_prompt_en: None,
            visual_prompt_zh: Some("年轻剑客".into()),
            ..new_test_char()
        };
        assert_eq!(c.primary_visual_prompt(), Some("年轻剑客"));
    }

    #[test]
    fn calibrate_character_serialization() {
        let cc = CalibratedCharacter {
            id: "cc1".into(),
            name: "李白".into(),
            importance: Importance::Protagonist,
            episode_range: Some((1, 10)),
            appearance_count: 42,
            role: Some("诗人剑客".into()),
            age: Some("25".into()),
            gender: Some("男".into()),
            relationships: Some("杜甫（挚友）、王维（相识）".into()),
            name_variants: vec!["李太白".into(), "诗仙".into()],
            visual_prompt_en: Some("a tall poet with long black hair".into()),
            visual_prompt_zh: Some("一位长发飘逸的高个子诗人".into()),
            facial_features: Some("剑眉星目".into()),
            unique_marks: Some("左手虎口有剑茧".into()),
            clothing_style: Some("白色长衫".into()),
            identity_anchors: None,
            negative_prompt: None,
        };

        let json = serde_json::to_string_pretty(&cc).unwrap();
        let back: CalibratedCharacter = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "李白");
        assert_eq!(back.importance, Importance::Protagonist);
    }

    fn new_test_char() -> Character {
        Character::new("test".into(), "测试".into())
    }
}
