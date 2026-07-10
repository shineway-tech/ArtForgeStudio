//! 电影摄影参数定义。
//!
//! 提供专业影视级别的镜头参数预设：景别、机位角度、
//! 镜头运动、焦距等，供分镜编辑器使用。

use serde::{Deserialize, Serialize};

// ============================================================================
// 景别 (Shot Size)
// ============================================================================

/// 景别 —— 被摄主体在画面中的大小范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotSize {
    /// 极远景 —— 人物极小，突出环境
    ExtremeWide,
    /// 远景 —— 人物全身 + 环境
    Wide,
    /// 全景 —— 人物全身
    Full,
    /// 中全景 —— 膝上
    MediumFull,
    /// 中景 —— 腰上
    Medium,
    /// 中近景 —— 胸上
    MediumClose,
    /// 近景 —— 肩以上
    Close,
    /// 特写 —— 面部
    ExtremeClose,
    /// 大特写 —— 局部细节 (眼/手)
    Macro,
}

impl ShotSize {
    pub fn all() -> &'static [ShotSize] {
        &[
            Self::ExtremeWide,
            Self::Wide,
            Self::Full,
            Self::MediumFull,
            Self::Medium,
            Self::MediumClose,
            Self::Close,
            Self::ExtremeClose,
            Self::Macro,
        ]
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ExtremeWide => "极远景",
            Self::Wide => "远景",
            Self::Full => "全景",
            Self::MediumFull => "中全景",
            Self::Medium => "中景",
            Self::MediumClose => "中近景",
            Self::Close => "近景",
            Self::ExtremeClose => "特写",
            Self::Macro => "大特写",
        }
    }

    /// 英文 prompt 标签
    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::ExtremeWide => {
                "extreme wide shot, establishing shot, tiny figure in vast landscape"
            }
            Self::Wide => "wide shot, full body in environment",
            Self::Full => "full shot, entire figure from head to toe",
            Self::MediumFull => "medium full shot, from knees up",
            Self::Medium => "medium shot, from waist up",
            Self::MediumClose => "medium close-up, from chest up",
            Self::Close => "close-up shot, shoulders and head",
            Self::ExtremeClose => "extreme close-up, face filling frame",
            Self::Macro => "macro shot, extreme detail on eyes or hands",
        }
    }
}

// ============================================================================
// 机位角度 (Camera Angle)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraAngle {
    /// 鸟瞰 —— 正上方垂直俯拍
    BirdsEye,
    /// 高角度/俯拍
    High,
    /// 平视 —— 与人眼同高
    EyeLevel,
    /// 低角度/仰拍
    Low,
    /// 虫视 —— 地面向上
    WormsEye,
    /// 荷兰角 —— 倾斜画面
    Dutch,
    /// 过肩镜头
    OverShoulder,
    /// POV —— 主观视角
    Pov,
}

impl CameraAngle {
    pub fn all() -> &'static [CameraAngle] {
        &[
            Self::BirdsEye,
            Self::High,
            Self::EyeLevel,
            Self::Low,
            Self::WormsEye,
            Self::Dutch,
            Self::OverShoulder,
            Self::Pov,
        ]
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::BirdsEye => "鸟瞰",
            Self::High => "俯拍",
            Self::EyeLevel => "平视",
            Self::Low => "仰拍",
            Self::WormsEye => "虫视",
            Self::Dutch => "荷兰角",
            Self::OverShoulder => "过肩",
            Self::Pov => "主观视角",
        }
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::BirdsEye => "bird's eye view, directly overhead",
            Self::High => "high angle shot, looking down",
            Self::EyeLevel => "eye level shot, straight on",
            Self::Low => "low angle shot, looking up",
            Self::WormsEye => "worm's eye view, extreme low angle from ground",
            Self::Dutch => "dutch angle, tilted frame, disorienting",
            Self::OverShoulder => "over the shoulder shot",
            Self::Pov => "POV shot, first person perspective",
        }
    }
}

// ============================================================================
// 镜头运动 (Camera Movement)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMovement {
    /// 固定
    Static,
    /// 推 —— 向前移动
    PushIn,
    /// 拉 —— 向后移动
    PullOut,
    /// 摇 —— 水平旋转
    Pan,
    /// 俯仰 —— 垂直旋转
    Tilt,
    /// 跟 —— 跟随主体
    Tracking,
    /// 弧线 —— 绕主体旋转
    Arc,
    /// 升降
    Crane,
    /// 手持晃动
    Handheld,
    /// 急推/快速变焦
    Crash,
}

