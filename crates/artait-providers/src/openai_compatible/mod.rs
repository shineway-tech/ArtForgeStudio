//! OpenAI 兼容协议族。
//!
//! 推理兼容:
//! - /v1/chat/completions
//! - /v1/responses
//! - /v1/messages
//! - /v1beta/models/{model}:generateContent
//! - /openai/v1/chat/completions
//! - /api/v1/chat/completions
//!
//! 生图兼容:
//! - /v1/images/generations
//! - /v1/images/edits
//! - /v1beta/models/{model}:generateContent

pub(crate) mod analyze;
pub(crate) mod endpoint;
pub(crate) mod generate;
pub(crate) mod media;
pub(crate) mod params;
pub(crate) mod response;
pub(crate) mod upload_cache;

use std::time::Duration;

#[cfg(test)]
use artait_model::ReferenceImage;
use artait_model::{ConnectionStatus, ProviderCapabilities, ProviderError, ProviderFamily};
use artait_provider::{
    http::{HttpRequest, HttpResponse},
    meta::ProviderMeta,
    request::{AnalysisOutput, AnalysisRequest, GenerationOutput, ImageGenerationRequest},
    Analyzer, ImageGenerator, Provider, ProviderContext, ProviderModelList, ProviderResult,
};
use async_trait::async_trait;
use endpoint::{AnalysisApi, EndpointBase, EndpointPlan, ImageApi};
use generate::{generate_with_gemini, generate_with_openai_images};
use serde::Deserialize;

pub use params::is_gpt_image_2_model;

const META: ProviderMeta = ProviderMeta {
    id: "openai-compatible",
    display_name: "OpenAI 兼容",
    family: ProviderFamily::OpenAiCompatible,
    capabilities: ProviderCapabilities {
        generate: true,
        generate_character: false,
        generate_video: false,
        analyze: true,
        test_connection: true,
        quota: false,
        upload_binary: false,
        poll_task: false,
    },
    default_generation_models: &["gpt-image-1", "dall-e-3", "gemini-2.5-flash-image"],
    default_analysis_models: &[
        "gpt-4o-mini",
        "gpt-4o",
        "claude-3-5-sonnet-latest",
        "gemini-2.5-flash",
    ],
    default_video_models: &[],
    config_schema: r#"{
  "type": "object",
  "properties": {
    "endpoint": {
      "type": "string",
      "format": "uri",
      "label": "API 端点",
      "default": "https://api.openai.com/v1",
      "help": "可填 base URL 或具体接口路径，如 /v1、/openai/v1、/api/v1、Gemini v1beta"
    },
    "api_key": {
      "type": "string",
      "secret": true,
      "label": "API Key",
      "help": "保存到本机 app_config.toml"
    },
    "generation_model": {
      "type": "string",
      "label": "生图模型",
      "default": "gpt-image-1"
    },
    "analysis_model": {
      "type": "string",
      "label": "推理模型",
      "default": "gpt-4o-mini"
    },
    "api_style": {
      "type": "string",
      "label": "接口风格",
      "enum": ["newapi", "cpa", "sub2api", "gemini", "auto", "chat", "responses", "messages", "images", "toapis", "embeddings", "rerank"],
      "default": "newapi"
    }
  },
  "required": ["endpoint", "api_key"]
}"#,
    is_legacy: false,
};

#[derive(Default)]
pub struct OpenAiCompatibleProvider;

impl OpenAiCompatibleProvider {
    fn read_error_message(resp: &HttpResponse) -> String {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&resp.body) {
            if let Some(message) = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
            {
                return message.to_string();
            }
            if let Some(message) = v.get("message").and_then(|m| m.as_str()) {
                return message.to_string();
            }
            if let Some(error) = v.get("error").and_then(|m| m.as_str()) {
                return error.to_string();
            }
        }
        resp.text().chars().take(300).collect()
    }

    fn rejected(resp: &HttpResponse) -> ProviderError {
        match resp.status {
            401 | 403 => ProviderError::ProviderRejected(format!(
                "HTTP {}: 鉴权失败（API Key 错误或权限不足）",
                resp.status
            )),
            429 => ProviderError::RateLimited,
            _ => ProviderError::ProviderRejected(format!(
                "HTTP {}: {}",
                resp.status,
                Self::read_error_message(resp)
            )),
        }
    }
}

