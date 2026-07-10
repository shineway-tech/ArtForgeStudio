//! Provider 请求 / 响应模型。

use std::path::PathBuf;

use artait_model::{CreationMode, ReferenceImage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub reference_images: Vec<ReferenceImage>,
    pub aspect_ratio: Option<String>,
    pub resolution: Option<(u32, u32)>,
    pub size: Option<String>,
    pub quality: Option<String>,
    pub count: u32,
    pub mode: CreationMode,
    pub action_name: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct CharacterGenerationRequest {
    pub prompt: String,
    pub reference_images: Vec<ReferenceImage>,
    pub aspect_ratio: Option<String>,
    pub resolution: Option<(u32, u32)>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AnalysisRequest {
    pub system_prompt: Option<String>,
    pub user_prompt: String,
    pub images: Vec<ReferenceImage>,
    pub model: Option<String>,
    pub response_format: AnalysisResponseFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisResponseFormat {
    Plain,
    Json,
}

#[derive(Debug, Clone)]
pub struct VideoGenerationRequest {
    pub prompt: String,
    pub image: Option<ReferenceImage>,
    pub duration: Option<f32>,
    pub aspect_ratio: Option<String>,
    pub resolution: Option<(u32, u32)>,
    pub generate_audio: bool,
    pub metadata: serde_json::Value,
    /// Seedance 视频生成参数（可选，用于 Memefast/Volcengine 视频通道）
    pub seedance_params: Option<artait_model::seedance::SeedanceVideoParams>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GenerationOutput {
    File {
        path: PathBuf,
        metadata: serde_json::Value,
    },
    Url {
        url: String,
        metadata: serde_json::Value,
    },
    Base64 {
        data: String,
        mime: String,
        metadata: serde_json::Value,
    },
    AsyncTask {
        provider_task_id: String,
        metadata: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisOutput {
    pub text: String,
    pub structured: Option<serde_json::Value>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt: u32,
    pub completion: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoOutput {
    pub kind: GenerationOutput,
    pub duration_seconds: Option<f32>,
    pub has_audio: bool,
}
