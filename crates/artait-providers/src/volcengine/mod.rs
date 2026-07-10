//! Volcengine Seedance 协议族（火山引擎图片生成）。
//!
//! 支持图片生成（seedancetoimage_v2）：提交异步任务 + 轮询 → 返回 URL。
//! 认证：Access Key + Secret Key，HMAC-SHA256 签名。

use std::time::Duration;

use artait_model::{
    ConnectionStatus, CreationMode, ProviderCapabilities, ProviderError, ProviderFamily,
};
use artait_provider::{
    http::{HttpMethod, HttpRequest},
    meta::ProviderMeta,
    request::{GenerationOutput, ImageGenerationRequest},
    ImageGenerator, Pollable, PollingStrategy, Provider, ProviderContext, ProviderModelList,
    ProviderResult,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const META: ProviderMeta = ProviderMeta {
    id: "volcengine-seedance",
    display_name: "火山引擎 Seedance",
    family: ProviderFamily::VolcengineSeedance,
    capabilities: ProviderCapabilities {
        generate: true,
        generate_character: false,
        generate_video: false,
        analyze: false,
        test_connection: true,
        quota: false,
        upload_binary: false,
        poll_task: true,
    },
    default_generation_models: &["seedancetoimage_v2"],
    default_analysis_models: &[],
    default_video_models: &[],
    config_schema: r#"{"type":"object","properties":{"access_key":{"type":"string","title":"Access Key"},"secret_key":{"type":"string","title":"Secret Key","format":"password"}}}"#,
    is_legacy: false,
};

const BASE_URL: &str = "https://visual.volcengineapi.com";
const SERVICE: &str = "cv";
const REGION: &str = "cn-north-1";

/// Volcengine Seedance 默认轮询策略：3 秒间隔，最多 60 次（≈3 分钟）。
fn seedance_polling() -> PollingStrategy {
    PollingStrategy {
        interval: std::time::Duration::from_secs(3),
        max_polls: 60,
        ..Default::default()
    }
}

// ── Provider ──────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct VolcengineSeedanceProvider;

impl VolcengineSeedanceProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Provider for VolcengineSeedanceProvider {
    fn meta(&self) -> &ProviderMeta {
        &META
    }

    async fn test_connection(&self, ctx: &ProviderContext) -> ProviderResult<ConnectionStatus> {
        let (ak, sk) = extract_keys(ctx)?;
        let body = serde_json::json!({ "req_key": "seedancetoimage_v2", "return_url": true });
        let req = signed_request("CVSync2AsyncSubmitTask", &body, &ak, &sk)?;
        let resp = ctx.http.execute(req).await?;
        if resp.status == 200 {
            let parsed: VolcResponse = resp.json()?;
            if parsed.code == 0 || parsed.code == 10000 {
                Ok(ConnectionStatus {
                    ok: true,
                    message: "连接成功".into(),
                })
            } else {
                Ok(ConnectionStatus {
                    ok: false,
                    message: format!(
                        "code={}: {}",
                        parsed.code,
                        parsed.message.unwrap_or_default()
                    ),
                })
            }
        } else {
            Ok(ConnectionStatus {
                ok: false,
                message: format!("HTTP {}", resp.status),
            })
        }
    }

    async fn list_models(&self, _ctx: &ProviderContext) -> ProviderResult<ProviderModelList> {
        Ok(ProviderModelList {
            generation: vec!["seedancetoimage_v2".into()],
            analysis: Vec::new(),
            video: Vec::new(),
        })
    }

    fn as_image_generator(&self) -> Option<&dyn ImageGenerator> {
        Some(self)
    }
    fn as_pollable(&self) -> Option<&dyn Pollable> {
        Some(self)
    }
}

// ── ImageGenerator ────────────────────────────────────────────────────────

#[async_trait]
impl ImageGenerator for VolcengineSeedanceProvider {
    async fn generate(
        &self,
        req: ImageGenerationRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<GenerationOutput> {
        let (ak, sk) = extract_keys(ctx)?;
        let prompt = build_prompt(&req);
        let body = serde_json::json!({
            "req_key": model_or_default(&req),
            "prompt": prompt,
            "return_url": true,
            "logo_info": { "add_logo": false },
        });

        let submit = signed_request("CVSync2AsyncSubmitTask", &body, &ak, &sk)?;
        let resp = ctx.http.execute(submit).await?;
        if resp.status != 200 {
            return Err(ProviderError::ConnectionFailed(format!(
                "HTTP {}",
                resp.status
            )));
        }
        let parsed: VolcResponse = resp.json()?;
        let task_id = parsed
            .data
            .as_ref()
            .and_then(|d| d.task_id.as_deref())
            .ok_or_else(|| {
                ProviderError::InvalidResponse(format!(
                    "Seedance 提交失败 code={}: {}",
                    parsed.code,
                    parsed.message.unwrap_or_default()
                ))
            })?;

        // 轮询直到完成
        let result = poll_until_done(task_id, &ak, &sk, ctx, &seedance_polling()).await?;
        let image_url = result
            .data
            .as_ref()
            .and_then(|d| d.image_urls.as_ref())
            .and_then(|urls| urls.first())
            .ok_or_else(|| ProviderError::InvalidResponse("Seedance 未返回图片 URL".into()))?;

        Ok(GenerationOutput::Url {
            url: image_url.clone(),
            metadata: serde_json::to_value(&result).unwrap_or_default(),
        })
    }
}

// ── Pollable (供 re-acquire 使用) ─────────────────────────────────────────

#[async_trait]
impl Pollable for VolcengineSeedanceProvider {
    async fn poll(
        &self,
        provider_task_id: &str,
        ctx: &ProviderContext,
    ) -> ProviderResult<Option<GenerationOutput>> {
        let (ak, sk) = extract_keys(ctx)?;
        let body = serde_json::json!({
            "req_key": "seedancetoimage_v2",
            "task_id": provider_task_id,
            "req_json": r#"{"return_url":true}"#,
        });

        let req = signed_request("CVSync2AsyncGetResult", &body, &ak, &sk)?;
        let resp = ctx.http.execute(req).await?;
        if resp.status != 200 {
            return Err(ProviderError::ConnectionFailed(format!(
                "HTTP {}",
                resp.status
            )));
        }
        let parsed: VolcResponse = resp.json()?;

        match parsed.code {
            0 => {
                let image_url = parsed
                    .data
                    .as_ref()
                    .and_then(|d| d.image_urls.as_ref())
                    .and_then(|urls| urls.first());
                Ok(image_url.map(|url| GenerationOutput::Url {
                    url: url.clone(),
                    metadata: serde_json::to_value(&parsed).unwrap_or_default(),
                }))
            }
            10001 => Ok(None), // 仍在处理中
            _ => Err(ProviderError::InvalidResponse(format!(
                "Seedance 轮询失败 code={}: {}",
                parsed.code,
                parsed.message.unwrap_or_default()
            ))),
        }
    }
}

// ── 轮询 ──────────────────────────────────────────────────────────────────

async fn poll_until_done(
    task_id: &str,
    ak: &str,
    sk: &str,
    ctx: &ProviderContext,
    strategy: &PollingStrategy,
) -> ProviderResult<VolcResponse> {
    let body = serde_json::json!({
        "req_key": "seedancetoimage_v2",
        "task_id": task_id,
        "req_json": r#"{"return_url":true}"#,
    });

    for attempt in 0..strategy.max_polls {
        if ctx.is_cancelled() {
            return Err(ProviderError::TaskCancelled);
        }
        let req = signed_request("CVSync2AsyncGetResult", &body, ak, sk)?;
        let resp = ctx.http.execute(req).await?;
        if resp.status != 200 {
            return Err(ProviderError::ConnectionFailed(format!(
                "HTTP {}",
                resp.status
            )));
        }
        let parsed: VolcResponse = resp.json()?;
        match parsed.code {
            0 => return Ok(parsed),
            10001 => {
                let delay = strategy.backoff_at(attempt);
                tokio::time::sleep(delay).await;
                continue;
            }
            _ => {
                return Err(ProviderError::InvalidResponse(format!(
                    "Seedance 任务失败 code={}: {}",
                    parsed.code,
                    parsed.message.unwrap_or_default()
                )));
            }
        }
    }
    Err(ProviderError::ProviderTimeout)
}

// ── 签名 (Volcengine HMAC-SHA256 V4) ─────────────────────────────────────

fn signed_request(
    _action: &str,
    body: &serde_json::Value,
    ak: &str,
    sk: &str,
) -> ProviderResult<HttpRequest> {
    let body_str = serde_json::to_string(body)
        .map_err(|e| ProviderError::InvalidResponse(format!("序列化失败: {e}")))?;
    let body_bytes = bytes::Bytes::from(body_str.clone());

    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = timestamp[..8].to_string();
    let host = "visual.volcengineapi.com";
    let content_type = "application/json";
    let signed_headers = "content-type;host;x-date";
    let canonical_headers =
        format!("content-type:{content_type}\nhost:{host}\nx-date:{timestamp}\n");
    let payload_hash = sha256_hex(&body_str);

    let canonical_request = format!(
        "POST\n/\n\n{}\n{}\n{}",
        canonical_headers, signed_headers, payload_hash
    );

    let credential_scope = format!("{date}/{REGION}/{SERVICE}/request");
    let string_to_sign = format!(
        "HMAC-SHA256\n{timestamp}\n{credential_scope}\n{}",
        sha256_hex(&canonical_request)
    );

    let signing_key = hmac_sha256(
        &hmac_sha256(
            &hmac_sha256(&hmac_sha256(sk.as_bytes(), &date), REGION),
            SERVICE,
        ),
        "request",
    );

    let signature = hex::encode(hmac_sha256_bytes(&signing_key, string_to_sign.as_bytes()));
    let authorization = format!(
        "HMAC-SHA256 Credential={ak}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );

    Ok(HttpRequest {
        method: HttpMethod::Post,
        url: BASE_URL.to_string(),
        headers: vec![
            ("Content-Type".into(), content_type.into()),
            ("Host".into(), host.into()),
            ("X-Date".into(), timestamp),
            ("Authorization".into(), authorization),
        ],
        body: Some(body_bytes),
        timeout: Some(Duration::from_secs(300)),
    })
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(input.as_bytes()))
}

fn hmac_sha256(key: &[u8], msg: &str) -> Vec<u8> {
    hmac_sha256_bytes(key, msg.as_bytes())
}

fn hmac_sha256_bytes(key: &[u8], msg: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key size");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

// ── 密钥 ──────────────────────────────────────────────────────────────────

fn extract_keys(ctx: &ProviderContext) -> ProviderResult<(String, String)> {
    let ak = ctx
        .extra
        .get("access_key")
        .and_then(|v| v.as_str())
        .or_else(|| ctx.secret.as_deref().and_then(|s| s.split(':').next()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ProviderError::InvalidConfig(
                "火山引擎需要 Access Key。请在 extra 中设置 access_key，或将 secret 设为 'ak:sk' 格式".into(),
            )
        })?;

    let sk = ctx
        .extra
        .get("secret_key")
        .and_then(|v| v.as_str())
        .or_else(|| ctx.secret.as_deref().and_then(|s| s.split(':').nth(1)))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ProviderError::InvalidConfig(
                "火山引擎需要 Secret Key。请在 extra 中设置 secret_key，或将 secret 设为 'ak:sk' 格式".into(),
            )
        })?;

    Ok((ak.to_string(), sk.to_string()))
}

// ── 辅助 ──────────────────────────────────────────────────────────────────

fn model_or_default(req: &ImageGenerationRequest) -> &str {
    req.metadata
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("seedancetoimage_v2")
}

fn build_prompt(req: &ImageGenerationRequest) -> String {
    let mut parts = vec![req.prompt.clone()];
    if let Some(ref neg) = req.negative_prompt {
        if !neg.trim().is_empty() {
            parts.push(format!("negative prompt: {neg}"));
        }
    }
    match req.mode {
        CreationMode::Scene => parts.push("style: cinematic, 8k, photorealistic".into()),
        CreationMode::Character => parts.push("style: character design sheet, detailed".into()),
        CreationMode::AnimationScene | CreationMode::AnimationCharacter => {
            parts.push("style: anime, 2d animation, clean lines".into())
        }
        _ => {}
    }
    parts.join(", ")
}

// ── 响应类型 ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VolcResponse {
    code: i32,
    message: Option<String>,
    data: Option<VolcData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VolcData {
    task_id: Option<String>,
    status: Option<String>,
    image_urls: Option<Vec<String>>,
    video_urls: Option<Vec<String>>,
    width: Option<u32>,
    height: Option<u32>,
}
