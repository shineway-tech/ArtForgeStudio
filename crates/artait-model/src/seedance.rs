//! Seedance 视频生成数据模型。
//!
//! 定义 Seedance 2.0 视频生成所需的类型：多模态引用、
//! 内联参数编码、S-Class 场景、3层提示词融合。

use serde::{Deserialize, Serialize};

// ============================================================================
// 多模态引用
// ============================================================================

/// 多模态引用类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaRefType {
    /// 首帧图片 —— 视频从这张图开始
    FirstFrame,
    /// 尾帧图片 —— 视频在这张图结束
    LastFrame,
    /// 参考视频 —— 动作/风格参考
    ReferenceVideo,
    /// 参考音频 —— BGM/节奏参考
    ReferenceAudio,
}

/// 多模态引用 —— @Image / @Video / @Audio 的内容项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaReference {
    /// 引用类型
    pub ref_type: MediaRefType,
    /// 媒体 URL
    pub url: String,
    /// 可选描述
    #[serde(default)]
    pub description: Option<String>,
}

// ============================================================================
// Seedance 视频参数
// ============================================================================

/// Seedance 视频生成参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedanceVideoParams {
    /// 模型名称
    pub model: String,
    /// 正向提示词（动作描述 + 镜头语言）
    pub prompt: String,
    /// 分辨率：480p / 720p / 1080p
    #[serde(default = "default_resolution")]
    pub resolution: String,
    /// 宽高比：16:9 / 9:16 / 1:1 / 4:3 / 3:4 / 21:9 / 9:21
    #[serde(default = "default_aspect")]
    pub aspect_ratio: String,
    /// 时长（秒）：4-15
    #[serde(default = "default_duration")]
    pub duration_secs: u32,
    /// 镜头是否固定（不自动运镜）
    #[serde(default)]
    pub camera_fixed: bool,
    /// 是否生成音频（口型同步）
    #[serde(default)]
    pub enable_audio: bool,
    /// 多模态引用列表
    #[serde(default)]
    pub references: Vec<MediaReference>,
    /// 负面提示词
    #[serde(default)]
    pub negative_prompt: Option<String>,
    /// 生成数量
    #[serde(default = "default_count")]
    pub count: u32,
}

fn default_resolution() -> String {
    "720p".into()
}
fn default_aspect() -> String {
    "16:9".into()
}
fn default_duration() -> u32 {
    5
}
fn default_count() -> u32 {
    1
}

impl Default for SeedanceVideoParams {
    fn default() -> Self {
        Self {
            model: "doubao-seedance-1-5-pro-251215".into(),
            prompt: String::new(),
            resolution: default_resolution(),
            aspect_ratio: default_aspect(),
            duration_secs: default_duration(),
            camera_fixed: false,
            enable_audio: false,
            references: vec![],
            negative_prompt: None,
            count: 1,
        }
    }
}

impl SeedanceVideoParams {
    /// 将参数编码为内联 Token 字符串（MemeFast 代理格式）。
    /// 格式：`prompt --rs 720p --rt 16:9 --dur 5 --cf false`
    pub fn encode_inline_tokens(&self) -> String {
        let mut tokens = Vec::new();
        tokens.push(format!("--rs {}", self.resolution));
        tokens.push(format!("--rt {}", self.aspect_ratio));
        tokens.push(format!("--dur {}", self.duration_secs));
        tokens.push(format!("--cf {}", self.camera_fixed));
        if self.count > 1 {
            tokens.push(format!("--count {}", self.count));
        }
        format!("{} {}", self.prompt, tokens.join(" "))
    }

    /// 约束校验：≤9 张图 + ≤3 个视频 + ≤3 个音频
    pub fn validate_constraints(&self) -> Result<(), String> {
        let image_count = self
            .references
            .iter()
            .filter(|r| {
                matches!(
                    r.ref_type,
                    MediaRefType::FirstFrame | MediaRefType::LastFrame
                )
            })
            .count();
        let video_count = self
            .references
            .iter()
            .filter(|r| matches!(r.ref_type, MediaRefType::ReferenceVideo))
            .count();
        let audio_count = self
            .references
            .iter()
            .filter(|r| matches!(r.ref_type, MediaRefType::ReferenceAudio))
            .count();

        if image_count > 9 {
            return Err(format!("图片引用超过限制：{} > 9", image_count));
        }
        if video_count > 3 {
            return Err(format!("视频引用超过限制：{} > 3", video_count));
        }
        if audio_count > 3 {
            return Err(format!("音频引用超过限制：{} > 3", audio_count));
        }
        Ok(())
    }
}

// ============================================================================
// Seedance 模型注册表
// ============================================================================

/// Seedance 可用模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedanceModel {
    /// Seedance Lite T2V —— 轻量文本生视频
    LiteT2v,
    /// Seedance Pro T2V —— 专业文本生视频
    ProT2v,
    /// Seedance Pro T2V Fast —— 快速专业文本生视频
    ProT2vFast,
    /// Seedance 1.5 Pro T2V —— 最新专业文本生视频
    V15ProT2v,
    /// Seedance 1.5 Pro T2V Fast —— 最新快速专业文本生视频
    V15ProT2vFast,
}

