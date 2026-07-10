use std::time::Duration;

use super::analyze;
use super::endpoint;
use super::media::{image_data_url, image_inline_data, image_multipart_bytes};
use super::params::{self, ImageParams};
use super::response;
use super::upload_cache;
use artait_model::ProviderError;
use artait_model::ReferenceImage;
use artait_provider::{
    http::HttpRequest,
    request::{GenerationOutput, ImageGenerationRequest},
    ProviderContext, ProviderResult,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct OpenAiImagesRequest<'a> {
    pub(crate) model: &'a str,
    pub(crate) prompt: &'a str,
    pub(crate) n: u32,
    pub(crate) size: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resolution: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) response_format: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quality: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image_size: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) aspect_ratio: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image_urls: Option<&'a [&'a str]>,
}

#[derive(Deserialize)]
pub(crate) struct OpenAiImagesResponse {
    pub(crate) data: Vec<OpenAiImageItem>,
}

#[derive(Deserialize)]
pub(crate) struct OpenAiImageItem {
    #[serde(default)]
    pub(crate) url: Option<String>,
    #[serde(default)]
    pub(crate) b64_json: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ToApisImageTaskResponse {
    pub(crate) id: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) progress: Option<u32>,
    #[serde(default)]
    pub(crate) url: Option<String>,
    #[serde(default)]
    pub(crate) result: Option<ToApisImageResult>,
    #[serde(default)]
    pub(crate) error: Option<ToApisError>,
}

#[derive(Deserialize)]
pub(crate) struct ToApisImageResult {
    #[serde(default)]
    pub(crate) data: Vec<OpenAiImageItem>,
}

#[derive(Deserialize)]
pub(crate) struct ToApisError {
    #[serde(default)]
    pub(crate) code: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) message: Option<String>,
}

pub(crate) struct MultipartBody {
    boundary: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) async fn generate_with_openai_images(
    req: ImageGenerationRequest,
    ctx: &ProviderContext,
    model: &str,
) -> ProviderResult<GenerationOutput> {
    let style = params::image_compat_style(ctx);
    let uses_gpt_image_2_body =
        params::is_gpt_image_2_model(model) && style.uses_toapis_gpt_image_body();
    if !req.reference_images.is_empty() {
        return generate_with_openai_image_edits(req, ctx, model).await;
    }

    let base = endpoint::EndpointBase::from_ctx(ctx)?;
    let params = params::pick_openai_image_params(model, &req, style);
    let image_urls = if uses_gpt_image_2_body && !req.reference_images.is_empty() {
        Some(reference_image_urls(&req.reference_images)?)
    } else {
        None
    };
    let image_url_refs = image_urls
        .as_ref()
        .map(|urls| urls.iter().map(String::as_str).collect::<Vec<_>>());
    let body = OpenAiImagesRequest {
        model,
        prompt: &req.prompt,
        n: req.count.clamp(1, 4),
        size: params.size,
        resolution: params.resolution,
        response_format: params.response_format,
        quality: params.quality,
        image_size: params.image_size,
        aspect_ratio: params.aspect_ratio,
        image_urls: image_url_refs.as_deref(),
    };
    let mut last_error = String::new();
    let url_candidates = base.openai_images_url_candidates();
    let url_candidate_total = url_candidates.len();
    for (idx, url) in url_candidates.into_iter().enumerate() {
        let candidate = format!("{}/{}", idx + 1, url_candidate_total);
        tracing::info!(
            target: "provider",
            provider = %ctx.instance_id,
            api_style = style.as_str(),
            api = "openai_images",
            candidate = %candidate,
            url = %url,
            model = %model,
            size = params.size,
            resolution = params.resolution.unwrap_or(""),
            image_size = params.image_size.unwrap_or(""),
            aspect_ratio = params.aspect_ratio.unwrap_or(""),
            refs = req.reference_images.len(),
            "image generation request"
        );
        let http_req = endpoint::apply_auth(
            HttpRequest::post(url.clone()),
            ctx,
            endpoint::AnalysisApi::OpenAiChat,
        )?
        .timeout(Duration::from_secs(180))
        .json_body(&body)?;
        let resp = ctx.http.execute(http_req).await?;
        if resp.is_success() {
            let metadata = serde_json::json!({
                "api": "openai_images",
                "endpoint_url": url,
                "model": model,
                "size": params.size,
                "pixel_size": params.pixel_size,
                "resolution": params.resolution,
                "response_format": params.response_format,
                "quality": params.quality,
                "image_size": params.image_size,
                "aspect_ratio": params.aspect_ratio,
                "count": body.n,
            });
            return response::output_from_openai_images_response(resp, ctx, &url, metadata).await;
        }

        let status = resp.status;
        let error_message = super::OpenAiCompatibleProvider::read_error_message(&resp);
        last_error = format!("{url} -> HTTP {status}: {error_message}");
        match status {
            404 | 405 => {
                tracing::warn!(
                    target: "provider",
                    provider = %ctx.instance_id,
                    api_style = style.as_str(),
                    candidate = %candidate,
                    url = %url,
                    status,
                    error = %error_message,
                    "image generation endpoint unavailable, trying next candidate"
                );
                continue;
            }
            _ => return Err(super::OpenAiCompatibleProvider::rejected(&resp)),
        }
    }

    Err(ProviderError::ProviderRejected(format!(
        "所有生图端点均不可用：{last_error}"
    )))
}

