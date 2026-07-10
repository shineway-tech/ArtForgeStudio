//! Provider 实例与元信息数据结构。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderScope {
    Generation,
    Analysis,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFamily {
    OpenAiCompatible,
    GeminiCompatible,
    WavespeedCompatible,
    VolcengineSeedance,
    DeepSeek,
    Ikuncode,
    Rembg,
    PhotoRoom,
    Mock,
    Custom,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub generate: bool,
    pub generate_character: bool,
    pub generate_video: bool,
    pub analyze: bool,
    pub test_connection: bool,
    pub quota: bool,
    pub upload_binary: bool,
    pub poll_task: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    #[serde(default)]
    pub generation_model: Option<String>,
    #[serde(default)]
    pub generation_model_options: Vec<String>,
    #[serde(default)]
    pub analysis_model: Option<String>,
    #[serde(default)]
    pub analysis_model_options: Vec<String>,
    #[serde(default)]
    pub video_model: Option<String>,
    #[serde(default)]
    pub video_model_options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInstance {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub family: ProviderFamily,
    pub scopes: Vec<ProviderScope>,
    #[serde(default = "default_true")]
    pub show_in_main_ui: bool,
    #[serde(default)]
    pub models: ProviderModelConfig,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default = "empty_extra", skip_serializing_if = "is_empty_extra")]
    pub extra: serde_json::Value,
}

fn default_true() -> bool {
    true
}

fn empty_extra() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

fn is_empty_extra(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderDefaults {
    #[serde(default)]
    pub generation: Option<String>,
    #[serde(default)]
    pub analysis: Option<String>,
    #[serde(default)]
    pub video: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub ok: bool,
    pub message: String,
}

#[derive(thiserror::Error, Debug)]
pub enum ProviderError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("missing secret: {0}")]
    MissingSecret(String),
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("rate limited")]
    RateLimited,
    #[error("provider rejected: {0}")]
    ProviderRejected(String),
    #[error("provider timeout")]
    ProviderTimeout,
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("task cancelled")]
    TaskCancelled,
    #[error("save failed: {0}")]
    SaveFailed(String),
    #[error("unsupported capability")]
    UnsupportedCapability,
    #[error("io error: {0}")]
    Io(String),
}
