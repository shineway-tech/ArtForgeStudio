use std::time::Duration;

use super::endpoint;
use super::media::{data_url_to_parts, image_data_url, image_inline_data};
use super::response;
use artait_model::ProviderError;
use artait_provider::{
    http::HttpRequest,
    request::{AnalysisOutput, AnalysisRequest, AnalysisResponseFormat, TokenUsage},
    ProviderContext, ProviderResult,
};
use serde::{Deserialize, Serialize};

// ── Chat types ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ChatResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ChatResponseFormat {
    JsonObject,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct ChatMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ChatUsage {
    #[serde(default)]
    pub(crate) prompt_tokens: u32,
    #[serde(default)]
    pub(crate) completion_tokens: u32,
    #[serde(default)]
    pub(crate) total_tokens: u32,
}

// ── Response types (OpenAI Responses API) ───────────────────────────────────

#[derive(Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ResponseInputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<ResponseTextConfig>,
}

#[derive(Serialize)]
pub(crate) struct ResponseInputMessage {
    role: String,
    content: Vec<ResponseInputPart>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseInputPart {
    InputText { text: String },
    InputImage { image_url: String },
}

#[derive(Serialize)]
struct ResponseTextConfig {
    format: ResponseTextFormat,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseTextFormat {
    JsonObject,
}

#[derive(Deserialize)]
struct ResponsesResponse {
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    output: Vec<ResponseOutputItem>,
    #[serde(default)]
    usage: Option<ResponsesUsage>,
}

#[derive(Deserialize)]
struct ResponseOutputItem {
    #[serde(default)]
    content: Vec<ResponseOutputContent>,
}

#[derive(Deserialize)]
struct ResponseOutputContent {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

// ── Anthropic types ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicPart>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicPart {
    Text { text: String },
    Image { source: AnthropicImageSource },
}

#[derive(Serialize)]
pub(crate) struct AnthropicImageSource {
    #[serde(rename = "type")]
    kind: String,
    media_type: String,
    data: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicResponsePart>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicResponsePart {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

// ── Gemini types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiRequest {
    pub(crate) contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) generation_config: Option<GeminiGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) size: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    pub(crate) parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) inline_data: Option<GeminiInlineData>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiInlineData {
    pub(crate) mime_type: String,
    pub(crate) data: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) response_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) response_modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) response_format: Option<GeminiResponseFormat>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiResponseFormat {
    pub(crate) image: GeminiImageConfig,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiImageConfig {
    pub(crate) aspect_ratio: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image_size: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiResponse {
    #[serde(default)]
    pub(crate) candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    pub(crate) usage_metadata: Option<GeminiUsage>,
}

#[derive(Deserialize)]
pub(crate) struct GeminiCandidate {
    pub(crate) content: Option<GeminiResponseContent>,
}

#[derive(Deserialize)]
pub(crate) struct GeminiResponseContent {
    #[serde(default)]
    pub(crate) parts: Vec<GeminiResponsePart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiResponsePart {
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) inline_data: Option<GeminiInlineData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiUsage {
    #[serde(default)]
    pub(crate) prompt_token_count: u32,
    #[serde(default)]
    pub(crate) candidates_token_count: u32,
    #[serde(default)]
    pub(crate) total_token_count: u32,
}

impl GeminiResponse {
    fn first_text(self) -> Option<String> {
        let text = self
            .candidates
            .into_iter()
            .filter_map(|c| c.content)
            .flat_map(|c| c.parts)
            .filter_map(|p| p.text)
            .collect::<Vec<_>>()
            .join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    pub(crate) fn first_inline_data(self) -> Option<(String, String)> {
        self.candidates
            .into_iter()
            .filter_map(|c| c.content)
            .flat_map(|c| c.parts)
            .find_map(|p| p.inline_data.map(|d| (d.mime_type, d.data)))
    }
}

// ── Analysis functions ──────────────────────────────────────────────────────

pub(crate) async fn analyze_with_openai_chat(
    req: AnalysisRequest,
    ctx: &ProviderContext,
    model: &str,
) -> ProviderResult<AnalysisOutput> {
    let base = endpoint::EndpointBase::from_ctx(ctx)?;
    let body = ChatRequest {
        model: model.to_string(),
        messages: build_chat_messages(&req)?,
        response_format: matches!(req.response_format, AnalysisResponseFormat::Json)
            .then_some(ChatResponseFormat::JsonObject),
        temperature: Some(0.7),
    };
    let http_req = endpoint::apply_auth(
        HttpRequest::post(base.openai_chat_url()),
        ctx,
        endpoint::AnalysisApi::OpenAiChat,
    )?
    .timeout(Duration::from_secs(120))
    .json_body(&body)?;
    let resp = ctx.http.execute(http_req).await?;
    if !resp.is_success() {
        return Err(super::OpenAiCompatibleProvider::rejected(&resp));
    }
    let parsed: ChatResponse = resp.json()?;
    let text = parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .ok_or_else(|| ProviderError::InvalidResponse("缺少 choices[0].message.content".into()))?;
    Ok(response::analysis_output(
        text,
        req.response_format,
        parsed.usage.map(response::chat_usage),
    ))
}

pub(crate) async fn analyze_with_openai_responses(
    req: AnalysisRequest,
    ctx: &ProviderContext,
    model: &str,
) -> ProviderResult<AnalysisOutput> {
    let base = endpoint::EndpointBase::from_ctx(ctx)?;
    let body = ResponsesRequest {
        model: model.to_string(),
        input: build_response_input(&req)?,
        text: matches!(req.response_format, AnalysisResponseFormat::Json).then_some(
            ResponseTextConfig {
                format: ResponseTextFormat::JsonObject,
            },
        ),
    };
    let http_req = endpoint::apply_auth(
        HttpRequest::post(base.openai_responses_url()),
        ctx,
        endpoint::AnalysisApi::OpenAiResponses,
    )?
    .timeout(Duration::from_secs(120))
    .json_body(&body)?;
    let resp = ctx.http.execute(http_req).await?;
    if !resp.is_success() {
        return Err(super::OpenAiCompatibleProvider::rejected(&resp));
    }
    let parsed: ResponsesResponse = resp.json()?;
    let text = parsed
        .output_text
        .or_else(|| {
            parsed
                .output
                .into_iter()
                .flat_map(|o| o.content)
                .find_map(|c| c.text)
        })
        .ok_or_else(|| ProviderError::InvalidResponse("Responses 响应缺少 output_text".into()))?;
    Ok(response::analysis_output(
        text,
        req.response_format,
        parsed.usage.map(|u| TokenUsage {
            prompt: u.input_tokens,
            completion: u.output_tokens,
            total: u.total_tokens,
        }),
    ))
}

pub(crate) async fn analyze_with_anthropic_messages(
    req: AnalysisRequest,
    ctx: &ProviderContext,
    model: &str,
) -> ProviderResult<AnalysisOutput> {
    let base = endpoint::EndpointBase::from_ctx(ctx)?;
    let body = AnthropicRequest {
        model: model.to_string(),
        max_tokens: 4096,
        system: req.system_prompt.clone().filter(|s| !s.trim().is_empty()),
        messages: vec![AnthropicMessage {
            role: "user".into(),
            content: build_anthropic_parts(&req)?,
        }],
    };
    let http_req = endpoint::apply_auth(
        HttpRequest::post(base.anthropic_messages_url()),
        ctx,
        endpoint::AnalysisApi::AnthropicMessages,
    )?
    .timeout(Duration::from_secs(120))
    .json_body(&body)?;
    let resp = ctx.http.execute(http_req).await?;
    if !resp.is_success() {
        return Err(super::OpenAiCompatibleProvider::rejected(&resp));
    }
    let parsed: AnthropicResponse = resp.json()?;
    let text = parsed
        .content
        .into_iter()
        .filter_map(|p| p.text)
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        return Err(ProviderError::InvalidResponse(
            "Messages 响应缺少 content.text".into(),
        ));
    }
    Ok(response::analysis_output(
        text,
        req.response_format,
        parsed.usage.map(|u| TokenUsage {
            prompt: u.input_tokens,
            completion: u.output_tokens,
            total: u.input_tokens + u.output_tokens,
        }),
    ))
}

pub(crate) async fn analyze_with_gemini(
    req: AnalysisRequest,
    ctx: &ProviderContext,
    model: &str,
) -> ProviderResult<AnalysisOutput> {
    let base = endpoint::EndpointBase::from_ctx(ctx)?;
    let url = base.gemini_generate_content_url(model);
    let body = GeminiRequest {
        contents: vec![GeminiContent {
            role: Some("user".into()),
            parts: build_gemini_parts(&req)?,
        }],
        system_instruction: req
            .system_prompt
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| GeminiContent {
                role: None,
                parts: vec![GeminiPart {
                    text: Some(s.clone()),
                    inline_data: None,
                }],
            }),
        generation_config: matches!(req.response_format, AnalysisResponseFormat::Json).then_some(
            GeminiGenerationConfig {
                response_mime_type: Some("application/json".into()),
                response_modalities: None,
                response_format: None,
            },
        ),
        size: None,
    };
    let http_req = endpoint::apply_gemini_auth(url, ctx)?
        .timeout(Duration::from_secs(120))
        .json_body(&body)?;
    let resp = ctx.http.execute(http_req).await?;
    if !resp.is_success() {
        return Err(super::OpenAiCompatibleProvider::rejected(&resp));
    }
    let parsed: GeminiResponse = resp.json()?;
    let usage = parsed.usage_metadata.as_ref().map(|u| TokenUsage {
        prompt: u.prompt_token_count,
        completion: u.candidates_token_count,
        total: u.total_token_count,
    });
    let text = parsed
        .first_text()
        .ok_or_else(|| ProviderError::InvalidResponse("Gemini 响应缺少文本".into()))?;
    Ok(response::analysis_output(text, req.response_format, usage))
}

// ── Builder helpers ─────────────────────────────────────────────────────────

pub(crate) fn build_chat_messages(req: &AnalysisRequest) -> ProviderResult<Vec<ChatMessage>> {
    let mut messages = Vec::with_capacity(2);
    if let Some(sys) = &req.system_prompt {
        if !sys.trim().is_empty() {
            messages.push(ChatMessage {
                role: "system".into(),
                content: Some(serde_json::Value::String(sys.clone())),
            });
        }
    }
    messages.push(ChatMessage {
        role: "user".into(),
        content: Some(openai_user_content(req)?),
    });
    Ok(messages)
}

pub(crate) fn openai_user_content(req: &AnalysisRequest) -> ProviderResult<serde_json::Value> {
    if req.images.is_empty() {
        return Ok(serde_json::Value::String(req.user_prompt.clone()));
    }
    let mut parts = vec![serde_json::json!({
        "type": "text",
        "text": req.user_prompt,
    })];
    for img in &req.images {
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": image_data_url(img)? },
        }));
    }
    Ok(serde_json::Value::Array(parts))
}