pub(crate) async fn generate_with_openai_image_edits(
    req: ImageGenerationRequest,
    ctx: &ProviderContext,
    model: &str,
) -> ProviderResult<GenerationOutput> {
    let base = endpoint::EndpointBase::from_ctx(ctx)?;
    let style = params::image_compat_style(ctx);
    let params = params::pick_openai_image_params(model, &req, style);
    let body = build_openai_image_edit_multipart(model, &req, &params)?;

    let mut last_error = String::new();
    let url_candidates = base.openai_image_edits_url_candidates();
    let url_candidate_total = url_candidates.len();
    for (idx, url) in url_candidates.into_iter().enumerate() {
        let candidate = format!("{}/{}", idx + 1, url_candidate_total);
        tracing::info!(
            target: "provider",
            provider = %ctx.instance_id,
            api_style = style.as_str(),
            api = "openai_image_edits",
            candidate = %candidate,
            url = %url,
            model = %model,
            size = params.size,
            refs = req.reference_images.len(),
            "image edit request"
        );
        let http_req = endpoint::apply_auth(
            HttpRequest::post(url.clone()),
            ctx,
            endpoint::AnalysisApi::OpenAiChat,
        )?
        .timeout(Duration::from_secs(180))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={}", body.boundary),
        )
        .body(body.bytes.clone());
        let resp = ctx.http.execute(http_req).await?;
        if resp.is_success() {
            let parsed: OpenAiImagesResponse = resp.json()?;
            let item = parsed
                .data
                .into_iter()
                .next()
                .ok_or_else(|| ProviderError::InvalidResponse("响应没有 data[0]".into()))?;
            let metadata = serde_json::json!({
                "api": "openai_images_edits",
                "endpoint_url": url,
                "model": model,
                "size": params.size,
                "pixel_size": params.pixel_size,
                "resolution": params.resolution,
                "response_format": params.response_format,
                "quality": params.quality,
                "image_size": params.image_size,
                "aspect_ratio": params.aspect_ratio,
                "reference_count": req.reference_images.len(),
                "count": req.count.clamp(1, 4),
            });
            return response::output_from_image_item(item.url, item.b64_json, metadata);
        }

        let status = resp.status;
        let error_message = super::OpenAiCompatibleProvider::read_error_message(&resp);
        last_error = format!("{url} -> HTTP {status}: {error_message}");
        match status {
            404 | 405 => {
                tracing::warn!(
                    target: "provider",
                    provider = %ctx.instance_id,
                    api_style = style.as_str(),
                    candidate = %candidate,
                    url = %url,
                    status,
                    error = %error_message,
                    "image edit endpoint unavailable, trying next candidate"
                );
                continue;
            }
            _ => return Err(super::OpenAiCompatibleProvider::rejected(&resp)),
        }
    }

    Err(ProviderError::ProviderRejected(format!(
        "所有图生图端点均不可用：{last_error}"
    )))
}