impl CameraMovement {
    pub fn all() -> &'static [CameraMovement] {
        &[
            Self::Static,
            Self::PushIn,
            Self::PullOut,
            Self::Pan,
            Self::Tilt,
            Self::Tracking,
            Self::Arc,
            Self::Crane,
            Self::Handheld,
            Self::Crash,
        ]
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Static => "固定",
            Self::PushIn => "推",
            Self::PullOut => "拉",
            Self::Pan => "摇",
            Self::Tilt => "俯仰",
            Self::Tracking => "跟",
            Self::Arc => "弧线",
            Self::Crane => "升降",
            Self::Handheld => "手持",
            Self::Crash => "急推",
        }
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::Static => "static camera, no movement",
            Self::PushIn => "push in, camera moves forward toward subject",
            Self::PullOut => "pull out, camera moves backward revealing more",
            Self::Pan => "pan, horizontal camera rotation",
            Self::Tilt => "tilt, vertical camera rotation",
            Self::Tracking => "tracking shot, camera follows subject movement",
            Self::Arc => "arc shot, camera orbits around subject",
            Self::Crane => "crane shot, camera rises or descends",
            Self::Handheld => "handheld camera, slight shake, documentary feel",
            Self::Crash => "crash zoom, rapid zoom in for dramatic effect",
        }
    }
}

// ============================================================================
// 焦距 (Focal Length)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocalLength {
    /// 超广角 < 24mm
    UltraWide,
    /// 广角 24-35mm
    Wide,
    /// 标准 35-70mm
    Standard,
    /// 中长焦 70-135mm
    MediumTele,
    /// 长焦 > 135mm
    Tele,
}

impl FocalLength {
    pub fn all() -> &'static [FocalLength] {
        &[
            Self::UltraWide,
            Self::Wide,
            Self::Standard,
            Self::MediumTele,
            Self::Tele,
        ]
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::UltraWide => "超广角",
            Self::Wide => "广角",
            Self::Standard => "标准",
            Self::MediumTele => "中长焦",
            Self::Tele => "长焦",
        }
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::UltraWide => {
                "ultra wide angle lens, exaggerated perspective, deep depth of field"
            }
            Self::Wide => "wide angle lens, broad view",
            Self::Standard => "standard focal length, natural perspective",
            Self::MediumTele => "medium telephoto, slight compression, shallow depth of field",
            Self::Tele => {
                "telephoto lens, compressed perspective, very shallow depth of field, bokeh"
            }
        }
    }
}

// ============================================================================
// 摄影风格预设
// ============================================================================

/// 摄影风格预设 —— 组合景别/角度/运动/焦距的常见电影风格。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CinematographyPreset {
    pub name: String,
    pub description: String,
    pub shot_size: Option<ShotSize>,
    pub camera_angle: Option<CameraAngle>,
    pub camera_movement: Option<CameraMovement>,
    pub focal_length: Option<FocalLength>,
    /// 完整 prompt 片段
    pub prompt_snippet: String,
}

