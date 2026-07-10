//! 场景数据模型。
//!
//! 定义场景库的核心类型：场景、视角、多视角联合图、文件夹组织。
//! 场景是角色和分镜之间的桥梁——角色在场景中活动，分镜在场景中拍摄。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================================
// 场景主结构
// ============================================================================

/// 场景状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SceneStatus {
    /// 草稿：未关联剧本
    #[default]
    Draft,
    /// 已关联剧本
    Linked,
}

impl SceneStatus {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Draft => "草稿",
            Self::Linked => "已关联",
        }
    }
}

/// 场景重要性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneImportance {
    /// 主要场景：频繁出场
    Main,
    /// 次要场景
    Secondary,
    /// 过渡场景：仅出现一两次
    Transition,
}

impl SceneImportance {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Main => "主要场景",
            Self::Secondary => "次要场景",
            Self::Transition => "过渡场景",
        }
    }
}

/// 场景 —— 场景库主数据结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    /// 唯一 ID
    pub id: String,
    /// 场景名称
    pub name: String,

    // 基本属性
    /// 地点描述
    #[serde(default)]
    pub location: String,
    /// 时间：日/夜/晨/暮
    #[serde(default)]
    pub time_of_day: Option<String>,
    /// 氛围描述
    #[serde(default)]
    pub atmosphere: Option<String>,

    // 视觉设计
    /// 中文视觉描述
    #[serde(default)]
    pub visual_prompt_zh: Option<String>,
    /// 英文视觉描述（AI 生成）
    #[serde(default)]
    pub visual_prompt_en: Option<String>,
    /// 建筑风格
    #[serde(default)]
    pub architecture_style: Option<String>,
    /// 光影设计
    #[serde(default)]
    pub lighting_design: Option<String>,
    /// 色彩基调
    #[serde(default)]
    pub color_palette: Option<String>,
    /// 关键道具列表
    #[serde(default)]
    pub key_props: Vec<String>,
    /// 空间布局描述
    #[serde(default)]
    pub spatial_layout: Option<String>,
    /// 时代特征
    #[serde(default)]
    pub era_details: Option<String>,

    // 多视角
    /// 多视角联合图（原图 URL 或路径）
    #[serde(default)]
    pub contact_sheet_image: Option<String>,
    /// 视角列表
    #[serde(default)]
    pub viewpoints: Vec<SceneViewpoint>,

    // 关联
    /// 标签
    #[serde(default)]
    pub tags: Vec<String>,
    /// 备注
    #[serde(default)]
    pub notes: Option<String>,
    /// 缩略图 URL
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    /// 视觉风格预设 ID
    #[serde(default)]
    pub style_id: Option<String>,

    // 组织
    /// 所属文件夹 ID
    #[serde(default)]
    pub folder_id: Option<String>,
    /// 所属项目 ID
    #[serde(default)]
    pub project_id: Option<String>,
    /// 场景状态
    #[serde(default)]
    pub status: SceneStatus,
    /// 关联的剧本集 ID
    #[serde(default)]
    pub linked_episode_id: Option<String>,

    // 统计（AI 校准时填充）
    /// 出场集号
    #[serde(default)]
    pub episode_numbers: Vec<u32>,
    /// 出场次数
    #[serde(default)]
    pub appearance_count: u32,
    /// 场景重要性
    #[serde(default)]
    pub importance: Option<SceneImportance>,

    // 时间戳
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Scene {
    pub fn new(id: String, name: String) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            location: String::new(),
            time_of_day: None,
            atmosphere: None,
            visual_prompt_zh: None,
            visual_prompt_en: None,
            architecture_style: None,
            lighting_design: None,
            color_palette: None,
            key_props: vec![],
            spatial_layout: None,
            era_details: None,
            contact_sheet_image: None,
            viewpoints: vec![],
            tags: vec![],
            notes: None,
            thumbnail_url: None,
            style_id: None,
            folder_id: None,
            project_id: None,
            status: SceneStatus::Draft,
            linked_episode_id: None,
            episode_numbers: vec![],
            appearance_count: 0,
            importance: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// 获取主视觉提示词（英文优先）。
    pub fn primary_visual_prompt(&self) -> Option<&str> {
        self.visual_prompt_en
            .as_deref()
            .or(self.visual_prompt_zh.as_deref())
    }

    /// 视角数量。
    pub fn viewpoint_count(&self) -> usize {
        self.viewpoints.len()
    }
}