pub(crate) async fn generate_with_gemini(
    req: ImageGenerationRequest,
    ctx: &ProviderContext,
    model: &str,
) -> ProviderResult<GenerationOutput> {
    let base = endpoint::EndpointBase::from_ctx(ctx)?;
    let url = base.gemini_generate_content_url(model);
    let params = params::pick_image_params(model, &req);
    let body = build_gemini_image_body(&req, model, &params, !base.is_gemini_base())?;
    let http_req = endpoint::apply_gemini_auth(url, ctx)?
        .timeout(Duration::from_secs(180))
        .json_body(&body)?;
    let resp = ctx.http.execute(http_req).await?;
    if !resp.is_success() {
        return Err(super::OpenAiCompatibleProvider::rejected(&resp));
    }
    let parsed: analyze::GeminiResponse = resp.json()?;
    let (mime, data) = parsed
        .first_inline_data()
        .ok_or_else(|| ProviderError::InvalidResponse("Gemini 响应没有 inlineData".into()))?;
    Ok(GenerationOutput::Base64 {
        data,
        mime,
        metadata: serde_json::json!({
            "api": "gemini_generate_content",
            "model": model,
            "size": params.size,
            "pixel_size": params.pixel_size,
            "resolution": params.resolution,
            "image_size": params.image_size,
            "aspect_ratio": params.aspect_ratio,
            "quality": req.quality,
        }),
    })
}

pub(crate) fn build_openai_image_edit_multipart(
    model: &str,
    req: &ImageGenerationRequest,
    params: &ImageParams,
) -> ProviderResult<MultipartBody> {
    if req.reference_images.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "/v1/images/edits 需要至少一张参考图".into(),
        ));
    }

    let boundary = multipart_boundary();
    let mut out = Vec::new();
    multipart_text(&mut out, &boundary, "model", model);
    multipart_text(&mut out, &boundary, "prompt", &req.prompt);
    multipart_text(&mut out, &boundary, "n", &req.count.clamp(1, 4).to_string());
    multipart_text(&mut out, &boundary, "size", params.size);
    if let Some(resolution) = params.resolution {
        multipart_text(&mut out, &boundary, "resolution", resolution);
    }
    if let Some(response_format) = params.response_format {
        multipart_text(&mut out, &boundary, "response_format", response_format);
    }
    if let Some(quality) = params.quality {
        multipart_text(&mut out, &boundary, "quality", quality);
    }
    if let Some(image_size) = params.image_size {
        multipart_text(&mut out, &boundary, "image_size", image_size);
    }
    if let Some(aspect_ratio) = params.aspect_ratio {
        multipart_text(&mut out, &boundary, "aspect_ratio", aspect_ratio);
    }

    let image_field = if req.reference_images.len() == 1 {
        "image"
    } else {
        "image[]"
    };
    for img in &req.reference_images {
        let field = image_field;
        multipart_image(&mut out, &boundary, field, img)?;
    }

    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(MultipartBody {
        boundary,
        bytes: out,
    })
}

pub(crate) fn reference_image_urls(images: &[ReferenceImage]) -> ProviderResult<Vec<String>> {
    images
        .iter()
        .map(|img| {
            // 优先使用已上传的公网 URL，否则转为 base64 data URL
            if let Some(url) = &img.uploaded_url {
                if !url.trim().is_empty() {
                    return Ok(url.clone());
                }
            }
            image_data_url(img)
        })
        .collect()
}