/// 获取所有摄影风格预设。
pub fn cinematography_presets() -> Vec<CinematographyPreset> {
    vec![
        CinematographyPreset {
            name: "对话特写".into(),
            description: "人物对话时的脸部特写".into(),
            shot_size: Some(ShotSize::Close),
            camera_angle: Some(CameraAngle::EyeLevel),
            camera_movement: Some(CameraMovement::Static),
            focal_length: Some(FocalLength::MediumTele),
            prompt_snippet: "close-up portrait shot, eye level, static camera, medium telephoto lens, cinematic lighting, shallow depth of field with bokeh background".into(),
        },
        CinematographyPreset {
            name: "英雄登场".into(),
            description: "低角度仰拍，主体显得高大威猛".into(),
            shot_size: Some(ShotSize::Full),
            camera_angle: Some(CameraAngle::Low),
            camera_movement: Some(CameraMovement::PushIn),
            focal_length: Some(FocalLength::Wide),
            prompt_snippet: "hero shot, low angle looking up, push in movement, wide angle lens, dramatic backlighting, powerful presence, epic atmosphere".into(),
        },
        CinematographyPreset {
            name: "追逐跟拍".into(),
            description: "侧面跟拍快速移动中的角色".into(),
            shot_size: Some(ShotSize::Medium),
            camera_angle: Some(CameraAngle::EyeLevel),
            camera_movement: Some(CameraMovement::Tracking),
            focal_length: Some(FocalLength::Standard),
            prompt_snippet: "tracking shot, side view, following fast movement, motion blur on background, dynamic action, standard lens".into(),
        },
        CinematographyPreset {
            name: "鸟瞰全景".into(),
            description: "从高处俯瞰整个场景".into(),
            shot_size: Some(ShotSize::ExtremeWide),
            camera_angle: Some(CameraAngle::BirdsEye),
            camera_movement: Some(CameraMovement::Crane),
            focal_length: Some(FocalLength::Wide),
            prompt_snippet: "bird's eye view, extreme wide establishing shot, crane movement descending, wide angle lens, sweeping landscape below, grand scale".into(),
        },
        CinematographyPreset {
            name: "悬疑 POV".into(),
            description: "第一人称视角，略带晃动".into(),
            shot_size: Some(ShotSize::Medium),
            camera_angle: Some(CameraAngle::Pov),
            camera_movement: Some(CameraMovement::Handheld),
            focal_length: Some(FocalLength::Standard),
            prompt_snippet: "POV shot, first person perspective, slight handheld shake, natural movement, immersive view, standard lens".into(),
        },
        CinematographyPreset {
            name: "戏剧性揭示".into(),
            description: "缓慢弧线环绕揭示场景".into(),
            shot_size: Some(ShotSize::MediumFull),
            camera_angle: Some(CameraAngle::EyeLevel),
            camera_movement: Some(CameraMovement::Arc),
            focal_length: Some(FocalLength::Wide),
            prompt_snippet: "dramatic reveal, arc shot orbiting around subject, smooth camera movement, wide angle lens, cinematic lighting, tension building".into(),
        },
        CinematographyPreset {
            name: "紧张对峙".into(),
            description: "荷兰角 + 特写，不安氛围".into(),
            shot_size: Some(ShotSize::Close),
            camera_angle: Some(CameraAngle::Dutch),
            camera_movement: Some(CameraMovement::Static),
            focal_length: Some(FocalLength::Standard),
            prompt_snippet: "dutch angle, tilted frame, close-up shot, static camera, unsettling atmosphere, psychological tension, dramatic shadows".into(),
        },
        CinematographyPreset {
            name: "O.S. 对话".into(),
            description: "过肩镜头，对话场景".into(),
            shot_size: Some(ShotSize::MediumClose),
            camera_angle: Some(CameraAngle::OverShoulder),
            camera_movement: Some(CameraMovement::Static),
            focal_length: Some(FocalLength::MediumTele),
            prompt_snippet: "over the shoulder shot, medium close-up, static camera, medium telephoto, soft background, natural dialogue lighting, intimate conversation".into(),
        },
    ]
}

// ============================================================================
// 从 Shot 数据组装 Prompt
// ============================================================================

/// 将摄影参数编译为一段 prompt 文本。
pub fn compile_cinematography_prompt(
    shot_size: Option<ShotSize>,
    camera_angle: Option<CameraAngle>,
    camera_movement: Option<CameraMovement>,
    focal_length: Option<FocalLength>,
) -> String {
    let mut parts: Vec<&str> = Vec::new();

    if let Some(ss) = shot_size {
        parts.push(ss.prompt_label());
    }
    if let Some(ca) = camera_angle {
        parts.push(ca.prompt_label());
    }
    if let Some(cm) = camera_movement {
        parts.push(cm.prompt_label());
    }
    if let Some(fl) = focal_length {
        parts.push(fl.prompt_label());
    }

    if parts.is_empty() {
        "cinematic shot".into()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_shot_sizes_have_labels() {
        for s in ShotSize::all() {
            assert!(!s.display_name().is_empty());
            assert!(!s.prompt_label().is_empty());
        }
    }

    #[test]
    fn all_angles_have_labels() {
        for a in CameraAngle::all() {
            assert!(!a.display_name().is_empty());
            assert!(!a.prompt_label().is_empty());
        }
    }

    #[test]
    fn all_movements_have_labels() {
        for m in CameraMovement::all() {
            assert!(!m.display_name().is_empty());
            assert!(!m.prompt_label().is_empty());
        }
    }

    #[test]
    fn presets_count() {
        assert_eq!(cinematography_presets().len(), 8);
    }

    #[test]
    fn compile_prompt_combines_all_params() {
        let prompt = compile_cinematography_prompt(
            Some(ShotSize::Close),
            Some(CameraAngle::Low),
            Some(CameraMovement::PushIn),
            Some(FocalLength::Wide),
        );
        assert!(prompt.contains("close-up"));
        assert!(prompt.contains("low angle"));
        assert!(prompt.contains("push in"));
        assert!(prompt.contains("wide angle"));
    }
}
