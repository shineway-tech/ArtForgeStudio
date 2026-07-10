//! 应用配置根类型。

use serde::{Deserialize, Serialize};

use crate::{
    feature::FeatureConfig,
    paths::PathConfig,
    project::ProjectEntry,
    provider::{ProviderDefaults, ProviderInstance},
    theme::UiConfig,
};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RemoveBackgroundConfig {
    #[serde(default)]
    pub rembg_endpoint: Option<String>,
    #[serde(default)]
    pub photoroom_secret_ref: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_true")]
    pub log_enabled: bool,
    #[serde(default)]
    pub debug_log_enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageUploadConfig {
    /// 图床上传地址（为空则使用 ImgBB）
    #[serde(default)]
    pub api_url: Option<String>,
    /// 图床上传 API Key
    #[serde(default)]
    pub api_key: Option<String>,
    /// 超过此大小（MB）的参考图自动上传，<=0 禁用，默认 10
    #[serde(default)]
    pub size_threshold_mb: u64,
}

impl Default for ImageUploadConfig {
    fn default() -> Self {
        Self {
            api_url: None,
            api_key: None,
            size_threshold_mb: 10,
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            log_enabled: true,
            debug_log_enabled: false,
        }
    }
}

/// Sidecar 进程配置（通用接口，可扩展支持多种 sidecar）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarConfig {
    /// Prompt Optimizer sidecar 可执行文件路径。
    /// 为空时使用默认路径（与主程序同目录的 `prompt-optimizer-server.exe`）。
    #[serde(default)]
    pub prompt_optimizer_path: Option<String>,

    /// Sidecar HTTP 端口（0 = 自动分配可用端口）。
    #[serde(default)]
    pub prompt_optimizer_port: u16,

    /// 空闲超时（秒）：无任务后保持运行的最大时间，超时自动关闭。
    /// 0 = 永不自动关闭。
    #[serde(default = "SidecarConfig::default_idle_timeout_secs")]
    pub prompt_optimizer_idle_timeout_secs: u64,
}

impl SidecarConfig {
    const fn default_idle_timeout_secs() -> u64 {
        300 // 5 分钟
    }
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self {
            prompt_optimizer_path: None,
            prompt_optimizer_port: 0,
            prompt_optimizer_idle_timeout_secs: Self::default_idle_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastWorkspaceState {
    pub page: String,
    pub prompt: String,
    #[serde(default)]
    pub negative: String,
    pub aspect: String,
    pub quality: String,
    pub count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,

    #[serde(default)]
    pub paths: PathConfig,

    #[serde(default)]
    pub ui: UiConfig,

    #[serde(default)]
    pub features: FeatureConfig,

    #[serde(default)]
    pub providers: Vec<ProviderInstance>,

    #[serde(default)]
    pub provider_defaults: ProviderDefaults,

    #[serde(default)]
    pub remove_background: RemoveBackgroundConfig,

    #[serde(default)]
    pub image_upload: ImageUploadConfig,

    #[serde(default)]
    pub runtime: RuntimeConfig,

    #[serde(default)]
    pub last_main_tab: Option<String>,

    #[serde(default)]
    pub migrated_from: Option<std::path::PathBuf>,

    #[serde(default)]
    pub last_workspace: Option<LastWorkspaceState>,

    #[serde(default)]
    pub prompt_history: Vec<String>,

    #[serde(default)]
    pub sidecar: SidecarConfig,

    /// 项目列表
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,

    /// 当前活动的项目 ID（None = 通配模式）
    #[serde(default)]
    pub last_project: Option<String>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            paths: PathConfig::default(),
            ui: UiConfig::default(),
            features: FeatureConfig::default(),
            providers: Vec::new(),
            provider_defaults: ProviderDefaults::default(),
            remove_background: RemoveBackgroundConfig::default(),
            image_upload: ImageUploadConfig::default(),
            runtime: RuntimeConfig::default(),
            last_main_tab: None,
            migrated_from: None,
            last_workspace: None,
            prompt_history: Vec::new(),
            sidecar: SidecarConfig::default(),
            projects: Vec::new(),
            last_project: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FeatureId, FeaturePreset, ProviderFamily, ProviderInstance, ProviderScope, ThemeId,
    };

    #[test]
    fn default_round_trips() {
        let cfg = AppConfig::default();
        let s = toml::to_string(&cfg).expect("serialize");
        let back: AppConfig = toml::from_str(&s).expect("deserialize");
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_eq!(back.features.preset, FeaturePreset::General);
        assert!(back.features.is_enabled(FeatureId::Scene));
        assert!(back.runtime.log_enabled);
        assert!(!back.runtime.debug_log_enabled);
    }

    #[test]
    fn missing_fields_get_defaults() {
        let s = "schema_version = 1\n";
        let cfg: AppConfig = toml::from_str(s).expect("deserialize");
        assert!(cfg.providers.is_empty());
        assert_eq!(cfg.schema_version, 1);
        assert_eq!(cfg.ui.theme, ThemeId::Dark);
    }

    #[test]
    fn empty_string_loads_defaults() {
        let cfg: AppConfig = toml::from_str("").expect("deserialize empty");
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn provider_instance_round_trips() {
        let mut cfg = AppConfig::default();
        cfg.providers.push(ProviderInstance {
            id: "test".into(),
            name: "Test".into(),
            provider_id: "openai-compatible".into(),
            family: ProviderFamily::OpenAiCompatible,
            scopes: vec![ProviderScope::Generation, ProviderScope::Analysis],
            show_in_main_ui: true,
            models: Default::default(),
            endpoint: Some("https://example.com/v1".into()),
            secret_ref: Some("test/api_key".into()),
            api_key: Some("sk-test".into()),
            extra: serde_json::Value::Object(serde_json::Map::new()),
        });

        let s = toml::to_string(&cfg).expect("serialize");
        assert!(s.contains("sk-test"));
        let back: AppConfig = toml::from_str(&s).expect("deserialize");
        assert_eq!(back.providers.len(), 1);
        assert_eq!(back.providers[0].id, "test");
        assert_eq!(back.providers[0].scopes.len(), 2);
        assert_eq!(back.providers[0].api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let s = r#"
            schema_version = 1
            unknown_top = "value"

            [unknown_section]
            x = 1
        "#;
        let cfg: AppConfig = toml::from_str(s).expect("deserialize");
        assert_eq!(cfg.schema_version, 1);
    }

    #[test]
    fn existing_ui_config_defaults_sidebar_collapsed() {
        let s = r#"
            schema_version = 1

            [ui]
            theme = "dark"
            font_family = "Sarasa UI SC"
            font_size = 14
            locale = "zh-CN"
        "#;
        let cfg: AppConfig = toml::from_str(s).expect("deserialize");
        assert!(!cfg.ui.sidebar_collapsed);
    }
}