#[derive(Deserialize)]
struct OpenAiModelListResponse {
    #[serde(default)]
    data: Vec<OpenAiModelItem>,
}

#[derive(Deserialize)]
struct OpenAiModelItem {
    id: String,
}

#[derive(Deserialize)]
struct GeminiModelListResponse {
    #[serde(default)]
    models: Vec<GeminiModelItem>,
}

#[derive(Deserialize)]
struct GeminiModelItem {
    name: String,
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn meta(&self) -> &ProviderMeta {
        &META
    }

    async fn test_connection(&self, ctx: &ProviderContext) -> ProviderResult<ConnectionStatus> {
        let base = EndpointBase::from_ctx(ctx)?;
        let model = response::pick_analysis_model(ctx, "gpt-4o-mini");
        let mut last_error = String::new();

        for url in base.model_endpoint_candidates(&model) {
            let api = if base.is_gemini_base() {
                AnalysisApi::GeminiGenerateContent
            } else {
                AnalysisApi::OpenAiChat
            };
            let req = match api {
                AnalysisApi::GeminiGenerateContent => {
                    endpoint::apply_gemini_auth(url.clone(), ctx)?
                }
                _ => endpoint::apply_auth(HttpRequest::get(url.clone()), ctx, api)?,
            }
            .timeout(Duration::from_secs(15));

            let resp = ctx.http.execute(req).await?;
            if resp.is_success() {
                return Ok(ConnectionStatus {
                    ok: true,
                    message: format!("HTTP {} · 端点可达 · {}", resp.status, url),
                });
            }
            last_error = format!(
                "{} -> HTTP {}: {}",
                url,
                resp.status,
                Self::read_error_message(&resp)
            );
            if matches!(resp.status, 401 | 403 | 429) {
                return Err(Self::rejected(&resp));
            }
        }

        Err(ProviderError::ProviderRejected(last_error))
    }

    async fn list_models(&self, ctx: &ProviderContext) -> ProviderResult<ProviderModelList> {
        let base = EndpointBase::from_ctx(ctx)?;
        let analysis_model = response::pick_analysis_model(ctx, "gpt-4o-mini");
        let mut last_error = String::new();

        for url in base.model_endpoint_candidates(&analysis_model) {
            let req = if base.is_gemini_base() {
                let secret = endpoint::require_secret(ctx)?;
                HttpRequest::get(url.clone())
                    .bearer(secret)
                    .header("x-goog-api-key", secret)
                    .header("X-Goog-Api-Key", secret)
            } else {
                endpoint::apply_auth(HttpRequest::get(url.clone()), ctx, AnalysisApi::OpenAiChat)?
            }
            .timeout(Duration::from_secs(20));

            let resp = ctx.http.execute(req).await?;
            if resp.is_success() {
                if base.is_gemini_base() {
                    let parsed: GeminiModelListResponse = resp.json()?;
                    return Ok(params::classify_models(
                        parsed.models.into_iter().map(|item| item.name).collect(),
                    ));
                }
                let parsed: OpenAiModelListResponse = resp.json()?;
                return Ok(params::classify_models(
                    parsed.data.into_iter().map(|item| item.id).collect(),
                ));
            }

            last_error = format!(
                "{} -> HTTP {}: {}",
                url,
                resp.status,
                Self::read_error_message(&resp)
            );
            if matches!(resp.status, 401 | 403 | 429) {
                return Err(Self::rejected(&resp));
            }
        }

        Err(ProviderError::ProviderRejected(last_error))
    }

