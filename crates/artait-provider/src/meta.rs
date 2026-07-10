//! ProviderMeta：协议族静态描述。

use artait_model::{ProviderCapabilities, ProviderFamily};

#[derive(Debug, Clone)]
pub struct ProviderMeta {
    pub id: &'static str,
    pub display_name: &'static str,
    pub family: ProviderFamily,
    pub capabilities: ProviderCapabilities,
    pub default_generation_models: &'static [&'static str],
    pub default_analysis_models: &'static [&'static str],
    pub default_video_models: &'static [&'static str],
    pub config_schema: &'static str,
    pub is_legacy: bool,
}
