use super::super::{
    create_atomic_temporary_file, ensure_managed_subdirectory, sync_parent_directory,
};
use super::{
    generation_content_policy_message, is_generation_content_policy_blocked, ApiClient, ApiError,
    BillingScope, SessionScope,
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
const GENERATION_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);

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
    pub(crate) billing_account_group_id: String,
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
    pub(crate) billing_account_group_id: String,
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
pub(crate) struct CreateVideoQuote {
    pub(crate) model_code: String,
    pub(crate) source_file_id: String,
    pub(crate) aspect_ratio: String,
    pub(crate) resolution: String,
    pub(crate) duration_secs: i32,
}

impl CreateVideoQuote {
    pub(crate) fn validate(&self) -> Result<(), ApiError> {
        if self.model_code.trim().is_empty() || self.source_file_id.trim().is_empty() {
            return Err(video_parameter_error("视频模型或源图片无效"));
        }
        if !matches!(
            self.aspect_ratio.as_str(),
            "21:9" | "16:9" | "4:3" | "1:1" | "3:4" | "9:16"
        ) {
            return Err(video_parameter_error("视频尺寸无效"));
        }
        if !matches!(self.resolution.as_str(), "480P" | "720P" | "1080P") {
            return Err(video_parameter_error("视频清晰度无效"));
        }
        if !(4..=15).contains(&self.duration_secs) {
            return Err(video_parameter_error("视频时长必须在 4 到 15 秒之间"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct VideoQuote {
    pub(crate) quote_id: String,
    pub(crate) credit_cost: String,
    pub(crate) expires_at: String,
    pub(crate) aspect_ratio: String,
    pub(crate) resolution: String,
    pub(crate) duration_secs: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CreateVideoGenerationTask {
    pub(crate) client_request_id: String,
    pub(crate) task_type: String,
    pub(crate) model_code: String,
    pub(crate) prompt: String,
    pub(crate) source_file_id: String,
    pub(crate) aspect_ratio: String,
    pub(crate) resolution: String,
    pub(crate) duration_secs: i32,
    pub(crate) quote_id: String,
}

impl CreateVideoGenerationTask {
    pub(crate) fn validate(&self) -> Result<(), ApiError> {
        CreateVideoQuote {
            model_code: self.model_code.clone(),
            source_file_id: self.source_file_id.clone(),
            aspect_ratio: self.aspect_ratio.clone(),
            resolution: self.resolution.clone(),
            duration_secs: self.duration_secs,
        }
        .validate()?;
        if self.client_request_id.trim().is_empty()
            || self.quote_id.trim().is_empty()
            || self.prompt.trim().is_empty()
            || self.task_type != "image_to_video"
        {
            return Err(video_parameter_error("视频生成请求无效"));
        }
        Ok(())
    }
}

fn video_parameter_error(message: &str) -> ApiError {
    ApiError::Protocol {
        message: message.to_string(),
        request_id: None,
    }
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
    saved_group: Option<String>,
}

impl GenerationApi {
    pub(crate) fn new(client: ApiClient) -> Self {
        Self {
            client,
            saved_group: None,
            download: reqwest::blocking::Client::builder()
                .connect_timeout(REFERENCE_TRANSFER_CONNECT_TIMEOUT)
                .timeout(GENERATION_DOWNLOAD_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::blocking::Client::new()),
        }
    }

    pub(crate) fn with_saved_group(mut self, group: &str) -> Self {
        self.saved_group = Some(group.to_owned());
        self
    }
    fn checked_saved_detail(&self, detail: GenerationTaskDetail) -> Result<GenerationTaskDetail, ApiError> {
        if let Some(group) = &self.saved_group { super::require_saved_group(group, &detail.billing_account_group_id)?; }
        Ok(detail)
    }
    pub(crate) fn upload_reference(&self, path: &Path) -> Result<String, ApiError> {
        let prepared =
            super::super::prepare_reference_for_upload(path).map_err(|_| ApiError::LocalState {
                message: "无法在本地处理参考图，请更换图片后重试".to_string(),
            })?;
        self.upload_reference_file(prepared.path(), None)
    }

    pub(crate) fn upload_reference_for_namespace(&self, path: &Path, authority: &super::super::NamespaceStorageAuthority,
        scope: &SessionScope, paired: bool,
    ) -> Result<String, ApiError> {
        if authority.lease().auth_epoch != scope.auth_epoch || authority.user_public_id() != scope.owner_user_id {
            return Err(ApiError::AuthenticationRequired);
        }
        let _activity = self.client.begin_user_work(scope)?;
        let read = self.client.upgrade_latch().begin_ordinary_blocking_effect().map_err(|required| required.as_error())?;
        let (bytes, filename, mime) = super::super::prepare_reference_bytes_for_namespace(authority, path, paired)
            .map_err(|_| ApiError::LocalState { message: "参考图不属于当前命名空间或无法安全读取".into() })?;
        drop(read);
        self.upload_reference_bytes(bytes, filename, mime, scope)
    }

    pub(crate) fn upload_reference_for_namespace_checked(&self, path: &Path,
        authority: &super::super::NamespaceStorageAuthority, scope: &SessionScope, paired: bool,
        expected_sha256: &str, expected_size_bytes: u64,
    ) -> Result<String, ApiError> {
        if authority.lease().auth_epoch != scope.auth_epoch || authority.user_public_id() != scope.owner_user_id {
            return Err(ApiError::AuthenticationRequired);
        }
        let activity=self.client.begin_user_work(scope)?;
        let read=self.client.upgrade_latch().begin_ordinary_blocking_effect().map_err(|required|required.as_error())?;
        let prepared=(||->anyhow::Result<_>{
            anyhow::ensure!(path.starts_with(authority.lease().namespace.root()),"reference outside original namespace");
            anyhow::ensure!(expected_size_bytes>0 && expected_sha256.len()==64,"retained fingerprint incomplete");
            // Exactly one held read. The bytes verified here are the same owned
            // bytes normalized below; neither stage reopens the source path.
            let bytes=authority.read_image_source(path,100*1024*1024)?;
            anyhow::ensure!(bytes.len() as u64==expected_size_bytes && sha256_hex(&bytes)==expected_sha256,
                "retained reference fingerprint changed");
            super::super::prepare_reference_upload_bytes(bytes,paired)
        })().map_err(|_|ApiError::LocalState{message:"原始参考图内容已变化或无法安全读取，任务记录已保留".into()});
        drop(read);
        if activity.is_quiescing(){return Err(ApiError::AuthenticationRequired);}
        let(bytes,filename,mime)=prepared?;
        self.upload_reference_bytes(bytes,filename,mime,scope)
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
        let scope = scope.ok_or(ApiError::AuthenticationRequired)?;
        let _activity = self.client.begin_user_work(scope)?;
        let read_permit = self.client.upgrade_latch().begin_ordinary_blocking_effect().map_err(|required| required.as_error())?;
        let bytes = fs::read(path).map_err(|error| ApiError::LocalState {
            message: format!("无法读取参考图：{error}"),
        })?;
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("reference.png");
        let mime = mime_for_path(path)?;
        drop(read_permit);
        self.upload_reference_bytes(bytes, filename, mime, scope)
    }

    fn upload_reference_bytes(&self, bytes: Vec<u8>, filename: &str, mime: &str, scope: &SessionScope) -> Result<String, ApiError> {
        let _activity = self.client.begin_user_work(scope)?;
        let sha256 = sha256_hex(&bytes);
        let body = serde_json::to_value(PrepareUploadRequest {
            filename,
            mime_type: mime,
            size_bytes: bytes.len() as u64,
            sha256: &sha256,
        })
        .map_err(protocol_error)?;
        let scope = Some(scope);
        let prepared = match scope {
            Some(scope) => self.client.identity_json_scoped::<PrepareUploadResponse>(
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
        let transfer = self.client.upgrade_latch().begin_ordinary_transfer().map_err(|required| required.as_error())?;
        let response = self
            .download
            .post(&prepared.upload.url)
            .timeout(REFERENCE_TRANSFER_TIMEOUT)
            .multipart(form)
            .send()?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let mut payload = Vec::new();
            response.take(64 * 1024).read_to_end(&mut payload).map_err(|_| ApiError::Protocol {
                message: "无法读取参考图上传响应".into(), request_id: None,
            })?;
            if status == 426 {
                if let Ok(envelope) = serde_json::from_slice::<super::ApiEnvelope<Value>>(&payload) {
                    if let Some(problem) = envelope.error {
                        let error = ApiError::Http { status, code: problem.code, message: problem.message, request_id: None, details: problem.details };
                        if let Some(required) = super::RequiredUpgrade::from_error(&error) {
                            let safe = required.as_error();
                            self.client.upgrade_latch().trip_from_ordinary_transfer(transfer, required, || drop(payload));
                            return Err(safe);
                        }
                    }
                }
            }
            return Err(ApiError::Protocol {
                message: format!("参考图上传失败（HTTP {status}）"),
                request_id: None,
            });
        }
        if let Some(scope) = scope {
            self.ensure_scope_active(scope)?;
        }
        drop(response);
        drop(transfer);
        let complete_path = format!("/v1/uploads/references/{}/complete", prepared.file.id);
        match scope {
            Some(scope) => self.client.identity_json_scoped::<serde_json::Value>(
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
        self.client.identity_json_scoped::<serde_json::Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_id}"),
            None,
            None,
            scope,
        )?;
        Ok(())
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_task(
        &self,
        request: &CreateGenerationTask,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body(&request.client_request_id, body)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    pub(crate) fn create_task_billing(
        &self,
        request: &CreateGenerationTask,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_billing(&request.client_request_id, body, scope)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn quote_video(&self, request: &CreateVideoQuote) -> Result<VideoQuote, ApiError> {
        request.validate()?;
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json::<VideoQuote>(
                Method::POST,
                "/v1/generation/video-quotes",
                Some(body),
                None,
            )
            .map(|response| response.data)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn quote_video_scoped(
        &self,
        request: &CreateVideoQuote,
        scope: &SessionScope,
    ) -> Result<VideoQuote, ApiError> {
        request.validate()?;
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json_scoped::<VideoQuote>(
                Method::POST,
                "/v1/generation/video-quotes",
                Some(body),
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn quote_video_billing(
        &self,
        request: &CreateVideoQuote,
        scope: &BillingScope,
    ) -> Result<VideoQuote, ApiError> {
        request.validate()?;
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .billing_json_scoped::<VideoQuote>(
                Method::POST,
                "/v1/generation/video-quotes",
                Some(body),
                None,
                scope,
            )
            .map(|response| response.data)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_video_task(
        &self,
        request: &CreateVideoGenerationTask,
    ) -> Result<GenerationTaskDetail, ApiError> {
        request.validate()?;
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body(&request.client_request_id, body)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_video_task_scoped(
        &self,
        request: &CreateVideoGenerationTask,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        request.validate()?;
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_scoped(&request.client_request_id, body, scope)
    }

    pub(crate) fn create_video_task_billing(
        &self,
        request: &CreateVideoGenerationTask,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        request.validate()?;
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_billing(&request.client_request_id, body, scope)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_upscale_task(
        &self,
        request: &CreateUpscaleGenerationTask,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body(&request.client_request_id, body)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_upscale_task_scoped(
        &self,
        request: &CreateUpscaleGenerationTask,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_scoped(&request.client_request_id, body, scope)
    }

    pub(crate) fn create_upscale_task_billing(
        &self,
        request: &CreateUpscaleGenerationTask,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_billing(&request.client_request_id, body, scope)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_image_edit_task(
        &self,
        request: &CreateImageEditTask,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body(&request.client_request_id, body)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
    pub(crate) fn create_image_edit_task_scoped(
        &self,
        request: &CreateImageEditTask,
        scope: &SessionScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_scoped(&request.client_request_id, body, scope)
    }

    pub(crate) fn create_image_edit_task_billing(
        &self,
        request: &CreateImageEditTask,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.create_task_body_billing(&request.client_request_id, body, scope)
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    pub(crate) fn create_watermark_removal_billing(
        &self,
        request: &CreateWatermarkRemoval,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .billing_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/watermark-removals",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .and_then(|response| {
                super::require_saved_group(&scope.request.account_group_id, &response.data.billing_account_group_id)?;
                self.checked_saved_detail(response.data)
            })
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    pub(crate) fn create_image_colorization_billing(
        &self,
        request: &CreateImageColorization,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .billing_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-colorizations",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .and_then(|response| {
                super::require_saved_group(&scope.request.account_group_id, &response.data.billing_account_group_id)?;
                self.checked_saved_detail(response.data)
            })
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    pub(crate) fn create_image_enhancement_billing(
        &self,
        request: &CreateImageEnhancement,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .billing_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-enhancements",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .and_then(|response| {
                super::require_saved_group(&scope.request.account_group_id, &response.data.billing_account_group_id)?;
                self.checked_saved_detail(response.data)
            })
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    pub(crate) fn create_image_cutout_billing(
        &self,
        request: &CreateImageCutout,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .billing_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/toolbox/image-cutouts",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .and_then(|response| {
                super::require_saved_group(&scope.request.account_group_id, &response.data.billing_account_group_id)?;
                self.checked_saved_detail(response.data)
            })
    }

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    // TEMP(team-accounts): remove in Task 10 after atomic caller migration
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

    fn create_task_body_billing(
        &self,
        client_request_id: &str,
        body: serde_json::Value,
        scope: &BillingScope,
    ) -> Result<GenerationTaskDetail, ApiError> {
        self.client
            .billing_json_scoped::<GenerationTaskDetail>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some(client_request_id),
                scope,
            )
            .and_then(|response| {
                super::require_saved_group(&scope.request.account_group_id, &response.data.billing_account_group_id)?;
                self.checked_saved_detail(response.data)
            })
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
            .identity_json_scoped::<GenerationTaskDetail>(
                Method::GET,
                &format!("/v1/generation/tasks/{task_id}"),
                None,
                None,
                scope,
            )
            .and_then(|response| self.checked_saved_detail(response.data))
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
            .identity_json_scoped::<GenerationTaskList>(
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
            .identity_json_scoped::<GenerationTaskDetail>(
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

    /// Streams into the caller's exclusive temporary. Publication and cleanup
    /// remain the caller's responsibility; no identity headers reach the blob host.
    pub(crate) fn download_verified_for_namespace(
        &self,
        file: &TaskOutputFile,
        scope: &SessionScope,
        authority: &super::super::NamespaceStorageAuthority,
        temporary: &mut super::super::NamespaceManagedFile,
    ) -> Result<(), ApiError> {
        let _activity = self.client.begin_user_work(scope)?;
        let _transfer = self.client.upgrade_latch().begin_ordinary_transfer().map_err(|required| required.as_error())?;
        self.ensure_scope_active(scope)?;
        if authority.user_public_id() != scope.owner_user_id
            || authority.lease().auth_epoch != scope.auth_epoch
        {
            return Err(ApiError::AuthenticationRequired);
        }
        let expected = file
            .size_bytes
            .parse::<u64>()
            .ok()
            .filter(|size| *size > 0 && size.to_string() == file.size_bytes)
            .ok_or_else(capability_integrity_error)?;
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(capability_integrity_error());
        }
        authority
            .inspect_regular(temporary)
            .map_err(capability_download_error)?;
        let url = file
            .download_url
            .as_deref()
            .filter(|url| !url.is_empty())
            .ok_or_else(capability_integrity_error)?;
        let response = self
            .download
            .get(url)
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|_| capability_transfer_error())?;
        self.ensure_scope_active(scope)?;
        let mut verified = NamespaceVerifiedReader {
            api: self,
            scope,
            response,
            expected,
            sha256: &file.sha256,
            total: 0,
            hasher: Sha256::new(),
            api_error: None,
        };
        match authority.write_new_regular_from(temporary, &mut verified) {
            Ok(_) => Ok(()),
            Err(error) => Err(verified
                .api_error
                .take()
                .unwrap_or_else(|| capability_download_error(error))),
        }
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
        let _activity = self.client.begin_user_work(scope)?;
        let _transfer = self.client.upgrade_latch().begin_ordinary_transfer().map_err(|required| required.as_error())?;
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
                self.ensure_scope_active(scope)?;
                let read = response.read(&mut buffer).map_err(local_download_error)?;
                self.ensure_scope_active(scope)?;
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
        let scope = scope.ok_or(ApiError::AuthenticationRequired)?;
        let _activity = self.client.begin_user_work(scope)?;
        let _transfer = self.client.upgrade_latch().begin_ordinary_transfer().map_err(|required| required.as_error())?;
        let scope = Some(scope);
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
        self.client.identity_json_scoped::<serde_json::Value>(
            Method::POST,
            &format!("/v1/generation/tasks/{task_id}/deliveries/{file_id}/ack"),
            Some(body),
            None,
            scope,
        )?;
        Ok(())
    }

    pub(crate) fn ensure_scope_active(&self, scope: &SessionScope) -> Result<(), ApiError> {
        if self.client.user_work_is_current(scope) {
            Ok(())
        } else {
            Err(ApiError::AuthenticationRequired)
        }
    }
}

fn capability_integrity_error() -> ApiError {
    ApiError::Protocol {
        message: "生成文件完整性校验失败".into(),
        request_id: None,
    }
}
fn capability_transfer_error() -> ApiError {
    ApiError::Protocol {
        message: "生成文件下载失败".into(),
        request_id: None,
    }
}
fn capability_download_error(_error: anyhow::Error) -> ApiError {
    ApiError::LocalState {
        message: "生成文件无法安全写入本地".into(),
    }
}
struct NamespaceVerifiedReader<'a> {
    api: &'a GenerationApi,
    scope: &'a SessionScope,
    response: reqwest::blocking::Response,
    expected: u64,
    sha256: &'a str,
    total: u64,
    hasher: Sha256,
    api_error: Option<ApiError>,
}
impl NamespaceVerifiedReader<'_> {
    fn fail(&mut self, error: ApiError) -> std::io::Error {
        self.api_error = Some(error);
        std::io::Error::other("verified namespace download failed")
    }
}
impl Read for NamespaceVerifiedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if let Err(error) = self.api.ensure_scope_active(self.scope) {
            return Err(self.fail(error));
        }
        let result = self.response.read(buffer);
        if let Err(error) = self.api.ensure_scope_active(self.scope) {
            return Err(self.fail(error));
        }
        let count = match result {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => return Err(error),
            Err(_) => return Err(self.fail(capability_transfer_error())),
        };
        if buffer.is_empty() {
            return Ok(0);
        }
        if count == 0 {
            if self.total != self.expected
                || !format!("{:x}", self.hasher.clone().finalize())
                    .eq_ignore_ascii_case(self.sha256)
            {
                return Err(self.fail(capability_integrity_error()));
            }
        } else {
            self.total = self
                .total
                .checked_add(count as u64)
                .filter(|total| *total <= self.expected)
                .ok_or_else(|| self.fail(capability_integrity_error()))?;
            self.hasher.update(&buffer[..count]);
        }
        Ok(count)
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

    #[test]
    fn generation_projections_require_billing_account_group_id() {
        let detail = serde_json::json!({
            "id": "task-1", "status": "queued", "progress_percent": 0,
            "success_count": 0, "failure_count": 0, "failure": null,
            "prompt": null, "result_prompt": null, "items": []
        });
        let summary = serde_json::json!({
            "id": "task-1", "type": "image_generation"
        });
        assert!(serde_json::from_value::<GenerationTaskDetail>(detail.clone()).is_err());
        assert!(serde_json::from_value::<GenerationTaskSummary>(summary.clone()).is_err());
        let mut detail_null = detail;
        detail_null["billing_account_group_id"] = serde_json::Value::Null;
        let mut summary_null = summary;
        summary_null["billing_account_group_id"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<GenerationTaskDetail>(detail_null).is_err());
        assert!(serde_json::from_value::<GenerationTaskSummary>(summary_null).is_err());
    }

    fn task(status: &str) -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "task-1".to_string(),
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".to_string(),
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
    fn video_quote_accepts_only_supported_parameters_and_serializes_decimal_credits() {
        for ratio in ["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"] {
            for resolution in ["480P", "720P", "1080P"] {
                for duration_secs in [4, 15] {
                    let request = CreateVideoQuote {
                        model_code: "seedance".to_string(),
                        source_file_id: "source-file".to_string(),
                        aspect_ratio: ratio.to_string(),
                        resolution: resolution.to_string(),
                        duration_secs,
                    };
                    assert!(request.validate().is_ok());
                }
            }
        }

        for (ratio, resolution, duration_secs) in [
            ("2:1", "720P", 4),
            ("16:9", "2K", 4),
            ("16:9", "720P", 3),
            ("16:9", "720P", 16),
        ] {
            assert!(CreateVideoQuote {
                model_code: "seedance".to_string(),
                source_file_id: "source-file".to_string(),
                aspect_ratio: ratio.to_string(),
                resolution: resolution.to_string(),
                duration_secs,
            }
            .validate()
            .is_err());
        }

        let quote: VideoQuote = serde_json::from_value(serde_json::json!({
            "quote_id": "quote-1",
            "credit_cost": "120",
            "expires_at": "2026-08-20T12:00:00Z",
            "aspect_ratio": "16:9",
            "resolution": "720P",
            "duration_secs": 8
        }))
        .unwrap();
        assert_eq!(quote.credit_cost, "120");
        assert_eq!(quote.duration_secs, 8);
    }

    #[test]
    fn image_to_video_task_has_explicit_quote_and_source_fields() {
        let body = serde_json::to_value(CreateVideoGenerationTask {
            client_request_id: "video-request".to_string(),
            task_type: "image_to_video".to_string(),
            model_code: "seedance".to_string(),
            prompt: "slow camera move".to_string(),
            source_file_id: "source-file".to_string(),
            aspect_ratio: "16:9".to_string(),
            resolution: "1080P".to_string(),
            duration_secs: 15,
            quote_id: "quote-1".to_string(),
        })
        .unwrap();

        assert_eq!(body["task_type"], "image_to_video");
        assert_eq!(body["source_file_id"], "source-file");
        assert_eq!(body["aspect_ratio"], "16:9");
        assert_eq!(body["resolution"], "1080P");
        assert_eq!(body["duration_secs"], 15);
        assert_eq!(body["quote_id"], "quote-1");
        assert!(body.get("count").is_none());
        assert!(body.get("quality").is_none());
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
