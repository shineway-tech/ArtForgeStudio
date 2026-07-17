use super::{ApiClient, ApiError};
use reqwest::blocking::multipart::{Form, Part};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct TaskFailure {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct TaskOutputFile {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) mime_type: String,
    pub(crate) size_bytes: String,
    pub(crate) sha256: String,
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
    pub(crate) download_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GenerationTaskItem {
    pub(crate) index: usize,
    pub(crate) status: String,
    pub(crate) credit_cost: String,
    pub(crate) failure: Option<TaskFailure>,
    pub(crate) file: Option<TaskOutputFile>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GenerationTaskDetail {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) progress_percent: i32,
    pub(crate) success_count: i32,
    pub(crate) failure_count: i32,
    pub(crate) failure: Option<TaskFailure>,
    pub(crate) prompt: Option<String>,
    pub(crate) result_prompt: Option<String>,
    #[serde(default)]
    pub(crate) request: Value,
    #[serde(default)]
    pub(crate) model: Option<TaskModel>,
    #[serde(default)]
    pub(crate) quality: String,
    #[serde(default)]
    pub(crate) requested_count: i32,
    #[serde(rename = "type", default)]
    pub(crate) task_type: String,
    pub(crate) items: Vec<GenerationTaskItem>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct TaskModel {
    pub(crate) code: String,
    pub(crate) version: u32,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GenerationTaskSummary {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) task_type: String,
}

#[derive(Deserialize)]
struct GenerationTaskList {
    items: Vec<GenerationTaskSummary>,
}

impl GenerationTaskDetail {
    pub(crate) fn terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "partially_completed" | "failed" | "cancelled"
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CreateGenerationTask {
    pub(crate) client_request_id: String,
    pub(crate) task_type: String,
    pub(crate) model_code: String,
    pub(crate) prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) aspect_ratio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reference_file_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_language: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct UploadFile {
    id: String,
}

#[derive(Clone, Debug, Deserialize)]
struct SignedUpload {
    method: String,
    url: String,
    fields: BTreeMap<String, String>,
    file_field: String,
}

#[derive(Clone, Debug, Deserialize)]
struct PrepareUploadResponse {
    file: UploadFile,
    upload: SignedUpload,
}

#[derive(Serialize)]
struct PrepareUploadRequest<'a> {
    filename: &'a str,
    mime_type: &'a str,
    size_bytes: u64,
}

#[derive(Serialize)]
struct DeliveryAck<'a> {
    sha256: &'a str,
    size_bytes: u64,
}

#[derive(Clone)]
pub(crate) struct GenerationApi {
    client: ApiClient,
    download: reqwest::blocking::Client,
}

impl GenerationApi {
    pub(crate) fn new(client: ApiClient) -> Self {
        Self {
            client,
            download: reqwest::blocking::Client::new(),
        }
    }

    pub(crate) fn upload_reference(&self, path: &Path) -> Result<String, ApiError> {
        let bytes = fs::read(path).map_err(|error| ApiError::LocalState {
            message: format!("无法读取参考图：{error}"),
        })?;
        let filename = path.file_name().and_then(|value| value.to_str()).unwrap_or("reference.png");
        let mime = mime_for_path(path)?;
        let body = serde_json::to_value(PrepareUploadRequest {
            filename,
            mime_type: mime,
            size_bytes: bytes.len() as u64,
        })
        .map_err(protocol_error)?;
        let prepared = self
            .client
            .authenticated_json::<PrepareUploadResponse>(
                Method::POST,
                "/v1/uploads/references",
                Some(body),
                None,
            )?
            .data;
        if prepared.upload.method != "POST" {
            return Err(ApiError::Protocol {
                message: "服务端返回了不支持的上传方式".to_string(),
                request_id: None,
            });
        }
        let mut form = Form::new();
        for (key, value) in prepared.upload.fields {
            form = form.text(key, value);
        }
        let part = Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(mime)
            .map_err(|error| ApiError::LocalState { message: error.to_string() })?;
        form = form.part(prepared.upload.file_field, part);
        let response = self.download.post(&prepared.upload.url).multipart(form).send()?;
        if !response.status().is_success() {
            return Err(ApiError::Protocol {
                message: format!("参考图上传失败（HTTP {}）", response.status().as_u16()),
                request_id: None,
            });
        }
        self.client.authenticated_json::<serde_json::Value>(
            Method::POST,
            &format!("/v1/uploads/references/{}/complete", prepared.file.id),
            None,
            None,
        )?;
        Ok(prepared.file.id)
    }

    pub(crate) fn delete_reference(&self, file_id: &str) {
        let _ = self.client.authenticated_json::<serde_json::Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_id}"),
            None,
            None,
        );
    }

    pub(crate) fn create_task(&self, request: &CreateGenerationTask) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json::<GenerationTaskDetail>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some(&request.client_request_id),
            )
            .map(|response| response.data)
    }

    pub(crate) fn task(&self, task_id: &str) -> Result<GenerationTaskDetail, ApiError> {
        self.client
            .authenticated_json::<GenerationTaskDetail>(
                Method::GET,
                &format!("/v1/generation/tasks/{task_id}"),
                None,
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn list_tasks(&self, status: &str) -> Result<Vec<GenerationTaskSummary>, ApiError> {
        self.client
            .authenticated_json::<GenerationTaskList>(
                Method::GET,
                &format!("/v1/generation/tasks?limit=20&status={status}"),
                None,
                None,
            )
            .map(|response| response.data.items)
    }

    pub(crate) fn cancel(&self, task_id: &str) -> Result<(), ApiError> {
        self.client.authenticated_json::<GenerationTaskDetail>(
            Method::POST,
            &format!("/v1/generation/tasks/{task_id}/cancel"),
            None,
            None,
        )?;
        Ok(())
    }

    pub(crate) fn download_verified(
        &self,
        file: &TaskOutputFile,
    ) -> Result<Vec<u8>, ApiError> {
        let url = file.download_url.as_deref().ok_or_else(|| ApiError::Protocol {
            message: "生成文件下载地址暂不可用".to_string(),
            request_id: None,
        })?;
        let bytes = self.download.get(url).send()?.error_for_status()?.bytes()?.to_vec();
        let expected_size = file.size_bytes.parse::<usize>().map_err(|_| ApiError::Protocol {
            message: "服务端返回了无效的文件大小".to_string(),
            request_id: None,
        })?;
        verify_downloaded_bytes(bytes, expected_size, &file.sha256)
    }

    pub(crate) fn acknowledge_delivery(
        &self,
        task_id: &str,
        file_id: &str,
        sha256: &str,
        size_bytes: u64,
    ) -> Result<(), ApiError> {
        let body = serde_json::to_value(DeliveryAck { sha256, size_bytes }).map_err(protocol_error)?;
        self.client.authenticated_json::<serde_json::Value>(
            Method::POST,
            &format!("/v1/generation/tasks/{task_id}/deliveries/{file_id}/ack"),
            Some(body),
            None,
        )?;
        Ok(())
    }
}

fn mime_for_path(path: &Path) -> Result<&'static str, ApiError> {
    match path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "png" => Ok("image/png"),
        "webp" => Ok("image/webp"),
        _ => Err(ApiError::LocalState { message: "参考图只支持 JPEG、PNG 或 WebP".to_string() }),
    }
}