impl SeedanceModel {
    /// 所有可用模型列表。
    pub fn all() -> &'static [SeedanceModel] {
        &[
            Self::LiteT2v,
            Self::ProT2v,
            Self::ProT2vFast,
            Self::V15ProT2v,
            Self::V15ProT2vFast,
        ]
    }

    /// 模型显示名称。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::LiteT2v => "Seedance Lite (T2V)",
            Self::ProT2v => "Seedance Pro (T2V)",
            Self::ProT2vFast => "Seedance Pro Fast (T2V)",
            Self::V15ProT2v => "Seedance 1.5 Pro (T2V)",
            Self::V15ProT2vFast => "Seedance 1.5 Pro Fast (T2V)",
        }
    }

    /// API 模型 ID。
    pub fn api_model_id(self) -> &'static str {
        match self {
            Self::LiteT2v => "doubao-seedance-lite-t2v",
            Self::ProT2v => "doubao-seedance-pro-t2v",
            Self::ProT2vFast => "doubao-seedance-pro-t2v-fast",
            Self::V15ProT2v => "doubao-seedance-1-5-pro-251215",
            Self::V15ProT2vFast => "doubao-seedance-1-5-pro-251215-fast",
        }
    }
}

// ============================================================================
// S-Class 场景（Seedance 2.0 多镜头叙事）
// ============================================================================

/// S-Class 场景 —— Seedance 2.0 多镜头融合视频的最小单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SClassScene {
    /// 唯一 ID
    pub id: String,
    /// 场景名称
    pub name: String,
    /// 动作描述（第一层：剧情动作）
    #[serde(default)]
    pub action_prompt: String,
    /// 镜头语言（第二层：摄影参数）
    #[serde(default)]
    pub cinematography_prompt: String,
    /// 口型/对白（第三层：音频同步）
    #[serde(default)]
    pub lip_sync_text: Option<String>,
    /// 视频参数
    #[serde(default)]
    pub video_params: SeedanceVideoParams,
    /// 生成的视频 URL
    #[serde(default)]
    pub generated_video_url: Option<String>,
    /// 生成状态
    #[serde(default)]
    pub status: SClassSceneStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SClassSceneStatus {
    #[default]
    Idle,
    Generating,
    Completed,
    Failed,
}

impl SClassScene {
    /// 构建 3 层融合提示词。
    pub fn build_fused_prompt(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.action_prompt.is_empty() {
            parts.push(self.action_prompt.clone());
        }
        if !self.cinematography_prompt.is_empty() {
            parts.push(self.cinematography_prompt.clone());
        }
        if let Some(ref lip) = self.lip_sync_text {
            parts.push(format!("dialogue: {}", lip));
        }
        parts.join(". ")
    }
}

// ============================================================================
// N×N 宫格拼接
// ============================================================================

/// 宫格布局配置 —— 多镜头首帧拼接。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridLayout {
    /// 列数
    pub columns: u32,
    /// 行数
    pub rows: u32,
    /// 每个格子的宽高比
    #[serde(default = "default_grid_aspect")]
    pub cell_aspect: String,
}

fn default_grid_aspect() -> String {
    "16:9".into()
}

impl Default for GridLayout {
    fn default() -> Self {
        Self {
            columns: 2,
            rows: 1,
            cell_aspect: "16:9".into(),
        }
    }
}

impl GridLayout {
    pub fn total_cells(&self) -> u32 {
        self.columns * self.rows
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_token_encoding() {
        let params = SeedanceVideoParams {
            prompt: "a warrior walking through snow".into(),
            resolution: "720p".into(),
            aspect_ratio: "16:9".into(),
            duration_secs: 5,
            camera_fixed: false,
            ..Default::default()
        };
        let encoded = params.encode_inline_tokens();
        assert!(encoded.contains("--rs 720p"));
        assert!(encoded.contains("--rt 16:9"));
        assert!(encoded.contains("--dur 5"));
        assert!(encoded.contains("--cf false"));
        assert!(encoded.starts_with("a warrior walking"));
    }

    #[test]
    fn constraint_validation_passes() {
        let params = SeedanceVideoParams {
            references: vec![
                MediaReference {
                    ref_type: MediaRefType::FirstFrame,
                    url: "img1.png".into(),
                    description: None,
                },
                MediaReference {
                    ref_type: MediaRefType::ReferenceVideo,
                    url: "vid1.mp4".into(),
                    description: None,
                },
            ],
            ..Default::default()
        };
        assert!(params.validate_constraints().is_ok());
    }

    #[test]
    fn constraint_validation_fails_on_too_many_images() {
        let refs: Vec<MediaReference> = (0..10)
            .map(|i| MediaReference {
                ref_type: MediaRefType::FirstFrame,
                url: format!("img{}.png", i),
                description: None,
            })
            .collect();
        let params = SeedanceVideoParams {
            references: refs,
            ..Default::default()
        };
        assert!(params.validate_constraints().is_err());
    }

    #[test]
    fn sclass_build_fused_prompt() {
        let scene = SClassScene {
            id: "s1".into(),
            name: "开场".into(),
            action_prompt: "主角在雨中奔跑".into(),
            cinematography_prompt: "跟拍镜头，低角度，快速运动".into(),
            lip_sync_text: Some("等等我！".into()),
            video_params: Default::default(),
            generated_video_url: None,
            status: SClassSceneStatus::Idle,
        };
        let fused = scene.build_fused_prompt();
        assert!(fused.contains("奔跑"));
        assert!(fused.contains("跟拍"));
        assert!(fused.contains("等等我"));
    }

    #[test]
    fn seedance_models_all_have_ids() {
        for m in SeedanceModel::all() {
            assert!(!m.api_model_id().is_empty());
            assert!(!m.display_name().is_empty());
        }
    }
}
