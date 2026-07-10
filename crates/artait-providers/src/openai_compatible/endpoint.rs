use artait_model::ProviderError;
use artait_provider::{http::HttpRequest, ProviderContext, ProviderResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisApi {
    OpenAiChat,
    OpenAiResponses,
    AnthropicMessages,
    GeminiGenerateContent,
    OpenAiEmbeddings,
    Rerank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageApi {
    OpenAiImages,
    GeminiGenerateContent,
}

#[derive(Debug, Clone, Copy)]
pub struct EndpointPlan {
    pub analysis_api: AnalysisApi,
    pub image_api: ImageApi,
}

#[derive(Debug, Clone)]
pub struct EndpointBase {
    raw: String,
}

impl EndpointBase {
    pub fn from_ctx(ctx: &ProviderContext) -> ProviderResult<Self> {
        let endpoint = ctx
            .endpoint
            .as_deref()
            .ok_or_else(|| ProviderError::InvalidConfig("缺少 endpoint".into()))?;
        let raw = endpoint.trim().trim_end_matches('/').to_string();
        if raw.is_empty() {
            return Err(ProviderError::InvalidConfig("endpoint 为空".into()));
        }
        Ok(Self { raw })
    }

    #[cfg(test)]
    pub fn from_raw(raw: &str) -> Self {
        Self {
            raw: raw.trim_end_matches('/').to_string(),
        }
    }

    pub fn with_path(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        if self.raw.ends_with(path) {
            return self.raw.clone();
        }
        if let Some(prefix) = self.raw.strip_suffix("/chat/completions") {
            return format!("{prefix}/{path}");
        }
        if let Some(prefix) = self.raw.strip_suffix("/responses") {
            return format!("{prefix}/{path}");
        }
        if let Some(prefix) = self.raw.strip_suffix("/messages") {
            return format!("{prefix}/{path}");
        }
        if let Some(prefix) = self.raw.strip_suffix("/embeddings") {
            return format!("{prefix}/{path}");
        }
        if let Some(prefix) = self.raw.strip_suffix("/images/generations") {
            return format!("{prefix}/{path}");
        }
        if let Some(prefix) = self.raw.strip_suffix("/images/edits") {
            return format!("{prefix}/{path}");
        }
        format!("{}/{}", self.openai_api_root(), path)
    }

    pub fn openai_api_root(&self) -> String {
        let raw = self.raw.trim_end_matches('/');
        if raw.ends_with("/v1") || raw.ends_with("/openai/v1") || raw.ends_with("/api/v1") {
            return raw.to_string();
        }
        if raw.contains("/v1/") || raw.contains("/openai/v1/") || raw.contains("/api/v1/") {
            return raw.to_string();
        }
        format!("{raw}/v1")
    }

    pub fn model_endpoint_candidates(&self, _model: &str) -> Vec<String> {
        if self.is_gemini_base() {
            return vec![self.gemini_models_url()];
        }
        if self.raw.ends_with("/models") {
            return vec![self.raw.clone()];
        }
        vec![
            self.with_path("models"),
            self.with_path("v1/models"),
            self.with_path("openai/v1/models"),
            self.with_path("api/v1/models"),
        ]
        .into_iter()
        .fold(Vec::new(), push_unique)
    }

    pub fn openai_images_url_candidates(&self) -> Vec<String> {
        self.openai_path_candidates("images/generations")
    }

    pub fn openai_image_edits_url_candidates(&self) -> Vec<String> {
        self.openai_path_candidates("images/edits")
    }

    pub fn openai_chat_url(&self) -> String {
        self.with_path("chat/completions")
    }

    pub fn openai_responses_url(&self) -> String {
        self.with_path("responses")
    }

    pub fn anthropic_messages_url(&self) -> String {
        self.with_path("messages")
    }

    pub fn openai_images_url(&self) -> String {
        self.with_path("images/generations")
    }

    pub fn openai_image_edits_url(&self) -> String {
        self.with_path("images/edits")
    }

    #[allow(dead_code)]
    pub fn openai_embeddings_url(&self) -> String {
        self.with_path("embeddings")
    }

    #[allow(dead_code)]
    pub fn rerank_url(&self) -> String {
        self.with_path("rerank")
    }

    pub fn gemini_generate_content_url(&self, model: &str) -> String {
        self.gemini_model_url(model, ":generateContent")
    }

    pub fn gemini_models_url(&self) -> String {
        format!("{}/models", self.gemini_api_root())
    }

    pub fn is_gemini_base(&self) -> bool {
        self.raw.contains("generativelanguage.googleapis.com")
            || self.raw.contains("/v1beta/models/")
            || self.raw.contains("/v1/models/")
            || self.raw.ends_with("/v1beta")
            || self.raw.contains(":generateContent")
    }

    fn gemini_model_url(&self, model: &str, suffix: &str) -> String {
        let base = self.raw.trim_end_matches(":generateContent");
        if base.contains("/models/") {
            return format!("{base}{suffix}");
        }
        let root = self.gemini_api_root();
        format!("{root}/models/{model}{suffix}")
    }

    fn gemini_api_root(&self) -> String {
        let base = self.raw.trim_end_matches(":generateContent");
        if let Some((root, _)) = base.split_once("/models/") {
            return root.to_string();
        }
        if base.ends_with("/v1") || base.ends_with("/v1beta") {
            return base.to_string();
        }
        format!("{base}/v1beta")
    }

    fn openai_path_candidates(&self, path: &str) -> Vec<String> {
        let path = path.trim_start_matches('/');
        let root = self.site_root();
        let primary = match path {
            "images/generations" => self.openai_images_url(),
            "images/edits" => self.openai_image_edits_url(),
            _ => self.with_path(path),
        };
        vec![
            primary,
            format!("{root}/v1/{path}"),
            format!("{root}/openai/v1/{path}"),
            format!("{root}/api/v1/{path}"),
            format!("{root}/{path}"),
        ]
        .into_iter()
        .fold(Vec::new(), push_unique)
    }

    fn site_root(&self) -> String {
        let raw = self.raw.trim_end_matches('/');
        let Some(scheme_idx) = raw.find("://") else {
            return raw.to_string();
        };
        let host_start = scheme_idx + 3;
        let Some(path_offset) = raw[host_start..].find('/') else {
            return raw.to_string();
        };
        raw[..host_start + path_offset].to_string()
    }
}

impl EndpointPlan {
    pub fn for_ctx(ctx: &ProviderContext, analysis_model: &str, image_model: &str) -> Self {
        let forced = ctx
            .extra
            .get("api_style")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let endpoint_is_gemini = EndpointBase::from_ctx(ctx)
            .map(|base| base.is_gemini_base())
            .unwrap_or(false);
        Self {
            analysis_api: match forced.as_str() {
                "responses" | "openai_responses" => AnalysisApi::OpenAiResponses,
                "messages" | "anthropic" => AnalysisApi::AnthropicMessages,
                "gemini" | "gemini_generate_content" => AnalysisApi::GeminiGenerateContent,
                "embedding" | "embeddings" | "openai_embedding" => AnalysisApi::OpenAiEmbeddings,
                "rerank" => AnalysisApi::Rerank,
                _ if endpoint_is_gemini && is_gemini_model(analysis_model) => {
                    AnalysisApi::GeminiGenerateContent
                }
                _ if is_anthropic_model(analysis_model) => AnalysisApi::AnthropicMessages,
                _ if is_embedding_model(analysis_model) => AnalysisApi::OpenAiEmbeddings,
                _ if is_rerank_model(analysis_model) => AnalysisApi::Rerank,
                _ if prefers_responses(analysis_model) => AnalysisApi::OpenAiResponses,
                _ => AnalysisApi::OpenAiChat,
            },
            image_api: match forced.as_str() {
                "gemini" | "gemini_generate_content" => ImageApi::GeminiGenerateContent,
                "images" | "openai_images" | "openai_image" | "cpa" | "cpa_api" | "toapis"
                | "to_apis" | "newapi" | "new_api" | "sub2api" | "sub2_api" => {
                    ImageApi::OpenAiImages
                }
                _ if endpoint_is_gemini && is_gemini_model(image_model) => {
                    ImageApi::GeminiGenerateContent
                }
                _ => ImageApi::OpenAiImages,
            },
        }
    }
}

pub fn apply_auth(
    req: HttpRequest,
    ctx: &ProviderContext,
    api: AnalysisApi,
) -> ProviderResult<HttpRequest> {
    let secret = require_secret(ctx)?;
    Ok(match api {
        AnalysisApi::AnthropicMessages => req
            .header("x-api-key", secret)
            .header("anthropic-version", "2023-06-01"),
        _ => req.bearer(secret),
    })
}

pub fn apply_gemini_auth(url: String, ctx: &ProviderContext) -> ProviderResult<HttpRequest> {
    let secret = require_secret(ctx)?;
    Ok(HttpRequest::post(url)
        .bearer(secret)
        .header("x-goog-api-key", secret)
        .header("X-Goog-Api-Key", secret))
}

pub fn require_secret(ctx: &ProviderContext) -> ProviderResult<&str> {
    ctx.secret
        .as_deref()
        .ok_or_else(|| ProviderError::MissingSecret("api_key".into()))
}

pub fn is_gemini_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("gemini") || m.contains("imagen") || m.contains("nano-banana")
}

pub fn is_anthropic_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("claude")
}

pub fn is_embedding_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("embedding") || m.starts_with("text-embedding") || m.starts_with("bge-")
}

pub fn is_rerank_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("rerank") || m.contains("re-rank")
}

pub fn prefers_responses(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") || m.starts_with("gpt-5")
}

fn push_unique(mut acc: Vec<String>, url: String) -> Vec<String> {
    if !acc.iter().any(|u| u == &url) {
        acc.push(url);
    }
    acc
}
