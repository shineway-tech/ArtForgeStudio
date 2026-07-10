//! 导演/分镜存储服务。
//!
//! 管理 Shot 和分镜包 (StoryboardPackage) 的运行时状态。
//! 支持批量设置摄影参数、生成状态跟踪。

use artait_model::cinematography::{
    cinematography_presets, compile_cinematography_prompt, CameraAngle, CameraMovement,
    CinematographyPreset, FocalLength, ShotSize,
};
use artait_model::script::{Shot, ShotPackage};
use std::collections::HashMap;

/// 分镜会话 —— 一次分镜编辑的完整状态。
#[derive(Debug, Clone, Default)]
pub struct DirectorSession {
    /// 所有分镜包
    pub packages: Vec<ShotPackage>,
    /// 当前选中的包索引
    pub selected_package: usize,
    /// 当前选中的镜头索引（包内）
    pub selected_shot: usize,
    /// 镜头 → 参数字典（id → 参数）
    pub shot_params: HashMap<String, ShotParams>,
    /// 生成状态
    pub generation_status: GenerationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GenerationStatus {
    #[default]
    Idle,
    Generating,
    Completed,
    Error(String),
}

/// 单个镜头的摄影参数。
#[derive(Debug, Clone)]
pub struct ShotParams {
    pub shot_size: Option<ShotSize>,
    pub camera_angle: Option<CameraAngle>,
    pub camera_movement: Option<CameraMovement>,
    pub focal_length: Option<FocalLength>,
    /// 自定义镜头说明（中文）
    pub custom_note: Option<String>,
}

impl Default for ShotParams {
    fn default() -> Self {
        Self {
            shot_size: Some(ShotSize::Medium),
            camera_angle: Some(CameraAngle::EyeLevel),
            camera_movement: None,
            focal_length: None,
            custom_note: None,
        }
    }
}

impl DirectorSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从分镜包列表创建会话。
    pub fn from_packages(packages: Vec<ShotPackage>) -> Self {
        let mut params = HashMap::new();
        for pkg in &packages {
            for shot in &pkg.shots {
                params.insert(shot.id.clone(), ShotParams::default());
            }
        }
        Self {
            packages,
            shot_params: params,
            ..Default::default()
        }
    }

    /// 获取或创建镜头的摄影参数。
    pub fn get_or_create_params(&mut self, shot_id: &str) -> &mut ShotParams {
        self.shot_params.entry(shot_id.to_string()).or_default()
    }

    /// 为镜头设置摄影参数。
    pub fn set_shot_params(&mut self, shot_id: &str, params: ShotParams) {
        self.shot_params.insert(shot_id.to_string(), params);
    }

    /// 为当前选中镜头应用摄影风格预设。
    pub fn apply_preset(&mut self, shot_id: &str, preset: &CinematographyPreset) {
        let p = self.get_or_create_params(shot_id);
        p.shot_size = preset.shot_size;
        p.camera_angle = preset.camera_angle;
        p.camera_movement = preset.camera_movement;
        p.focal_length = preset.focal_length;
    }

    /// 为镜头构建完整的视觉 prompt（动作 + 摄影参数）。
    pub fn build_shot_prompt(&self, shot: &Shot) -> String {
        let params = self.shot_params.get(&shot.id);

        let mut parts: Vec<String> = Vec::new();

        // 动作描述
        parts.push(if let Some(ref en) = shot.visual_prompt_en {
            en.clone()
        } else if !shot.action.is_empty() {
            shot.action.clone()
        } else {
            format!("shot {}", shot.number)
        });

        // 角色
        if !shot.characters.is_empty() {
            parts.push(format!("characters: {}", shot.characters.join(", ")));
        }

        // 摄影参数
        if let Some(p) = params {
            let cine = compile_cinematography_prompt(
                p.shot_size,
                p.camera_angle,
                p.camera_movement,
                p.focal_length,
            );
            parts.push(cine);
        }

        // 自定义备注
        if let Some(p) = params {
            if let Some(ref note) = p.custom_note {
                parts.push(note.clone());
            }
        }

        parts.join(", ")
    }

