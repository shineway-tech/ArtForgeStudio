//! Memefast Seedance 视频生成协议族。
//!
//! 通过 MemeFast 代理 API 调用 Seedance 视频生成。
//! 鉴权：Bearer Token。支持 T2V、I2V、多模态引用。

use std::time::Duration;

use artait_model::{ConnectionStatus, ProviderCapabilities, ProviderError, ProviderFamily};
use artait_provider::{
    http::{HttpMethod, HttpRequest},
    meta::ProviderMeta,
    request::{GenerationOutput, VideoGenerationRequest, VideoOutput},
    Pollable, PollingStrategy, Provider, ProviderContext, ProviderModelList, ProviderResult,
    VideoGenerator,
};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tracing::info;

const META: ProviderMeta = ProviderMeta {
    id: "memefast-seedance",
    display_name: "MemeFast Seedance 视频",
    family: ProviderFamily::VolcengineSeedance,
    capabilities: ProviderCapabilities {
        generate: false,
        generate_character: false,
        generate_video: true,
        analyze: false,
        test_connection: true,
        quota: false,
        upload_binary: false,
        poll_task: true,
    },
    default_generation_models: &[],
    default_analysis_models: &[],
    default_video_models: &[
        "doubao-seedance-1-5-pro-251215",
        "doubao-seedance-pro-t2v",
        "doubao-seedance-lite-t2v",
    ],
    config_schema: r#"{"type":"object","properties":{"api_key":{"type":"string","title":"API Key","format":"password"}}}"#,
    is_legacy: false,
};

fn video_polling() -> PollingStrategy {
    PollingStrategy {
        interval: Duration::from_secs(5),
        max_polls: 180,
        ..Default::default()
    }
}

#[derive(Default)]
pub struct MemefastSeedanceProvider;

impl MemefastSeedanceProvider {
    pub fn new() -> Self {
        Self
    }

    fn base_url(ctx: &ProviderContext) -> String {
        ctx.endpoint
            .clone()
            .unwrap_or_else(|| "https://memefast.top".into())
            .trim_end_matches('/')
            .to_string()
    }

    fn api_key(ctx: &ProviderContext) -> Result<String, ProviderError> {
        ctx.secret
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned()
            .ok_or_else(|| ProviderError::InvalidConfig("API Key 未配置".into()))
    }
}

#[async_trait]
impl Provider for MemefastSeedanceProvider {
    fn meta(&self) -> &ProviderMeta {
        &META
    }

    async fn test_connection(&self, ctx: &ProviderContext) -> ProviderResult<ConnectionStatus> {
        let key = Self::api_key(ctx)?;
        let url = format!(
            "{}/volc/v1/contents/generations/tasks?limit=1",
            Self::base_url(ctx)
        );
        let req = HttpRequest {
            method: HttpMethod::Get,
            url,
            headers: vec![
                ("Authorization".into(), format!("Bearer {}", key)),
                ("Accept".into(), "application/json".into()),
            ],
            body: None,
            timeout: Some(Duration::from_secs(10)),
        };
        let resp = ctx.http.execute(req).await?;
        Ok(ConnectionStatus {
            ok: resp.status == 200,
            message: if resp.status == 200 {
                "连接成功".into()
            } else {
                format!("HTTP {}", resp.status)
            },
        })
    }

    async fn list_models(&self, _ctx: &ProviderContext) -> ProviderResult<ProviderModelList> {
        Ok(ProviderModelList {
            generation: vec![],
            analysis: vec![],
            video: vec![
                "doubao-seedance-1-5-pro-251215".into(),
                "doubao-seedance-pro-t2v".into(),
                "doubao-seedance-lite-t2v".into(),
            ],
        })
    }

    fn as_video_generator(&self) -> Option<&dyn VideoGenerator> {
        Some(self)
    }
    fn as_pollable(&self) -> Option<&dyn Pollable> {
        Some(self)
    }
}