fn protocol_error(error: serde_json::Error) -> ApiError {
    ApiError::Protocol { message: error.to_string(), request_id: None }
}

fn verify_downloaded_bytes(
    bytes: Vec<u8>,
    expected_size: usize,
    expected_sha256: &str,
) -> Result<Vec<u8>, ApiError> {
    let actual_sha = format!("{:x}", Sha256::digest(&bytes));
    if bytes.len() != expected_size || !actual_sha.eq_ignore_ascii_case(expected_sha256) {
        return Err(ApiError::Protocol {
            message: "生成文件完整性校验失败".to_string(),
            request_id: None,
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: &str) -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "task-1".to_string(),
            status: status.to_string(),
            progress_percent: 0,
            success_count: 0,
            failure_count: 0,
            failure: None,
            prompt: None,
            result_prompt: None,
            request: Value::Null,
            model: None,
            quality: "1K".to_string(),
            requested_count: 1,
            task_type: "image_generation".to_string(),
            items: Vec::new(),
        }
    }

    #[test]
    fn partial_success_failure_and_cancel_are_terminal() {
        for status in ["completed", "partially_completed", "failed", "cancelled"] {
            assert!(task(status).terminal(), "{status}");
        }
        for status in ["queued", "processing"] {
            assert!(!task(status).terminal(), "{status}");
        }
    }

    #[test]
    fn downloaded_file_must_match_size_and_sha256() {
        let bytes = b"generated-image".to_vec();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(
            verify_downloaded_bytes(bytes.clone(), bytes.len(), &hash).unwrap(),
            bytes
        );
        assert!(verify_downloaded_bytes(bytes.clone(), bytes.len() + 1, &hash).is_err());
        assert!(verify_downloaded_bytes(bytes, 15, &"0".repeat(64)).is_err());
    }
}
