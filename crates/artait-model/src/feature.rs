//! 功能开关与预设。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureId {
    UiConcept,
    Scene,
    Character,
    Effect,
    ActionSequence,
    AssetBrowser,
    AnimationScene,
    AnimationCharacter,
    CharacterTurnaround,
    AnimationScript,
    Storyboard,
    CharacterLibrary,
    SceneLibrary,
    Video,
    Project,
}

impl FeatureId {
    pub const ALL: &'static [FeatureId] = &[
        FeatureId::UiConcept,
        FeatureId::Scene,
        FeatureId::Character,
        FeatureId::Effect,
        FeatureId::ActionSequence,
        FeatureId::Video,
        FeatureId::AssetBrowser,
        FeatureId::AnimationScene,
        FeatureId::AnimationCharacter,
        FeatureId::CharacterTurnaround,
        FeatureId::AnimationScript,
        FeatureId::Storyboard,
        FeatureId::CharacterLibrary,
        FeatureId::SceneLibrary,
        FeatureId::Project,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            FeatureId::UiConcept => "UI 概念",
            FeatureId::Scene => "创建场景",
            FeatureId::Character => "创建角色",
            FeatureId::Effect => "特效",
            FeatureId::ActionSequence => "动作序列",
            FeatureId::Video => "视频",
            FeatureId::AssetBrowser => "图库",
            FeatureId::AnimationScene => "动画场景",
            FeatureId::AnimationCharacter => "动画角色",
            FeatureId::CharacterTurnaround => "角色三视图",
            FeatureId::AnimationScript => "剧本",
            FeatureId::Storyboard => "分镜板",
            FeatureId::CharacterLibrary => "角色库",
            FeatureId::SceneLibrary => "场景库",
            FeatureId::Project => "项目",
        }
    }

    /// FeatureId 对应的页面路由标识符。
    pub fn route_id(self) -> &'static str {
        match self {
            FeatureId::UiConcept => "ui_concept",
            FeatureId::Scene => "scene",
            FeatureId::Character => "character",
            FeatureId::Effect => "effect",
            FeatureId::ActionSequence => "action_sequence",
            FeatureId::Video => "video",
            FeatureId::AssetBrowser => "asset_browser",
            FeatureId::AnimationScene => "animation_scene",
            FeatureId::AnimationCharacter => "animation_character",
            FeatureId::CharacterTurnaround => "character_turnaround",
            FeatureId::AnimationScript => "animation_script",
            FeatureId::Storyboard => "storyboard",
            FeatureId::CharacterLibrary => "character_library",
            FeatureId::SceneLibrary => "scene_library",
            FeatureId::Project => "project",
        }
    }

    /// 所属工作台模式。
    pub fn workspace_mode(self) -> &'static str {
        match self {
            FeatureId::UiConcept
            | FeatureId::Scene
            | FeatureId::Character
            | FeatureId::Effect
            | FeatureId::ActionSequence
            | FeatureId::Video
            | FeatureId::AnimationScene
            | FeatureId::AnimationCharacter
            | FeatureId::CharacterTurnaround => "art",
            FeatureId::AnimationScript
            | FeatureId::Storyboard
            | FeatureId::CharacterLibrary
            | FeatureId::SceneLibrary => "film",
            FeatureId::AssetBrowser | FeatureId::Project => "both",
        }
    }
}

/// 从页面路由反向查找 FeatureId。
pub fn feature_id_from_route(route: &str) -> Option<FeatureId> {
    Some(match route {
        "scene" => FeatureId::Scene,
        "character" => FeatureId::Character,
        "ui_concept" => FeatureId::UiConcept,
        "effect" => FeatureId::Effect,
        "action_sequence" => FeatureId::ActionSequence,
        "video" => FeatureId::Video,
        "animation_scene" => FeatureId::AnimationScene,
        "animation_character" => FeatureId::AnimationCharacter,
        "character_turnaround" => FeatureId::CharacterTurnaround,
        "animation_script" => FeatureId::AnimationScript,
        "storyboard" => FeatureId::Storyboard,
        "asset_browser" => FeatureId::AssetBrowser,
        "character_library" => FeatureId::CharacterLibrary,
        "scene_library" => FeatureId::SceneLibrary,
        "project" => FeatureId::Project,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeaturePreset {
    General,
    Animation,
    Full,
    Custom,
}

impl FeaturePreset {
    pub fn enabled_features(self) -> Vec<FeatureId> {
        match self {
            FeaturePreset::General => vec![
                FeatureId::UiConcept,
                FeatureId::Scene,
                FeatureId::Character,
                FeatureId::Effect,
                FeatureId::ActionSequence,
                FeatureId::Video,
                FeatureId::AssetBrowser,
                FeatureId::CharacterLibrary,
                FeatureId::SceneLibrary,
                FeatureId::Project,
            ],
            FeaturePreset::Animation => vec![
                FeatureId::AnimationScene,
                FeatureId::AnimationCharacter,
                FeatureId::CharacterTurnaround,
                FeatureId::AnimationScript,
                FeatureId::Storyboard,
                FeatureId::Video,
                FeatureId::AssetBrowser,
                FeatureId::CharacterLibrary,
                FeatureId::SceneLibrary,
                FeatureId::Project,
            ],
            FeaturePreset::Full => FeatureId::ALL.to_vec(),
            FeaturePreset::Custom => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureConfig {
    pub preset: FeaturePreset,
    pub enabled: Vec<FeatureId>,
    #[serde(default)]
    pub sidebar_hidden: Vec<FeatureId>,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            preset: FeaturePreset::General,
            enabled: FeaturePreset::General.enabled_features(),
            sidebar_hidden: Vec::new(),
        }
    }
}

impl FeatureConfig {
    pub fn is_enabled(&self, id: FeatureId) -> bool {
        self.enabled.contains(&id)
    }

    pub fn is_sidebar_visible(&self, id: FeatureId) -> bool {
        !self.sidebar_hidden.contains(&id)
    }

    /// 迁移：确保当前预设的所有功能都在启用列表中。
    /// 用于新版本增加了 FeatureId 后，旧配置文件自动补全。
    pub fn migrate(&mut self) -> bool {
        let preset_features = self.preset.enabled_features();
        let mut changed = false;
        for f in &preset_features {
            if !self.enabled.contains(f) {
                self.enabled.push(*f);
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_preset_has_ten_features() {
        assert_eq!(FeaturePreset::General.enabled_features().len(), 10);
    }

    #[test]
    fn full_preset_has_all() {
        assert_eq!(
            FeaturePreset::Full.enabled_features().len(),
            FeatureId::ALL.len()
        );
    }
}