#[async_trait]
impl VideoGenerator for MemefastSeedanceProvider {
    async fn generate_video(
        &self,
        req: VideoGenerationRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<VideoOutput> {
        let key = Self::api_key(ctx)?;
        let base = Self::base_url(ctx);
        let params = req
            .seedance_params
            .ok_or_else(|| ProviderError::InvalidConfig("缺少 Seedance 参数".into()))?;

        params
            .validate_constraints()
            .map_err(|e| ProviderError::InvalidConfig(e))?;

        let mut content: Vec<serde_json::Value> = Vec::new();
        content.push(serde_json::json!({ "type": "text", "text": params.encode_inline_tokens() }));

        for r in &params.references {
            match r.ref_type {
                artait_model::seedance::MediaRefType::FirstFrame =>
                    content.push(serde_json::json!({ "type": "image_url", "image_url": { "url": r.url }, "role": "first_frame" })),
                artait_model::seedance::MediaRefType::LastFrame =>
                    content.push(serde_json::json!({ "type": "image_url", "image_url": { "url": r.url }, "role": "last_frame" })),
                artait_model::seedance::MediaRefType::ReferenceVideo =>
                    content.push(serde_json::json!({ "type": "video_url", "video_url": { "url": r.url } })),
                artait_model::seedance::MediaRefType::ReferenceAudio =>
                    content.push(serde_json::json!({ "type": "audio_url", "audio_url": { "url": r.url } })),
            }
        }

        let body = serde_json::json!({ "model": params.model, "content": content });
        let submit_url = format!("{}/volc/v1/contents/generations/tasks", base);
        info!(url = %submit_url, "提交 Seedance 视频任务");

        let submit_req = HttpRequest {
            method: HttpMethod::Post,
            url: submit_url,
            headers: vec![
                ("Authorization".into(), format!("Bearer {}", key)),
                ("Content-Type".into(), "application/json".into()),
                ("Accept".into(), "application/json".into()),
            ],
            body: Some(Bytes::from(serde_json::to_vec(&body).unwrap_or_default())),
            timeout: Some(Duration::from_secs(30)),
        };
        let resp = ctx.http.execute(submit_req).await?;
        if resp.status != 200 {
            let body_str = String::from_utf8_lossy(&resp.body);
            return Err(ProviderError::ConnectionFailed(format!(
                "HTTP {}: {}",
                resp.status, body_str
            )));
        }

        let submit_result: MemefastSubmitResponse = resp.json()?;
        let task_id = submit_result.id.clone();
        if submit_result.status == "failed" || submit_result.status == "error" {
            let err_msg = submit_result
                .error
                .as_ref()
                .and_then(|e| e.message.clone())
                .unwrap_or_else(|| submit_result.status.clone());
            return Err(ProviderError::InvalidResponse(format!(
                "任务创建失败: {}",
                err_msg
            )));
        }
        info!(task_id = %task_id, "Seedance 视频任务已提交");

        let poll_url = format!("{}/volc/v1/contents/generations/tasks/{}", base, task_id);
        let strategy = video_polling();
        let mut attempt: u32 = 0;

        loop {
            if ctx.is_cancelled() {
                return Err(ProviderError::TaskCancelled);
            }

            let poll_req = HttpRequest {
                method: HttpMethod::Get,
                url: poll_url.clone(),
                headers: vec![
                    ("Authorization".into(), format!("Bearer {}", key)),
                    ("Accept".into(), "application/json".into()),
                ],
                body: None,
                timeout: Some(Duration::from_secs(15)),
            };
            let poll_resp = ctx.http.execute(poll_req).await?;
            if poll_resp.status != 200 {
                return Err(ProviderError::ConnectionFailed(format!(
                    "轮询 HTTP {}",
                    poll_resp.status
                )));
            }

            let task: MemefastTaskResponse = poll_resp.json()?;
            match task.status.as_str() {
                "succeeded" | "completed" => {
                    let video_url = extract_video_url(&task)
                        .ok_or_else(|| ProviderError::InvalidResponse("未找到视频 URL".into()))?;
                    info!(task_id = %task_id, url = %video_url, "视频生成完成");
                    return Ok(VideoOutput {
                        kind: GenerationOutput::Url {
                            url: video_url,
                            metadata: serde_json::to_value(&task).unwrap_or_default(),
                        },
                        duration_seconds: None,
                        has_audio: false,
                    });
                }
                "failed" | "expired" | "cancelled" => {
                    let err_msg = task
                        .error
                        .as_ref()
                        .and_then(|e| e.message.clone())
                        .unwrap_or_else(|| task.status.clone());
                    return Err(ProviderError::InvalidResponse(format!(
                        "任务失败: {}",
                        err_msg
                    )));
                }
                _ => {}
            }

            attempt += 1;
            if attempt >= strategy.max_polls as u32 {
                return Err(ProviderError::ProviderTimeout);
            }
            tokio::time::sleep(strategy.backoff_at(attempt as usize)).await;
        }
    }
}

#[async_trait]
impl Pollable for MemefastSeedanceProvider {
    async fn poll(
        &self,
        provider_task_id: &str,
        ctx: &ProviderContext,
    ) -> ProviderResult<Option<GenerationOutput>> {
        let key = Self::api_key(ctx)?;
        let base = Self::base_url(ctx);
        let url = format!(
            "{}/volc/v1/contents/generations/tasks/{}",
            base, provider_task_id
        );
        let req = HttpRequest {
            method: HttpMethod::Get,
            url,
            headers: vec![
                ("Authorization".into(), format!("Bearer {}", key)),
                ("Accept".into(), "application/json".into()),
            ],
            body: None,
            timeout: Some(Duration::from_secs(15)),
        };
        let resp = ctx.http.execute(req).await?;
        if resp.status != 200 {
            return Ok(None);
        }
        let task: MemefastTaskResponse = resp.json()?;
        match task.status.as_str() {
            "succeeded" | "completed" => {
                let url = extract_video_url(&task).unwrap_or_default();
                Ok(Some(GenerationOutput::Url {
                    url,
                    metadata: serde_json::to_value(&task).unwrap_or_default(),
                }))
            }
            "failed" | "expired" | "cancelled" => Err(ProviderError::InvalidResponse(format!(
                "任务已终止: {}",
                task.status
            ))),
            _ => Ok(None),
        }
    }
}

// JSON types
#[derive(Debug, Deserialize)]
struct MemefastSubmitResponse {
    id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    error: Option<MemefastError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MemefastTaskResponse {
    #[serde(default)]
    id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<MemefastError>,
    #[serde(default)]
    video_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MemefastError {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

fn extract_video_url(task: &MemefastTaskResponse) -> Option<String> {
    if let Some(ref c) = task.content {
        if let Some(u) = c.get("video_url").and_then(|v| v.as_str()) {
            return Some(u.to_string());
        }
        if let Some(o) = c.get("output") {
            if let Some(u) = o.get("video_url").and_then(|v| v.as_str()) {
                return Some(u.to_string());
            }
            if let Some(u) = o.get("url").and_then(|v| v.as_str()) {
                return Some(u.to_string());
            }
        }
    }
    if let Some(ref o) = task.output {
        if let Some(u) = o.get("video_url").and_then(|v| v.as_str()) {
            return Some(u.to_string());
        }
        if let Some(u) = o.get("url").and_then(|v| v.as_str()) {
            return Some(u.to_string());
        }
    }
    task.video_url.clone().or_else(|| task.url.clone())
}