/// 上传超过 10MB 的参考图到图床，返回 public URL。
/// 成功时更新 `ReferenceImage.uploaded_url`。
pub(crate) async fn upload_large_reference_images(
    req: &mut ImageGenerationRequest,
    ctx: &ProviderContext,
) -> ProviderResult<()> {
    const THRESHOLD_BYTES: u64 = 10 * 1024 * 1024; // 10 MB
    let output_dir = &ctx.output_path;

    for img in &mut req.reference_images {
        // 已有有效 URL 则跳过（可能是之前拖入/上传的）
        if let Some(ref url) = img.uploaded_url {
            if !url.trim().is_empty() {
                continue;
            }
        }
        // 检查文件大小
        let file_size = match std::fs::metadata(&img.local_path) {
            Ok(meta) => meta.len(),
            Err(_) => continue,
        };
        if file_size < THRESHOLD_BYTES {
            // 小文件也查一下缓存（可能之前其他生图任务已上传过）
            if let Some(cached) = upload_cache::lookup_cached_url(&img.local_path, output_dir) {
                tracing::debug!(
                    target: "provider",
                    file = %img.local_path.display(),
                    url = %cached,
                    "参考图命中上传缓存"
                );
                img.uploaded_url = Some(cached);
            }
            continue;
        }

        // 先查缓存
        if let Some(cached) = upload_cache::lookup_cached_url(&img.local_path, output_dir) {
            tracing::info!(
                target: "provider",
                file = %img.local_path.display(),
                url = %cached,
                "large reference image hit upload cache"
            );
            img.uploaded_url = Some(cached);
            continue;
        }

        tracing::info!(
            target: "provider",
            provider = %ctx.instance_id,
            file = %img.local_path.display(),
            size_mb = file_size / (1024 * 1024),
            "uploading large reference image"
        );

        match upload_ref_image(ctx, img).await {
            Ok(url) => {
                tracing::info!(
                    target: "provider",
                    provider = %ctx.instance_id,
                    url = %url,
                    "reference image uploaded"
                );
                upload_cache::save_cached_url(&img.local_path, output_dir, &url, &ctx.provider_id);
                img.uploaded_url = Some(url);
            }
            Err(e) => {
                tracing::warn!(
                    target: "provider",
                    provider = %ctx.instance_id,
                    error = %e,
                    "reference image upload failed, will fall back to base64"
                );
            }
        }
    }
    Ok(())
}

async fn upload_ref_image(ctx: &ProviderContext, img: &ReferenceImage) -> ProviderResult<String> {
    let upload_url = ctx
        .extra
        .get("image_upload_api_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "https://api.imgbb.com/1/upload".to_string());

    let api_key = ctx
        .extra
        .get("image_upload_api_key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ctx.secret.as_deref().unwrap_or(""));

    let is_imgbb = upload_url.contains("imgbb.com");

    let file_name = img
        .local_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image");
    let file_bytes = std::fs::read(&img.local_path).map_err(|e| {
        ProviderError::Io(format!("读取参考图失败 {}: {e}", img.local_path.display()))
    })?;
    let mime = img.mime_type.as_str();
    let mime = if mime.is_empty() { "image/png" } else { mime };

    let part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name(file_name.to_string())
        .mime_str(mime)
        .map_err(|e| ProviderError::InvalidConfig(format!("invalid mime type: {e}")))?;
    let field_name = if is_imgbb { "image" } else { "file" };
    let form = reqwest::multipart::Form::new().part(field_name, part);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| ProviderError::Io(format!("failed to build upload client: {e}")))?;

    let mut req_builder = client.post(&upload_url).multipart(form);

    if is_imgbb {
        // ImgBB: key 作为 query param
        req_builder = req_builder.query(&[("key", api_key)]);
    } else {
        // 通用图床：Bearer auth
        req_builder = req_builder.header("Authorization", format!("Bearer {api_key}"));
    }

    let resp = req_builder
        .send()
        .await
        .map_err(|e| ProviderError::Io(format!("upload failed: {e}")))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| ProviderError::Io(format!("failed to read upload response: {e}")))?;

    if !status.is_success() {
        return Err(ProviderError::ProviderRejected(format!(
            "upload HTTP {status}: {body}"
        )));
    }

    extract_upload_url(&body, is_imgbb)
}

