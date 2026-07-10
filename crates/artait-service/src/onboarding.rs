//! 首启引导 4 步流程的 Rust 端数据类型与逻辑。

use std::path::{Path, PathBuf};

use anyhow::Result;
use artait_model::{AppConfig, FeatureConfig, FeatureId, FeaturePreset, ThemeId};

/// 用户首启引导内部状态。`AppState.onboarding` 是这份的 Slint 视图。
#[derive(Clone)]
pub struct OnboardingDraft {
    pub step: i32,
    pub preset: FeaturePreset,
    pub features: Vec<bool>,
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub prompt_dir: PathBuf,
    pub theme_id: ThemeId,
    pub legacy_path: Option<PathBuf>,
    pub legacy_hint: String,
    pub last_error: String,
}

/// 顺序与 Slint feature-flags 数组一一对应。
pub const FEATURE_ORDER: &[FeatureId] = &[
    FeatureId::Scene,
    FeatureId::Character,
    FeatureId::UiConcept,
    FeatureId::Effect,
    FeatureId::ActionSequence,
    FeatureId::AssetBrowser,
    FeatureId::AnimationScene,
    FeatureId::AnimationCharacter,
    FeatureId::CharacterTurnaround,
    FeatureId::AnimationScript,
    FeatureId::Storyboard,
];

impl OnboardingDraft {
    pub fn from_default() -> Self {
        let cfg = AppConfig::default();
        let legacy = detect_legacy_dir();
        let legacy_hint = legacy
            .as_ref()
            .map(|p| format!("发现旧版工作区：{}", p.display()))
            .unwrap_or_default();

        Self {
            step: 1,
            preset: FeaturePreset::General,
            features: features_from_preset(FeaturePreset::General),
            input_dir: cfg.paths.input_dir,
            output_dir: cfg.paths.output_dir,
            prompt_dir: cfg.paths.prompt_dir,
            theme_id: ThemeId::Dark,
            legacy_path: legacy,
            legacy_hint,
            last_error: String::new(),
        }
    }

    pub fn pick_preset(&mut self, id: &str) {
        self.preset = match id {
            "general" => FeaturePreset::General,
            "animation" => FeaturePreset::Animation,
            "full" => FeaturePreset::Full,
            "custom" => FeaturePreset::Custom,
            _ => return,
        };
        if self.preset != FeaturePreset::Custom {
            self.features = features_from_preset(self.preset);
        }
    }

    pub fn toggle_feature(&mut self, idx: usize) {
        if let Some(v) = self.features.get_mut(idx) {
            *v = !*v;
            self.preset = FeaturePreset::Custom;
        }
    }

    pub fn use_legacy_paths(&mut self) {
        let Some(base) = self.legacy_path.clone() else {
            return;
        };
        let probe = |sub: &str| {
            let p = base.join(sub);
            if p.exists() {
                p
            } else {
                base.join(sub)
            }
        };
        self.input_dir = probe("input");
        self.output_dir = probe("out");
        self.prompt_dir = probe("prompt");
    }

    pub fn set_dir(&mut self, kind: &str, path: PathBuf) {
        match kind {
            "input" => self.input_dir = path,
            "output" => self.output_dir = path,
            "prompt" => self.prompt_dir = path,
            _ => {}
        }
        self.last_error.clear();
    }

    pub fn current_dir(&self, kind: &str) -> &PathBuf {
        match kind {
            "input" => &self.input_dir,
            "output" => &self.output_dir,
            _ => &self.prompt_dir,
        }
    }

    pub fn pick_theme(&mut self, id: &str) {
        self.theme_id = match id {
            "dark" => ThemeId::Dark,
            "light" => ThemeId::Light,
            "ocean" => ThemeId::Ocean,
            "warm" => ThemeId::Warm,
            "forest" => ThemeId::Forest,
            "rose" => ThemeId::Rose,
            "cyber" => ThemeId::Cyber,
            "oled" => ThemeId::Oled,
            "cream" => ThemeId::Cream,
            "system" => ThemeId::System,
            "user" => ThemeId::User,
            _ => return,
        };
    }

    pub fn validate_step(&mut self) -> bool {
        self.last_error.clear();
        match self.step {
            1 => {
                if !self.features.iter().any(|v| *v) {
                    self.last_error = "至少启用一项功能".into();
                    return false;
                }
                true
            }
            2 => {
                for (label, p) in [
                    ("输入目录", &self.input_dir),
                    ("输出目录", &self.output_dir),
                    ("提示词目录", &self.prompt_dir),
                ] {
                    if let Err(e) = std::fs::create_dir_all(p) {
                        self.last_error = format!("{label}创建失败：{e}");
                        return false;
                    }
                }
                true
            }
            _ => true,
        }
    }

    pub fn next(&mut self) {
        if !self.validate_step() {
            return;
        }
        if self.step < 3 {
            self.step += 1;
        }
    }

    pub fn prev(&mut self) {
        if self.step > 1 {
            self.step -= 1;
            self.last_error.clear();
        }
    }

    /// 写最终 AppConfig。
    pub fn into_config(self) -> AppConfig {
        let enabled: Vec<FeatureId> = FEATURE_ORDER
            .iter()
            .zip(self.features.iter())
            .filter_map(|(id, on)| if *on { Some(*id) } else { None })
            .collect();

        let mut cfg = AppConfig::default();
        cfg.paths.input_dir = self.input_dir;
        cfg.paths.output_dir = self.output_dir;
        cfg.paths.prompt_dir = self.prompt_dir;
        cfg.ui.theme = self.theme_id;
        cfg.features = FeatureConfig {
            preset: self.preset,
            enabled,
            sidebar_hidden: Vec::new(),
        };
        cfg.migrated_from = self.legacy_path;
        cfg
    }
}

fn features_from_preset(preset: FeaturePreset) -> Vec<bool> {
    let enabled = preset.enabled_features();
    FEATURE_ORDER
        .iter()
        .map(|id| enabled.contains(id))
        .collect()
}

/// 启发式探测：当前工作目录或常见旧目录是否存在 Python 版的 config.json / out / prompt。
fn detect_legacy_dir() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = std::env::current_dir()
        .ok()
        .into_iter()
        .chain([PathBuf::from("D:\\BoBO\\ArtAIT")])
        .collect();
    for c in candidates {
        if looks_like_legacy(&c) {
            return Some(c);
        }
    }
    None
}

fn looks_like_legacy(p: &Path) -> bool {
    let key_files = ["config.json", "out", "prompt", "apply_prompt"];
    key_files.iter().filter(|f| p.join(f).exists()).count() >= 2
}

pub fn preset_id(p: FeaturePreset) -> &'static str {
    match p {
        FeaturePreset::General => "general",
        FeaturePreset::Animation => "animation",
        FeaturePreset::Full => "full",
        FeaturePreset::Custom => "custom",
    }
}

pub fn theme_id_str(id: ThemeId) -> &'static str {
    match id {
        ThemeId::Dark => "dark",
        ThemeId::Light => "light",
        ThemeId::Ocean => "ocean",
        ThemeId::Warm => "warm",
        ThemeId::Forest => "forest",
        ThemeId::Rose => "rose",
        ThemeId::Cyber => "cyber",
        ThemeId::Oled => "oled",
        ThemeId::Cream => "cream",
        ThemeId::System => "system",
        ThemeId::User => "user",
    }
}

/// 草稿存盘路径。
#[allow(dead_code)]
pub fn draft_path() -> Result<PathBuf> {
    Ok(artait_config::config_dir()?.join(".onboarding-draft.toml"))
}
