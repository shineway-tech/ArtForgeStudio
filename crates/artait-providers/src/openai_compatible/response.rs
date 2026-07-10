use std::time::Duration;

use super::analyze::ChatUsage;
use super::endpoint;
use super::endpoint::AnalysisApi;
use super::generate::{OpenAiImagesResponse, ToApisImageTaskResponse};
use super::OpenAiCompatibleProvider;
use artait_model::ProviderError;
use artait_provider::{
    http::{HttpRequest, HttpResponse},
    request::{AnalysisOutput, AnalysisResponseFormat, GenerationOutput, TokenUsage},
    ProviderContext, ProviderResult,
};

pub(crate) fn output_from_image_item(
    url: Option<String>,
    b64_json: Option<String>,
    metadata: serde_json::Value,
) -> ProviderResult<GenerationOutput> {
    if let Some(url) = url {
        Ok(GenerationOutput::Url { url, metadata })
    } else if let Some(data) = b64_json {
        Ok(GenerationOutput::Base64 {
            data,
            mime: "image/png".into(),
            metadata,
        })
    } else {
        Err(ProviderError::InvalidResponse(
            "响应既没有 url 也没有 b64_json".into(),
        ))
    }
}

pub(crate) fn add_bearer_download_header(
    mut metadata: serde_json::Value,
    ctx: &ProviderContext,
) -> ProviderResult<serde_json::Value> {
    let secret = endpoint::require_secret(ctx)?;
    if let serde_json::Value::Object(obj) = &mut metadata {
        obj.insert(
            "download_headers".into(),
            serde_json::json!({
                "Authorization": format!("Bearer {secret}")
            }),
        );
    }
    Ok(metadata)
}

pub(crate) async fn output_from_openai_images_response(
    resp: HttpResponse,
    ctx: &ProviderContext,
    generation_url: &str,
    metadata: serde_json::Value,
) -> ProviderResult<GenerationOutput> {
    let metadata = add_bearer_download_header(metadata, ctx)?;
    if let Ok(parsed) = resp.json::<OpenAiImagesResponse>() {
        let item = parsed
            .data
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::InvalidResponse("响应没有 data[0]".into()))?;
        return output_from_image_item(item.url, item.b64_json, metadata);
    }

    let task: ToApisImageTaskResponse = resp.json()?;
    output_from_toapis_task(task, ctx, generation_url, metadata).await
}

pub(crate) async fn output_from_toapis_task(
    task: ToApisImageTaskResponse,
    ctx: &ProviderContext,
    generation_url: &str,
    metadata: serde_json::Value,
) -> ProviderResult<GenerationOutput> {
    if let Some(output) = completed_toapis_task_output(&task, metadata.clone())? {
        return Ok(output);
    }
    let task_id = task.id;
    for attempt in 0..40 {
        if ctx.is_cancelled() {
            return Err(ProviderError::TaskCancelled);
        }
        let wait_secs = if attempt == 0 { 2 } else { 3 };
        tokio::time::sleep(Duration::from_secs(wait_secs)).await;
        if ctx.is_cancelled() {
            return Err(ProviderError::TaskCancelled);
        }

        let url = openai_image_task_url(generation_url, &task_id);
        let http_req = endpoint::apply_auth(HttpRequest::get(url), ctx, AnalysisApi::OpenAiChat)?
            .timeout(Duration::from_secs(30));
        let resp = ctx.http.execute(http_req).await?;
        if !resp.is_success() {
            return Err(OpenAiCompatibleProvider::rejected(&resp));
        }
        let task: ToApisImageTaskResponse = resp.json()?;
        if let Some(output) = completed_toapis_task_output(&task, metadata.clone())? {
            return Ok(output);
        }
    }
    Err(ProviderError::ProviderTimeout)
}

pub(crate) fn openai_image_task_url(generation_url: &str, task_id: &str) -> String {
    format!(
        "{}/{}",
        generation_url.trim_end_matches('/'),
        task_id.trim_start_matches('/')
    )
}

pub(crate) fn completed_toapis_task_output(
    task: &ToApisImageTaskResponse,
    metadata: serde_json::Value,
) -> ProviderResult<Option<GenerationOutput>> {
    match task.status.as_str() {
        "completed" => {
            if let Some(url) = task.url.clone().filter(|url| !url.trim().is_empty()) {
                let mut metadata = metadata;
                if let serde_json::Value::Object(obj) = &mut metadata {
                    obj.insert("provider_task_id".into(), task.id.clone().into());
                    if let Some(progress) = task.progress {
                        obj.insert("progress".into(), progress.into());
                    }
                }
                return Ok(Some(GenerationOutput::Url { url, metadata }));
            }
            let item = task
                .result
                .as_ref()
                .and_then(|r| r.data.first())
                .ok_or_else(|| {
                    ProviderError::InvalidResponse("ToAPIs 完成但缺少 result.data[0]".into())
                })?;
            let mut metadata = metadata;
            if let serde_json::Value::Object(obj) = &mut metadata {
                obj.insert("provider_task_id".into(), task.id.clone().into());
                if let Some(progress) = task.progress {
                    obj.insert("progress".into(), progress.into());
                }
            }
            output_from_image_item(item.url.clone(), item.b64_json.clone(), metadata).map(Some)
        }
        "failed" => {
            let message = task
                .error
                .as_ref()
                .and_then(|e| e.message.as_deref())
                .unwrap_or("ToAPIs 图片任务失败");
            let code = task
                .error
                .as_ref()
                .and_then(|e| e.code.as_ref())
                .map(|v| v.to_string())
                .unwrap_or_default();
            Err(ProviderError::ProviderRejected(format!(
                "ToAPIs task {} failed: {} {}",
                task.id, code, message
            )))
        }
        "queued" | "pending" | "submitted" | "running" | "in_progress" | "processing"
        | "generating" => Ok(None),
        other => Err(ProviderError::InvalidResponse(format!(
            "未知 ToAPIs 任务状态: {other}"
        ))),
    }
}

pub(crate) fn analysis_output(
    text: String,
    format: AnalysisResponseFormat,
    usage: Option<TokenUsage>,
) -> AnalysisOutput {
    let structured = if matches!(format, AnalysisResponseFormat::Json) {
        serde_json::from_str::<serde_json::Value>(&text).ok()
    } else {
        None
    };
    AnalysisOutput {
        text,
        structured,
        usage,
    }
}

pub(crate) fn chat_usage(u: ChatUsage) -> TokenUsage {
    TokenUsage {
        prompt: u.prompt_tokens,
        completion: u.completion_tokens,
        total: u.total_tokens,
    }
}

pub(crate) fn pick_generation_model(ctx: &ProviderContext, default: &str) -> String {
    ctx.extra
        .get("generation_model")
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

pub(crate) fn pick_analysis_model(ctx: &ProviderContext, default: &str) -> String {
    ctx.extra
        .get("analysis_model")
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}