pub(crate) fn build_response_input(
    req: &AnalysisRequest,
) -> ProviderResult<Vec<ResponseInputMessage>> {
    let mut content = vec![ResponseInputPart::InputText {
        text: req.user_prompt.clone(),
    }];
    for img in &req.images {
        content.push(ResponseInputPart::InputImage {
            image_url: image_data_url(img)?,
        });
    }
    let mut input = Vec::new();
    if let Some(sys) = &req.system_prompt {
        if !sys.trim().is_empty() {
            input.push(ResponseInputMessage {
                role: "system".into(),
                content: vec![ResponseInputPart::InputText { text: sys.clone() }],
            });
        }
    }
    input.push(ResponseInputMessage {
        role: "user".into(),
        content,
    });
    Ok(input)
}

pub(crate) fn build_anthropic_parts(req: &AnalysisRequest) -> ProviderResult<Vec<AnthropicPart>> {
    let mut parts = vec![AnthropicPart::Text {
        text: req.user_prompt.clone(),
    }];
    for img in &req.images {
        let data_url = image_data_url(img)?;
        let (media_type, data) = data_url_to_parts(&data_url).ok_or_else(|| {
            ProviderError::InvalidConfig("Anthropic Messages 需要 base64 图片".into())
        })?;
        parts.push(AnthropicPart::Image {
            source: AnthropicImageSource {
                kind: "base64".into(),
                media_type,
                data,
            },
        });
    }
    Ok(parts)
}

pub(crate) fn build_gemini_parts(req: &AnalysisRequest) -> ProviderResult<Vec<GeminiPart>> {
    let mut parts = vec![GeminiPart {
        text: Some(req.user_prompt.clone()),
        inline_data: None,
    }];
    for img in &req.images {
        let (mime_type, data) = image_inline_data(img)?;
        parts.push(GeminiPart {
            text: None,
            inline_data: Some(GeminiInlineData { mime_type, data }),
        });
    }
    Ok(parts)
}
