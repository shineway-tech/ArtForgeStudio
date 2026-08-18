use super::super::{
    create_atomic_temporary_file, ensure_managed_subdirectory, sync_parent_directory,
};
use super::{
    generation_content_policy_message, is_generation_content_policy_blocked, ApiClient, ApiError,
    SessionScope,
};
use reqwest::blocking::multipart::{Form, Part};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

const REFERENCE_TRANSFER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REFERENCE_TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct TaskFailure {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl TaskFailure {
    pub(crate) fn is_content_policy_blocked(&self) -> bool {
        is_generation_content_policy_blocked(&self.code, &self.message)
    }

    pub(crate) fn generation_message(&self) -> String {
        if !self.is_content_policy_blocked() {
            return if self.message.trim().is_empty() {
                "服务端未能生成该图片".to_string()
            } else {
                self.message.trim().to_string()
            };
        }

        generation_content_policy_message(&self.message)
    }
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

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CreateUpscaleGenerationTask {
    pub(crate) client_request_id: String,
    pub(crate) task_type: String,
    pub(crate) model_code: String,
    pub(crate) prompt: String,
    pub(crate) quality: String,
    pub(crate) reference_file_ids: Vec<String>,
    pub(crate) target_width: u32,
    pub(crate) target_height: u32,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CreateImageEditTask {
    pub(crate) client_request_id: String,
    pub(crate) task_type: String,
    pub(crate) model_code: String,
    pub(crate) prompt: String,
    pub(crate) quality: String,
    pub(crate) aspect_ratio: String,
    pub(crate) source_file_id: String,
    pub(crate) mask_file_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CreateWatermarkRemoval {
    pub(crate) client_request_id: String,
    pub(crate) reference_file_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CreateImageColorization {
    pub(crate) client_request_id: String,
    pub(crate) reference_file_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CreateImageEnhancement {
    pub(crate) client_request_id: String,
    pub(crate) reference_file_id: String,
    pub(crate) target_quality: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CreateImageCutout {
    pub(crate) client_request_id: String,
    pub(crate) reference_file_id: String,
    pub(crate) subject_type: String,
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
    sha256: &'a str,
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
            download: reqwest::blocking::Client::builder()
                .connect_timeout(REFERENCE_TRANSFER_CONNECT_TIMEOUT)
                .timeout(REFERENCE_TRANSFER_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::blocking::Client::new()),
        }
    }

    pub(crate) fn upload_reference(&self, path: &Path) -> Result<String, ApiError> {
        let prepared =
            super::super::prepare_reference_for_upload(path).map_err(|_| ApiError::LocalState {
                message: "无法在本地处理参考图，请更换图片后重试".to_string(),
            })?;
        self.upload_reference_file(prepared.path(), None)
    }

    pub(crate) fn upload_reference_scoped(
        &self,
        path: &Path,
        scope: &SessionScope,
    ) -> Result<String, ApiError> {
        let prepared =
            super::super::prepare_reference_for_upload(path).map_err(|_| ApiError::LocalState {
                message: "无法在本地处理参考图，请更换图片后重试".to_string(),
            })?;
        self.upload_reference_file(prepared.path(), Some(scope))
    }

    /// Uploads an image that has already been normalized for a paired operation such as
    /// image editing. The caller is responsible for keeping paired images at identical sizes.
    pub(crate) fn upload_prepared_reference(&self, path: &Path) -> Result<String, ApiError> {
        self.upload_reference_file(path, None)
    }

    pub(crate) fn upload_prepared_reference_scoped(
        &self,
        path: &Path,
        scope: &SessionScope,
    ) -> Result<String, ApiError> {
        self.upload_reference_file(path, Some(scope))
    }

    fn upload_reference_file(
        &self,
        path: &Path,
        scope: Option<&SessionScope>,
    ) -> Result<String, ApiError> {
        let bytes = fs::read(path).map_err(|error| ApiError::LocalState {
            message: format!("无法读取参考图：{error}"),
        })?;
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("reference.png");
        let mime = mime_for_path(path)?;
        let sha256 = sha256_hex(&bytes);
        let body = serde_json::to_value(PrepareUploadRequest {
            filename,
            mime_type: mime,
            size_bytes: bytes.len() as u64,
            sha256: &sha256,
        })
        .map_err(protocol_error)?;
        let prepared = match scope {
            Some(scope) => self
                .client
                .authenticated_json_scoped::<PrepareUploadResponse>(
                    Method::POST,
                    "/v1/uploads/references",
                    Some(body.clone()),
                    None,
                    scope,
                ),
            None => self.client.authenticated_json::<PrepareUploadResponse>(
                Method::POST,
                "/v1/uploads/references",
                Some(body),
                None,
            ),
        }?
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
            .map_err(|error| ApiError::LocalState {
                message: error.to_string(),
            })?;
        form = form.part(prepared.upload.file_field, part);
        if let Some(scope) = scope {
            self.ensure_scope_active(scope)?;
        }
        let response = self
            .download
            .post(&prepared.upload.url)
            .timeout(REFERENCE_TRANSFER_TIMEOUT)
            .multipart(form)
            .send()?;
        if !response.status().is_success() {
            return Err(ApiError::Protocol {
                message: format!("参考图上传失败（HTTP {}）", response.status().as_u16()),
                request_id: None,
            });
        }
        if let Some(scope) = scope {
            self.ensure_scope_active(scope)?;
        }
        let complete_path = format!("/v1/uploads/references/{}/complete", prepared.file.id);
        match scope {
            Some(scope) => self.client.authenticated_json_scoped::<serde_json::Value>(
                Method::POST,
                &complete_path,
                None,
                None,
                scope,
            )?,
            None => self.client.authenticated_json::<serde_json::Value>(
                Method::POST,
                &complete_path,
                None,
                None,
            )?,
        };
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

    pub(crate) fn delete_reference_scoped(
        &self,
        file_id: &str,
        scope: &SessionScope,
    ) -> Result<(), ApiError> {
        self.client.authenticated_json_scoped::<serde_json::Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_id}"),
            None,
            None,
            scope,
        )?;
        Ok(())
    }

    pub(crate) fn create_task(
        &self,
        request: &CreateGenerationTask,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body(&request.client_request_id, body)
    }

    pub(crate) fn create_task_scoped(
        &self,
        request: &CreateGenerationTask,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_upscale_task(
        &self,
        request: &CreateUpscaleGenerationTask,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body(&request.client_request_id, body)
    }

    pub(crate) fn create_upscale_task_scoped(
        &self,
        request: &CreateUpscaleGenerationTask,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_scoped(&request.client_request_id, body, scope)
    }

    pub(crate) fn create_image_edit_task(
        &self,
        request: &CreateImageEditTask,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body(&request.client_request_id, body)
    }

    pub(crate) fn create_image_edit_task_scoped(
        &self,
        request: &CreateImageEditTask,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_scoped(&request.client_request_id, body, scope)
    }

    pub(crate) fn create_watermark_removal(
        &self,
        request: &CreateWatermarkRemoval,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/watermark-removals",
                Some(body),
                Some(&request.client_request_id),
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_watermark_removal_scoped(
        &self,
        request: &CreateWatermarkRemoval,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/watermark-removals",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_image_colorization(
        &self,
        request: &CreateImageColorization,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-colorizations",
                Some(body),
                Some(&request.client_request_id),
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_image_colorization_scoped(
        &self,
        request: &CreateImageColorization,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-colorizations",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_image_enhancement(
        &self,
        request: &CreateImageEnhancement,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-enhancements",
                Some(body),
                Some(&request.client_request_id),
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_image_enhancement_scoped(
        &self,
        request: &CreateImageEnhancement,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-enhancements",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_image_cutout(
        &self,
        request: &CreateImageCutout,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-cutouts",
                Some(body),
                Some(&request.client_request_id),
            )
            .map(|response| response.data)
    }

    pub(crate) fn create_image_cutout_scoped(
        &self,
        request: &CreateImageCutout,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-cutouts",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    fn create_task_body(
        &self,
        client_request_id: &str,
        body: serde_json::Value,
    ) -> Result<GenerationTaskDetail, ApiError> {
        self.client
            .authenticated_json::<GenerationTaskDetail>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some(client_request_id),
            )
            .map(|response| response.data)
    }

    fn create_task_body_scoped(
        &self,
        client_request_id: &str,
        body: serde_json::Value,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        self.client
            .authenticated_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some(client_request_id),
                scope,
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

    pub(crate) fn task_scoped(
        &self,
        task_id: &str,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        self.client
            .authenticated_json_scoped::<GenerationTaskDetail>(
                Method::GET,
                &format!("/v1/generation/tasks/{task_id}"),
                None,
                None,
                scope,
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

    pub(crate) fn list_tasks_scoped(
        &self,
        status: &str,
        scope: &SessionScope,
    ) -> Result<Vec<GenerationTaskSummary>, ApiError> {
        self.client
            .authenticated_json_scoped::<GenerationTaskList>(
                Method::GET,
                &format!("/v1/generation/tasks?limit=20&status={status}"),
                None,
                None,
                scope,
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

    pub(crate) fn cancel_scoped(
        &self,
        task_id: &str,
        scope: &SessionScope,
    ) -> Result<(), ApiError> {
        self.client
            .authenticated_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                &format!("/v1/generation/tasks/{task_id}/cancel"),
                None,
                None,
                scope,
            )?;
        Ok(())
    }

    pub(crate) fn download_verified(&self, file: &TaskOutputFile) -> Result<Vec<u8>, ApiError> {
        self.download_verified_inner(file, None)
    }

    pub(crate) fn download_verified_scoped(
        &self,
        file: &TaskOutputFile,
        scope: &SessionScope,
    ) -> Result<Vec<u8>, ApiError> {
        self.download_verified_inner(file, Some(scope))
    }

    pub(crate) fn download_verified_to_path_scoped(
        &self,
        file: &TaskOutputFile,
        scope: &SessionScope,
        destination: &Path,
    ) -> Result<(), ApiError> {
        self.ensure_scope_active(scope)?;
        let url = file
            .download_url
            .as_deref()
            .ok_or_else(|| ApiError::Protocol {
                message: "生成文件下载地址暂不可用".to_string(),
                request_id: None,
            })?;
        let expected_size = file
            .size_bytes
            .parse::<u64>()
            .map_err(|_| ApiError::Protocol {
                message: "服务端返回了无效的文件大小".to_string(),
                request_id: None,
            })?;
        let parent = destination.parent().ok_or_else(|| ApiError::Protocol {
            message: "生成文件暂存路径无效".to_string(),
            request_id: None,
        })?;
        if !ensure_managed_subdirectory(parent) {
            return Err(ApiError::Protocol {
                message: "生成文件暂存目录不安全".to_string(),
                request_id: None,
            });
        }
        let (mut output, temporary) =
            create_atomic_temporary_file(destination).map_err(local_download_error)?;
        let result = (|| {
            let mut response = self.download.get(url).send()?.error_for_status()?;
            let mut hasher = Sha256::new();
            let mut total = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = response.read(&mut buffer).map_err(local_download_error)?;
                if read == 0 {
                    break;
                }
                total = total.saturating_add(read as u64);
                if total > expected_size {
                    return Err(ApiError::Protocol {
                        message: "生成文件完整性校验失败".to_string(),
                        request_id: None,
                    });
                }
                hasher.update(&buffer[..read]);
                output
                    .write_all(&buffer[..read])
                    .map_err(local_download_error)?;
            }
            output.sync_all().map_err(local_download_error)?;
            drop(output);
            self.ensure_scope_active(scope)?;
            let actual_sha = format!("{:x}", hasher.finalize());
            if total != expected_size || !actual_sha.eq_ignore_ascii_case(&file.sha256) {
                return Err(ApiError::Protocol {
                    message: "生成文件完整性校验失败".to_string(),
                    request_id: None,
                });
            }
            if destination.exists() {
                fs::remove_file(destination).map_err(local_download_error)?;
            }
            fs::rename(&temporary, destination).map_err(local_download_error)?;
            sync_parent_directory(destination).map_err(local_download_error)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn download_verified_inner(
        &self,
        file: &TaskOutputFile,
        scope: Option<&SessionScope>,
    ) -> Result<Vec<u8>, ApiError> {
        if let Some(scope) = scope {
            self.ensure_scope_active(scope)?;
        }
        let url = file
            .download_url
            .as_deref()
            .ok_or_else(|| ApiError::Protocol {
                message: "生成文件下载地址暂不可用".to_string(),
                request_id: None,
            })?;
        let bytes = self
            .download
            .get(url)
            .send()?
            .error_for_status()?
            .bytes()?
            .to_vec();
        if let Some(scope) = scope {
            self.ensure_scope_active(scope)?;
        }
        let expected_size = file
            .size_bytes
            .parse::<usize>()
            .map_err(|_| ApiError::Protocol {
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
        let body =
            serde_json::to_value(DeliveryAck { sha256, size_bytes }).map_err(protocol_error)?;
        self.client.authenticated_json::<serde_json::Value>(
            Method::POST,
            &format!("/v1/generation/tasks/{task_id}/deliveries/{file_id}/ack"),
            Some(body),
            None,
        )?;
        Ok(())
    }

    pub(crate) fn acknowledge_delivery_scoped(
        &self,
        task_id: &str,
        file_id: &str,
        sha256: &str,
        size_bytes: u64,
        scope: &SessionScope,
    ) -> Result<(), ApiError> {
        let body =
            serde_json::to_value(DeliveryAck { sha256, size_bytes }).map_err(protocol_error)?;
        self.client.authenticated_json_scoped::<serde_json::Value>(
            Method::POST,
            &format!("/v1/generation/tasks/{task_id}/deliveries/{file_id}/ack"),
            Some(body),
            None,
            scope,
        )?;
        Ok(())
    }

    fn ensure_scope_active(&self, scope: &SessionScope) -> Result<(), ApiError> {
        if self.client.session().is_scope_current(scope) {
            Ok(())
        } else {
            Err(ApiError::AuthenticationRequired)
        }
    }
}

fn local_download_error(error: std::io::Error) -> ApiError {
    ApiError::LocalState {
        message: format!("生成文件无法写入本地：{error}"),
    }
}

fn mime_for_path(path: &Path) -> Result<&'static str, ApiError> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "png" => Ok("image/png"),
        "webp" => Ok("image/webp"),
        _ => Err(ApiError::LocalState {
            message: "参考图只支持 JPEG、PNG 或 WebP".to_string(),
        }),
    }
}

fn protocol_error(error: serde_json::Error) -> ApiError {
    ApiError::Protocol {
        message: error.to_string(),
        request_id: None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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

    #[test]
    fn prepared_upload_includes_the_actual_content_sha256() {
        let bytes = b"reference-image";
        let sha256 = sha256_hex(bytes);
        let body = serde_json::to_value(PrepareUploadRequest {
            filename: "reference.png",
            mime_type: "image/png",
            size_bytes: bytes.len() as u64,
            sha256: &sha256,
        })
        .unwrap();

        assert_eq!(
            body["sha256"],
            "4110dd12af975f556bdac0299d0bfa04d42fa22d94f56b8550f1762e48fff7fb"
        );
    }

    #[test]
    fn image_edit_request_uses_explicit_source_and_mask_without_count() {
        let body = serde_json::to_value(CreateImageEditTask {
            client_request_id: "edit-request".to_string(),
            task_type: "image_edit".to_string(),
            model_code: "openai_image".to_string(),
            prompt: "replace the sky".to_string(),
            quality: "2K".to_string(),
            aspect_ratio: "16:9".to_string(),
            source_file_id: "source-file".to_string(),
            mask_file_id: "mask-file".to_string(),
        })
        .unwrap();

        assert_eq!(body["task_type"], "image_edit");
        assert_eq!(body["source_file_id"], "source-file");
        assert_eq!(body["mask_file_id"], "mask-file");
        assert_eq!(body["quality"], "2K");
        assert!(body.get("count").is_none());
        assert!(body.get("reference_file_ids").is_none());
    }

    #[test]
    fn content_policy_failure_is_classified_from_provider_code() {
        let failure = TaskFailure {
            code: "content_policy_violation".to_string(),
            message: "The generated image may violate safeguards about nudity or sexual content"
                .to_string(),
        };

        assert!(failure.is_content_policy_blocked());
        let message = failure.generation_message();
        assert!(message.contains("裸露、色情或情色内容"));
        assert!(message.contains("不返还积分"));
    }

    #[test]
    fn content_policy_failure_is_classified_from_clear_upstream_message() {
        let failure = TaskFailure {
            code: "provider_rejected".to_string(),
            message: "生成的图片可能违反了关于裸露、色情或情色内容的防护规则".to_string(),
        };

        assert!(failure.is_content_policy_blocked());
        assert!(failure.generation_message().contains("上游安全系统拦截"));
    }

    #[test]
    fn ordinary_provider_failure_keeps_its_original_message() {
        let failure = TaskFailure {
            code: "provider_timeout".to_string(),
            message: "上游模型响应超时，请重试".to_string(),
        };

        assert!(!failure.is_content_policy_blocked());
        assert_eq!(failure.generation_message(), "上游模型响应超时，请重试");
    }

    #[test]
    fn content_filter_service_error_is_not_treated_as_a_policy_block() {
        let failure = TaskFailure {
            code: "content_filter_service_error".to_string(),
            message: "内容审核服务暂时不可用，请稍后重试".to_string(),
        };

        assert!(!failure.is_content_policy_blocked());
        assert_eq!(
            failure.generation_message(),
            "内容审核服务暂时不可用，请稍后重试"
        );
    }
}