    fn as_image_generator(&self) -> Option<&dyn ImageGenerator> {
        Some(self)
    }

    fn as_analyzer(&self) -> Option<&dyn Analyzer> {
        Some(self)
    }
}

#[async_trait]
impl ImageGenerator for OpenAiCompatibleProvider {
    async fn generate(
        &self,
        req: ImageGenerationRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<GenerationOutput> {
        if ctx.is_cancelled() {
            return Err(ProviderError::TaskCancelled);
        }

        let model = response::pick_generation_model(ctx, "gpt-image-1");
        let analysis_model = response::pick_analysis_model(ctx, "gpt-4o-mini");
        let plan = EndpointPlan::for_ctx(ctx, &analysis_model, &model);
        let mut req = req;
        // 超过 10MB 的参考图自动上传图床
        generate::upload_large_reference_images(&mut req, ctx).await?;
        match plan.image_api {
            ImageApi::GeminiGenerateContent => generate_with_gemini(req, ctx, &model).await,
            ImageApi::OpenAiImages => generate_with_openai_images(req, ctx, &model).await,
        }
    }
}

#[async_trait]
impl Analyzer for OpenAiCompatibleProvider {
    async fn analyze(
        &self,
        req: AnalysisRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<AnalysisOutput> {
        if ctx.is_cancelled() {
            return Err(ProviderError::TaskCancelled);
        }

        let model = req
            .model
            .clone()
            .unwrap_or_else(|| response::pick_analysis_model(ctx, "gpt-4o-mini"));
        let image_model = response::pick_generation_model(ctx, "gpt-image-1");
        let plan = EndpointPlan::for_ctx(ctx, &model, &image_model);
        match plan.analysis_api {
            AnalysisApi::OpenAiChat => analyze::analyze_with_openai_chat(req, ctx, &model).await,
            AnalysisApi::OpenAiResponses => {
                analyze::analyze_with_openai_responses(req, ctx, &model).await
            }
            AnalysisApi::AnthropicMessages => {
                analyze::analyze_with_anthropic_messages(req, ctx, &model).await
            }
            AnalysisApi::GeminiGenerateContent => {
                analyze::analyze_with_gemini(req, ctx, &model).await
            }
            AnalysisApi::OpenAiEmbeddings => Err(ProviderError::UnsupportedCapability),
            AnalysisApi::Rerank => Err(ProviderError::UnsupportedCapability),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use artait_provider::HttpClient;
    use async_trait::async_trait;

    struct SequenceHttpClient {
        statuses: Mutex<Vec<u16>>,
        urls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl HttpClient for SequenceHttpClient {
        async fn execute(&self, req: HttpRequest) -> ProviderResult<HttpResponse> {
            self.urls.lock().unwrap().push(req.url);
            let status = self.statuses.lock().unwrap().remove(0);
            let body = if status == 200 {
                r#"{"data":[{"url":"https://example.com/generated.png"}]}"#
            } else {
                r#"{"error":{"message":"not found"}}"#
            };
            Ok(HttpResponse {
                status,
                headers: Vec::new(),
                body: body.as_bytes().to_vec().into(),
            })
        }
    }

    #[test]
    fn routes_banana_models_by_endpoint_style() {
        let mut ctx = ProviderContext::new_for_test("openai-1", "openai-compatible");
        ctx.endpoint = Some("https://api.tokenhub.host/v1".into());
        let plan = EndpointPlan::for_ctx(&ctx, "gemini-2.5-flash", "nano-banana-pro-vt");
        assert_eq!(plan.analysis_api, AnalysisApi::OpenAiChat);
        assert_eq!(plan.image_api, ImageApi::OpenAiImages);

        ctx.endpoint = Some("https://generativelanguage.googleapis.com/v1beta".into());
        let plan = EndpointPlan::for_ctx(&ctx, "gemini-2.5-flash", "nano-banana-pro-vt");
        assert_eq!(plan.analysis_api, AnalysisApi::GeminiGenerateContent);
        assert_eq!(plan.image_api, ImageApi::GeminiGenerateContent);

        ctx.endpoint = Some("https://api.tokenhub.host/v1".into());
        ctx.extra = serde_json::json!({ "api_style": "gemini" });
        let plan = EndpointPlan::for_ctx(&ctx, "gemini-2.5-flash", "nano-banana-pro-vt");
        assert_eq!(plan.analysis_api, AnalysisApi::GeminiGenerateContent);
        assert_eq!(plan.image_api, ImageApi::GeminiGenerateContent);

        ctx.extra = serde_json::json!({ "api_style": "sub2api" });
        let plan = EndpointPlan::for_ctx(&ctx, "gpt-4o-mini", "gpt-image-2");
        assert_eq!(plan.analysis_api, AnalysisApi::OpenAiChat);
        assert_eq!(plan.image_api, ImageApi::OpenAiImages);

        ctx.extra = serde_json::json!({ "api_style": "newapi" });
        let plan = EndpointPlan::for_ctx(&ctx, "gpt-4o-mini", "gpt-image-2");
        assert_eq!(plan.analysis_api, AnalysisApi::OpenAiChat);
        assert_eq!(plan.image_api, ImageApi::OpenAiImages);

        ctx.extra = serde_json::json!({ "api_style": "cpa" });
        let plan = EndpointPlan::for_ctx(&ctx, "gpt-4o-mini", "gpt-image-2");
        assert_eq!(plan.analysis_api, AnalysisApi::OpenAiChat);
        assert_eq!(plan.image_api, ImageApi::OpenAiImages);
    }

    #[test]
    fn detects_image_size_for_gpt_image_2() {
        let req = image_req("16:9", "4K");
        let params = params::pick_image_params("gpt-image-2", &req);
        assert_eq!(params.size, "16:9");
        assert_eq!(params.pixel_size, "3840x2160");
        assert_eq!(params.resolution, Some("4K"));
        assert_eq!(params.response_format, Some("url"));
        assert_eq!(params.quality, None);

        let req = image_req("1:1", "4K");
        let params = params::pick_image_params("gpt-image-2", &req);
        assert_eq!(params.size, "16:9");
        assert_eq!(params.pixel_size, "3840x2160");

        let req = image_req("21:9", "2K");
        let params = params::pick_image_params("gpt-image-2", &req);
        assert_eq!(params.size, "21:9");
        assert_eq!(params.pixel_size, "2688x1152");
        assert_eq!(params.resolution, Some("2K"));
    }

    #[test]
    fn serializes_gpt_image_2_toapis_body() {
        let req = image_req("9:21", "4K");
        let params = params::pick_image_params("gpt-image-2", &req);
        let body = generate::OpenAiImagesRequest {
            model: "gpt-image-2",
            prompt: &req.prompt,
            n: 1,
            size: params.size,
            resolution: params.resolution,
            response_format: params.response_format,
            quality: params.quality,
            image_size: params.image_size,
            aspect_ratio: params.aspect_ratio,
            image_urls: None,
        };
        let json = serde_json::to_value(body).unwrap();
        assert_eq!(json["size"], "9:21");
        assert_eq!(json["resolution"], "4K");
        assert_eq!(json["response_format"], "url");
        assert!(json.get("quality").is_none());
    }

    #[test]
    fn serializes_gpt_image_2_sub2api_body() {
        let req = image_req("16:9", "4K");
        let params = params::pick_openai_image_params(
            "gpt-image-2",
            &req,
            params::ImageCompatStyle::Sub2Api,
        );
        let body = generate::OpenAiImagesRequest {
            model: "gpt-image-2",
            prompt: &req.prompt,
            n: 1,
            size: params.size,
            resolution: params.resolution,
            response_format: params.response_format,
            quality: params.quality,
            image_size: params.image_size,
            aspect_ratio: params.aspect_ratio,
            image_urls: None,
        };
        let json = serde_json::to_value(body).unwrap();
        assert_eq!(json["size"], "3840x2160");
        assert_eq!(json["response_format"], "url");
        assert!(json.get("resolution").is_none());
        assert!(json.get("image_size").is_none());
        assert!(json.get("aspect_ratio").is_none());
        assert!(json.get("quality").is_none());
    }

    #[test]
    fn serializes_gpt_image_2_cpa_body() {
        let req = image_req("9:21", "4K");
        let params =
            params::pick_openai_image_params("gpt-image-2", &req, params::ImageCompatStyle::Cpa);
        assert_eq!(params.size, "9:21");
        assert_eq!(params.pixel_size, "1648x3840");
        assert_eq!(params.resolution, Some("4K"));
        assert_eq!(params.response_format, Some("url"));
        assert_eq!(params.quality, None);
    }

    #[test]
    fn serializes_gpt_image_2_newapi_body() {
        let req = image_req("9:21", "4K");
        let params =
            params::pick_openai_image_params("gpt-image-2", &req, params::ImageCompatStyle::NewApi);
        assert_eq!(params.size, "1648x3840");
        assert_eq!(params.pixel_size, "1648x3840");
        assert_eq!(params.resolution, None);
        assert_eq!(params.response_format, Some("url"));
        assert_eq!(params.image_size, None);
        assert_eq!(params.aspect_ratio, None);
    }

    #[test]
    fn treats_running_image_tasks_as_pending() {
        let task = generate::ToApisImageTaskResponse {
            id: "task-1".into(),
            status: "running".into(),
            progress: Some(30),
            url: None,
            result: None,
            error: None,
        };

        let output = response::completed_toapis_task_output(&task, serde_json::json!({})).unwrap();
        assert!(output.is_none());
    }

    #[test]
    fn maps_nano_banana_openai_sizes_from_proxy_list() {
        let req = image_req("1:1", "4K");
        let params = params::pick_openai_image_params(
            "nano-banana-pro-vt",
            &req,
            params::ImageCompatStyle::Auto,
        );
        assert_eq!(params.size, "2880x2880");
        assert_eq!(params.pixel_size, "2880x2880");
        assert_eq!(params.image_size, Some("4K"));
        assert_eq!(params.aspect_ratio, Some("1:1"));

        let req = image_req("16:9", "4K");
        let params = params::pick_openai_image_params(
            "nano-banana-pro-vt",
            &req,
            params::ImageCompatStyle::Auto,
        );
        assert_eq!(params.size, "3840x2160");
        assert_eq!(params.image_size, Some("4K"));
        assert_eq!(params.aspect_ratio, Some("16:9"));

        let req = image_req("9:21", "2K");
        let params = params::pick_openai_image_params(
            "nano-banana-pro-vt",
            &req,
            params::ImageCompatStyle::Auto,
        );
        assert_eq!(params.size, "1248x2912");
        assert_eq!(params.image_size, Some("2K"));
        assert_eq!(params.aspect_ratio, Some("9:21"));

        let params = params::pick_openai_image_params(
            "nano-banana-pro-vt",
            &req,
            params::ImageCompatStyle::NewApi,
        );
        assert_eq!(params.size, "1248x2912");
        assert_eq!(params.image_size, None);
        assert_eq!(params.aspect_ratio, None);
    }

    #[test]
    fn detects_openai_legacy_size() {
        let req = image_req("9:16", "2K");
        let params = params::pick_image_params("gpt-image-1", &req);
        assert_eq!(params.size, "1024x1536");
        assert_eq!(params.quality, Some("medium"));
    }

    #[test]
    fn maps_dalle_quality_conservatively() {
        let req = image_req("1:1", "4K");
        let params = params::pick_image_params("dall-e-3", &req);
        assert_eq!(params.size, "1024x1024");
        assert_eq!(params.quality, Some("hd"));

        let params = params::pick_image_params("dall-e-2", &req);
        assert_eq!(params.size, "1024x1024");
        assert_eq!(params.quality, None);
    }

    #[test]
    fn detects_gemini_image_params() {
        let req = image_req("16:9", "4K");
        let params = params::pick_image_params("gemini-2.5-flash-image", &req);
        assert_eq!(params.size, "1344x768");
        assert_eq!(params.image_size, None);
        assert_eq!(params.aspect_ratio, Some("16:9"));
    }

    #[test]
    fn builds_gemini_image_size_for_supported_models() {
        let req = image_req("9:16", "4K");
        let params = params::pick_image_params("gemini-3-pro-image", &req);
        let body =
            generate::build_gemini_image_body(&req, "gemini-3-pro-image", &params, false).unwrap();
        let json = serde_json::to_value(body).unwrap();
        assert_eq!(
            json["generationConfig"]["responseFormat"]["image"]["aspectRatio"],
            "9:16"
        );
        assert_eq!(
            json["generationConfig"]["responseFormat"]["image"]["imageSize"],
            "4K"
        );
        assert!(json.get("size").is_none());

        let params = params::pick_image_params("gemini-2.5-flash-image", &req);
        let body =
            generate::build_gemini_image_body(&req, "gemini-2.5-flash-image", &params, false)
                .unwrap();
        let json = serde_json::to_value(body).unwrap();
        assert!(json["generationConfig"]["responseFormat"]["image"]
            .get("imageSize")
            .is_none());
    }

    #[test]
    fn adds_gemini_compat_size_for_proxy_bodies() {
        let req = image_req("16:9", "4K");
        let params = params::pick_image_params("nano-banana-pro", &req);
        let body =
            generate::build_gemini_image_body(&req, "nano-banana-pro", &params, true).unwrap();
        let json = serde_json::to_value(body).unwrap();
        assert_eq!(json["size"], "5504x3072");
        assert_eq!(
            json["generationConfig"]["responseFormat"]["image"]["aspectRatio"],
            "16:9"
        );
        assert_eq!(
            json["generationConfig"]["responseFormat"]["image"]["imageSize"],
            "4K"
        );
    }

    #[test]
    fn maps_gemini31_flash_image_pixels() {
        let req = image_req("1:8", "512");
        let params = params::pick_image_params("gemini-3.1-flash-image", &req);
        assert_eq!(params.size, "192x1536");
        assert_eq!(params.image_size, Some("512"));
        assert_eq!(params.aspect_ratio, Some("1:8"));

        let req = image_req("21:9", "4K");
        let params = params::pick_image_params("nano-banana-2", &req);
        assert_eq!(params.size, "6336x2688");
        assert_eq!(params.image_size, Some("4K"));
        assert_eq!(params.aspect_ratio, Some("21:9"));
    }

    #[test]
    fn maps_gemini3_pro_image_pixels() {
        let req = image_req("16:9", "4K");
        let params = params::pick_image_params("nano-banana-pro", &req);
        assert_eq!(params.size, "5504x3072");
        assert_eq!(params.image_size, Some("4K"));
        assert_eq!(params.aspect_ratio, Some("16:9"));

        let req = image_req("1:8", "4K");
        let params = params::pick_image_params("gemini-3-pro-image", &req);
        assert_eq!(params.size, "4096x4096");
        assert_eq!(params.aspect_ratio, Some("1:1"));
    }

    #[test]
    fn maps_gemini25_flash_image_pixels_without_image_size() {
        let req = image_req("9:16", "4K");
        let params = params::pick_image_params("gemini-2.5-flash-image", &req);
        assert_eq!(params.size, "768x1344");
        assert_eq!(params.image_size, None);
        assert_eq!(params.aspect_ratio, Some("9:16"));
    }

    #[test]
    fn keeps_openai_prefixed_base_paths() {
        let base = EndpointBase::from_raw("https://example.com/openai/v1");
        assert_eq!(
            base.openai_chat_url(),
            "https://example.com/openai/v1/chat/completions"
        );
        let base = EndpointBase::from_raw("https://example.com/api/v1");
        assert_eq!(
            base.openai_images_url(),
            "https://example.com/api/v1/images/generations"
        );
    }

    #[test]
    fn appends_v1_for_site_root_base_urls() {
        let base = EndpointBase::from_raw("https://api.openai.com");
        assert_eq!(
            base.openai_chat_url(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            base.openai_embeddings_url(),
            "https://api.openai.com/v1/embeddings"
        );
    }

    #[test]
    fn builds_newapi_rerank_url() {
        let base = EndpointBase::from_raw("https://newapi.example.com");
        assert_eq!(base.rerank_url(), "https://newapi.example.com/v1/rerank");
    }

    #[test]
    fn normalizes_concrete_endpoint_to_sibling_paths() {
        let base = EndpointBase::from_raw("https://example.com/v1/chat/completions");
        assert_eq!(
            base.openai_responses_url(),
            "https://example.com/v1/responses"
        );
        assert_eq!(
            base.openai_images_url(),
            "https://example.com/v1/images/generations"
        );
        assert_eq!(
            base.openai_image_edits_url(),
            "https://example.com/v1/images/edits"
        );
        let base = EndpointBase::from_raw("https://example.com/v1/images/edits");
        assert_eq!(
            base.openai_images_url(),
            "https://example.com/v1/images/generations"
        );
    }

    #[test]
    fn builds_image_endpoint_candidates_for_common_proxy_prefixes() {
        let base = EndpointBase::from_raw("https://api.tokenhub.host/v1");
        assert_eq!(
            base.openai_images_url_candidates(),
            vec![
                "https://api.tokenhub.host/v1/images/generations",
                "https://api.tokenhub.host/openai/v1/images/generations",
                "https://api.tokenhub.host/api/v1/images/generations",
                "https://api.tokenhub.host/images/generations",
            ]
        );
        assert_eq!(
            base.openai_image_edits_url_candidates(),
            vec![
                "https://api.tokenhub.host/v1/images/edits",
                "https://api.tokenhub.host/openai/v1/images/edits",
                "https://api.tokenhub.host/api/v1/images/edits",
                "https://api.tokenhub.host/images/edits",
            ]
        );
    }

    #[tokio::test]
    async fn falls_back_to_next_image_endpoint_after_404() {
        let http = Arc::new(SequenceHttpClient {
            statuses: Mutex::new(vec![404, 200]),
            urls: Mutex::new(Vec::new()),
        });
        let mut ctx = ProviderContext::with_http("openai-1", "openai-compatible", http.clone());
        ctx.endpoint = Some("https://api.tokenhub.host/v1".into());
        ctx.secret = Some("test-key".into());
        ctx.extra = serde_json::json!({ "api_style": "sub2api" });

        let output =
            generate::generate_with_openai_images(image_req("1:1", "1K"), &ctx, "gpt-image-2")
                .await
                .unwrap();

        match output {
            GenerationOutput::Url { url, .. } => {
                assert_eq!(url, "https://example.com/generated.png");
            }
            other => panic!("expected url output, got {other:?}"),
        }
        assert_eq!(
            *http.urls.lock().unwrap(),
            vec![
                "https://api.tokenhub.host/v1/images/generations".to_string(),
                "https://api.tokenhub.host/openai/v1/images/generations".to_string(),
            ]
        );
    }

    #[test]
    fn builds_image_task_url_from_successful_generation_endpoint() {
        assert_eq!(
            response::openai_image_task_url(
                "https://api.tokenhub.host/api/v1/images/generations",
                "task-1",
            ),
            "https://api.tokenhub.host/api/v1/images/generations/task-1"
        );
    }

    #[test]
    fn builds_gemini_generate_content_url() {
        let base = EndpointBase::from_raw("https://generativelanguage.googleapis.com/v1beta");
        assert_eq!(
            base.gemini_generate_content_url("gemini-2.5-flash"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent"
        );
        assert_eq!(
            base.gemini_models_url(),
            "https://generativelanguage.googleapis.com/v1beta/models"
        );
        let root = EndpointBase::from_raw("https://generativelanguage.googleapis.com");
        assert_eq!(
            root.gemini_generate_content_url("gemini-2.5-flash"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent"
        );
    }

    #[test]
    fn builds_image_edit_multipart_with_reference_images() {
        let root =
            std::env::temp_dir().join(format!("artait-test-image-edit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.png");
        let second = root.join("second.png");
        std::fs::write(&first, b"first-image").unwrap();
        std::fs::write(&second, b"second-image").unwrap();

        let mut req = image_req("1:1", "2K");
        req.reference_images = vec![
            test_ref_image(first, "first.png"),
            test_ref_image(second, "second.png"),
        ];
        let params = params::pick_image_params("gpt-image-1", &req);
        let body =
            generate::build_openai_image_edit_multipart("gpt-image-1", &req, &params).unwrap();
        let text = String::from_utf8_lossy(&body.bytes);

        assert!(text.contains("name=\"model\""));
        assert!(text.contains("gpt-image-1"));
        assert!(text.contains("name=\"prompt\""));
        assert!(text.contains("name=\"quality\""));
        assert!(text.contains("medium"));
        assert!(!text.contains("name=\"image\"; filename=\"first.png\""));
        assert!(text.contains("name=\"image[]\"; filename=\"first.png\""));
        assert!(text.contains("name=\"image[]\"; filename=\"second.png\""));
        assert!(text.contains("first-image"));
        assert!(text.contains("second-image"));
    }

    #[test]
    fn builds_sub2api_gpt_image_2_edit_multipart() {
        let root =
            std::env::temp_dir().join(format!("artait-test-sub2api-edit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.png");
        std::fs::write(&source, b"source-image").unwrap();

        let mut req = image_req("3:2", "1K");
        req.reference_images = vec![test_ref_image(source, "source.png")];
        let params = params::pick_openai_image_params(
            "gpt-image-2",
            &req,
            params::ImageCompatStyle::Sub2Api,
        );
        let body =
            generate::build_openai_image_edit_multipart("gpt-image-2", &req, &params).unwrap();
        let text = String::from_utf8_lossy(&body.bytes);

        assert!(text.contains("name=\"image\"; filename=\"source.png\""));
        assert!(text.contains("name=\"size\""));
        assert!(text.contains("1536x1024"));
        assert!(text.contains("name=\"response_format\""));
        assert!(text.contains("url"));
        assert!(text.contains("source-image"));
        assert!(!text.contains("name=\"resolution\""));
        assert!(!text.contains("name=\"image_size\""));
        assert!(!text.contains("name=\"aspect_ratio\""));
    }

    fn image_req(aspect: &str, quality: &str) -> ImageGenerationRequest {
        ImageGenerationRequest {
            prompt: "test".into(),
            negative_prompt: None,
            reference_images: vec![],
            aspect_ratio: Some(aspect.into()),
            resolution: None,
            size: None,
            quality: Some(quality.into()),
            count: 1,
            mode: artait_model::CreationMode::Scene,
            action_name: None,
            metadata: serde_json::Value::Null,
        }
    }

    fn test_ref_image(path: PathBuf, name: &str) -> ReferenceImage {
        ReferenceImage {
            local_path: path,
            display_name: name.into(),
            mime_type: "image/png".into(),
            width: None,
            height: None,
            uploaded_url: None,
            upload_cache_key: None,
            source: artait_model::ReferenceImageSource::UserPicked,
        }
    }
}