fn extract_upload_url(body: &str, is_imgbb: bool) -> ProviderResult<String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::InvalidResponse(format!("upload response is not JSON: {e}")))?;

    if is_imgbb {
        // ImgBB: {"data":{"url":"..."},"success":true}
        if !value
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let msg = value
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(ProviderError::ProviderRejected(format!("ImgBB: {msg}")));
        }
        return value
            .pointer("/data/url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_owned())
            .ok_or_else(|| {
                ProviderError::InvalidResponse("ImgBB response missing data.url".into())
            });
    }

    // 通用图床
    for path in [
        &["data", "url"][..],
        &["data", "display_url"][..],
        &["url"][..],
        &["image_url"][..],
    ] {
        if let Some(url) = get_nested_str(&value, path) {
            if !url.trim().is_empty() {
                return Ok(url.to_owned());
            }
        }
    }
    Err(ProviderError::InvalidResponse(
        "upload response does not contain a URL".into(),
    ))
}

fn get_nested_str<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str()
}

pub(crate) fn multipart_boundary() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("artait-{nanos}")
}

pub(crate) fn multipart_text(out: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
    );
    out.extend_from_slice(value.as_bytes());
    out.extend_from_slice(b"\r\n");
}

pub(crate) fn multipart_image(
    out: &mut Vec<u8>,
    boundary: &str,
    name: &str,
    img: &ReferenceImage,
) -> ProviderResult<()> {
    let (mime, bytes) = image_multipart_bytes(img)?;
    let filename = sanitize_multipart_filename(&img.display_name);
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    out.extend_from_slice(format!("Content-Type: {mime}\r\n\r\n").as_bytes());
    out.extend_from_slice(&bytes);
    out.extend_from_slice(b"\r\n");
    Ok(())
}

pub(crate) fn sanitize_multipart_filename(name: &str) -> String {
    let trimmed = name.trim();
    let fallback = if trimmed.is_empty() {
        "image.png"
    } else {
        trimmed
    };
    fallback
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\r' | '\n' => '_',
            _ => c,
        })
        .collect()
}

pub(crate) fn build_gemini_image_body(
    req: &ImageGenerationRequest,
    model: &str,
    params: &ImageParams,
    include_compat_size: bool,
) -> ProviderResult<analyze::GeminiRequest> {
    let mut parts = vec![analyze::GeminiPart {
        text: Some(req.prompt.clone()),
        inline_data: None,
    }];
    for img in &req.reference_images {
        let (mime_type, data) = image_inline_data(img)?;
        parts.push(analyze::GeminiPart {
            text: None,
            inline_data: Some(analyze::GeminiInlineData { mime_type, data }),
        });
    }
    Ok(analyze::GeminiRequest {
        contents: vec![analyze::GeminiContent {
            role: Some("user".into()),
            parts,
        }],
        system_instruction: None,
        generation_config: Some(analyze::GeminiGenerationConfig {
            response_mime_type: None,
            response_modalities: Some(vec!["TEXT".into(), "IMAGE".into()]),
            response_format: Some(analyze::GeminiResponseFormat {
                image: analyze::GeminiImageConfig {
                    aspect_ratio: params
                        .aspect_ratio
                        .unwrap_or_else(|| {
                            params::gemini_aspect_ratio_for_model(
                                model,
                                req.aspect_ratio.as_deref().unwrap_or("1:1"),
                            )
                        })
                        .into(),
                    image_size: params.image_size.map(str::to_string),
                },
            }),
        }),
        size: include_compat_size.then(|| params.size.to_string()),
    })
}