    /// 获取所有摄影风格预设。
    pub fn presets() -> &'static [CinematographyPreset] {
        // 返回静态引用 — 通过 lazy_static 或 once_cell
        // 当前使用函数调用方式
        &[]
    }

    /// 获取全部预设列表 (owned)。
    pub fn all_presets() -> Vec<CinematographyPreset> {
        cinematography_presets()
    }

    /// 批量设置多个镜头的同一参数。
    pub fn batch_set_shot_size(&mut self, shot_ids: &[String], size: ShotSize) {
        for id in shot_ids {
            self.get_or_create_params(id).shot_size = Some(size);
        }
    }

    pub fn batch_set_camera_angle(&mut self, shot_ids: &[String], angle: CameraAngle) {
        for id in shot_ids {
            self.get_or_create_params(id).camera_angle = Some(angle);
        }
    }

    /// 获取镜头的摄影参数摘要（中文，用于 UI 展示）。
    pub fn shot_params_summary(&self, shot_id: &str) -> String {
        let params = match self.shot_params.get(shot_id) {
            Some(p) => p,
            None => return "中景 · 平视".into(),
        };

        let mut parts: Vec<&str> = Vec::new();
        if let Some(ss) = params.shot_size {
            parts.push(ss.display_name());
        }
        if let Some(ca) = params.camera_angle {
            parts.push(ca.display_name());
        }
        if let Some(cm) = params.camera_movement {
            parts.push(cm.display_name());
        }
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shot(id: &str, num: u32) -> Shot {
        Shot {
            id: id.into(),
            number: num,
            episode_index: 0,
            scene_header: String::new(),
            action: "角色走进房间".into(),
            dialogues: vec![],
            characters: vec!["张三".into()],
            shot_size: None,
            camera_angle: None,
            camera_movement: None,
            focal_length: None,
            visual_prompt_en: None,
            visual_prompt_zh: None,
            generated_image_path: None,
            generation_status: Default::default(),
        }
    }

    fn make_package(idx: usize, shots: Vec<Shot>) -> ShotPackage {
        ShotPackage {
            index: idx,
            label: format!("包{idx}"),
            shot_count: shots.len(),
            shots,
            markdown: String::new(),
        }
    }

    #[test]
    fn session_from_packages() {
        let s = make_shot("s1", 1);
        let pkg = make_package(0, vec![s]);
        let session = DirectorSession::from_packages(vec![pkg]);
        assert_eq!(session.shot_params.len(), 1);
    }

    #[test]
    fn apply_preset_changes_params() {
        let mut session = DirectorSession::new();
        session.get_or_create_params("s1");
        let preset = cinematography_presets().into_iter().next().unwrap();
        session.apply_preset("s1", &preset);
        let p = session.shot_params.get("s1").unwrap();
        assert_eq!(p.shot_size, preset.shot_size);
    }

    #[test]
    fn build_prompt_includes_action_and_cinematography() {
        let mut session = DirectorSession::new();
        let shot = make_shot("s1", 1);
        session.set_shot_params(
            "s1",
            ShotParams {
                shot_size: Some(ShotSize::Close),
                camera_angle: Some(CameraAngle::Low),
                camera_movement: None,
                focal_length: None,
                custom_note: Some("dramatic lighting".into()),
            },
        );
        let prompt = session.build_shot_prompt(&shot);
        assert!(prompt.contains("走进房间"));
        assert!(prompt.contains("close-up"));
        assert!(prompt.contains("low angle"));
        assert!(prompt.contains("dramatic lighting"));
    }

    #[test]
    fn params_summary_chinese() {
        let mut session = DirectorSession::new();
        session.set_shot_params(
            "s1",
            ShotParams {
                shot_size: Some(ShotSize::Wide),
                camera_angle: Some(CameraAngle::BirdsEye),
                camera_movement: Some(CameraMovement::Crane),
                focal_length: None,
                custom_note: None,
            },
        );
        let summary = session.shot_params_summary("s1");
        assert!(summary.contains("远景"));
        assert!(summary.contains("鸟瞰"));
        assert!(summary.contains("升降"));
    }
}