// ============================================================================
// 场景视角
// ============================================================================

/// 场景视角 —— 同一场景的不同拍摄角度。
///
/// 通过多视角联合图技术，一次生成 6 个视角的联合图，
/// 然后自动切割为独立图片，保证场景背景一致性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneViewpoint {
    /// 视角 ID，如 "dining"、"sofa"、"window"
    pub id: String,
    /// 中文名：餐桌区、沙发区、窗边
    pub name: String,
    /// 英文名
    #[serde(default)]
    pub name_en: String,
    /// 关联的分镜 ID 列表
    #[serde(default)]
    pub shot_ids: Vec<String>,
    /// 该视角需要的道具
    #[serde(default)]
    pub key_props: Vec<String>,
    /// 在联合图中的位置（0-5，对应 3×2 网格）
    pub grid_index: u32,
    /// 生成的图片 URL 或路径
    #[serde(default)]
    pub image_url: Option<String>,
    /// 生成时间
    #[serde(default)]
    pub generated_at: Option<DateTime<Utc>>,
}

/// 多视角联合图布局。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactSheetLayout {
    /// 3 列 × 2 行（6 个视角）
    Grid3x2,
    /// 2 列 × 2 行（4 个视角）
    Grid2x2,
    /// 2 列 × 3 行（6 个视角）
    Grid2x3,
}

impl ContactSheetLayout {
    pub fn columns(self) -> u32 {
        match self {
            Self::Grid3x2 | Self::Grid2x3 => 3,
            Self::Grid2x2 => 2,
        }
    }

    pub fn rows(self) -> u32 {
        match self {
            Self::Grid3x2 => 2,
            Self::Grid2x2 => 2,
            Self::Grid2x3 => 3,
        }
    }

    pub fn total_cells(self) -> u32 {
        self.columns() * self.rows()
    }
}

// ============================================================================
// 场景文件夹
// ============================================================================

/// 场景文件夹。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub is_auto_created: bool,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_scene_defaults() {
        let s = Scene::new("s1".into(), "酒馆大厅".into());
        assert_eq!(s.name, "酒馆大厅");
        assert_eq!(s.status, SceneStatus::Draft);
        assert!(s.viewpoints.is_empty());
    }

    #[test]
    fn primary_visual_prompt_prefers_en() {
        let mut s = Scene::new("s1".into(), "大厅".into());
        s.visual_prompt_en = Some("a grand hall with marble columns".into());
        s.visual_prompt_zh = Some("大理石柱的宏伟厅堂".into());
        assert_eq!(
            s.primary_visual_prompt(),
            Some("a grand hall with marble columns")
        );
    }

    #[test]
    fn contact_sheet_3x2_has_6_cells() {
        assert_eq!(ContactSheetLayout::Grid3x2.total_cells(), 6);
        assert_eq!(ContactSheetLayout::Grid3x2.columns(), 3);
        assert_eq!(ContactSheetLayout::Grid3x2.rows(), 2);
    }

    #[test]
    fn contact_sheet_2x2_has_4_cells() {
        assert_eq!(ContactSheetLayout::Grid2x2.total_cells(), 4);
    }

    #[test]
    fn scene_serialization_roundtrip() {
        let mut s = Scene::new("sid".into(), "测试场景".into());
        s.location = "古代酒馆".into();
        s.time_of_day = Some("夜".into());
        s.atmosphere = Some("热闹喧哗".into());
        s.viewpoints.push(SceneViewpoint {
            id: "bar".into(),
            name: "吧台区".into(),
            name_en: "Bar Area".into(),
            shot_ids: vec![],
            key_props: vec!["酒壶".into()],
            grid_index: 0,
            image_url: None,
            generated_at: None,
        });

        let json = serde_json::to_string(&s).unwrap();
        let back: Scene = serde_json::from_str(&json).unwrap();
        assert_eq!(back.location, "古代酒馆");
        assert_eq!(back.viewpoints.len(), 1);
        assert_eq!(back.viewpoints[0].name, "吧台区");
    }
}
