use super::*;
use std::sync::{Mutex, OnceLock};

const RECOVERY_SCHEMA_VERSION: u32 = 2;
/// Exact retained request authority. Callers choose only a retained key, never a payer,
/// body, request type, or borrowed billing epoch. It deliberately carries no new-work grant.
pub(super) struct SavedReplayRequest {
    authority: Option<Arc<NamespaceStorageAuthority>>, redemption: Option<PrivatePersistence>, session: SessionScope,
    kind: SavedReplayKind, key: String, payer: String, source: serde_json::Value,
    body: serde_json::Value, path: String,
}
#[derive(Clone, Copy)]
enum SavedReplayKind { Generation, Prompt, Deep, Order, Redemption }
impl SavedReplayRequest {
    fn load(authority: Arc<NamespaceStorageAuthority>, session: &SessionScope, kind: SavedReplayKind, key: &str) -> Result<Self> {
        anyhow::ensure!(authority.user_public_id() == session.owner_user_id && authority.lease().auth_epoch == session.auth_epoch, "saved replay creator/namespace mismatch");
        let source = Self::source(&authority, kind, key)?;
        let payer = source.get("billing_account_group_id").and_then(serde_json::Value::as_str).ok_or_else(|| anyhow!("saved payer missing"))?.to_owned();
        let (path, body) = match kind {
            SavedReplayKind::Redemption => anyhow::bail!("redemption requires retained SQLite authority"),
            SavedReplayKind::Generation => {
                let record: PendingGenerationRecord = serde_json::from_value(source.clone())?;
                anyhow::ensure!(record.server_task_id.is_empty() && !record.canvas_ui_extraction && !record.cancel_requested, "accepted, cancelled or unsupported task cannot be replayed as new work");
                (saved_generation_create_path(&record)?.into(), saved_generation_create_body(&record)?)
            }
            SavedReplayKind::Prompt => {
                let record: PendingPromptTaskRecord = serde_json::from_value(source.clone())?;
                anyhow::ensure!(record.server_task_id.is_empty() && record.uploaded_file_ids.len() == record.reference_paths.len(), "saved prompt uploads incomplete");
                ("/v1/generation/tasks".into(), serde_json::to_value(prompt_task_create_request(&record))?)
            }
            SavedReplayKind::Deep => {
                let record: PendingPromptOptimizationRecord = serde_json::from_value(source.clone())?;
                anyhow::ensure!(record.server_job_id.is_empty(), "accepted prompt cannot be replayed as new work");
                match record.operation {
                    PendingPromptOptimizationOperation::Create { request } => {
                        anyhow::ensure!(request.client_request_id == key, "saved prompt key mismatch");
                        ("/v1/prompt-optimizations".into(), serde_json::to_value(request)?)
                    }
                    PendingPromptOptimizationOperation::Retry { source_job_id } =>
                        (format!("/v1/prompt-optimizations/{source_job_id}/retry"), serde_json::json!({"client_request_id":key})),
                }
            }
            SavedReplayKind::Order => {
                let record: PendingOrderRecord = serde_json::from_value(source.clone())?;
                anyhow::ensure!(record.order_id.is_empty(), "accepted order cannot be replayed as new work");
                match record.kind.as_str() {
                    "credit" => ("/v1/credits/orders".into(), serde_json::json!({"pack_code":record.product_code,"client_request_id":key})),
                    "membership" => ("/v1/membership/orders".into(), serde_json::json!({"plan_code":record.product_code,"client_request_id":key})),
                    "membership_upgrade" => {
                        anyhow::ensure!(!record.upgrade_quote_id.is_empty(), "missing original quote; no replacement quote is allowed");
                        ("/v1/membership/upgrade-orders".into(), serde_json::json!({"quote_id":record.upgrade_quote_id,"client_request_id":key}))
                    }
                    _ => anyhow::bail!("unsupported saved order type"),
                }
            }
        };
        Ok(Self { authority: Some(authority), redemption: None, session: session.clone(), kind, key: key.into(), payer, source, body, path })
    }
    fn source(authority: &NamespaceStorageAuthority, kind: SavedReplayKind, key: &str) -> Result<serde_json::Value> {
        let rows = match kind {
            SavedReplayKind::Redemption => anyhow::bail!("redemption requires retained SQLite authority"),
            SavedReplayKind::Generation => load_pending_generations_for_namespace(authority)?.into_iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>, _>>()?,
            SavedReplayKind::Prompt => load_pending_prompt_tasks_for_namespace(authority)?.into_iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>, _>>()?,
            SavedReplayKind::Deep => load_pending_prompt_optimizations_for_namespace(authority)?.into_iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>, _>>()?,
            SavedReplayKind::Order => load_pending_orders_for_namespace(authority)?.into_iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>, _>>()?,
        };
        rows.into_iter().find(|row| row.get("client_request_id").and_then(serde_json::Value::as_str) == Some(key)).ok_or_else(|| anyhow!("retained request not found"))
    }
    pub(super) fn generation(authority: Arc<NamespaceStorageAuthority>, session: &SessionScope, key: &str) -> Result<Self> { Self::load(authority, session, SavedReplayKind::Generation, key) }
    pub(super) fn prompt(authority: Arc<NamespaceStorageAuthority>, session: &SessionScope, key: &str) -> Result<Self> { Self::load(authority, session, SavedReplayKind::Prompt, key) }
    pub(super) fn deep(authority: Arc<NamespaceStorageAuthority>, session: &SessionScope, key: &str) -> Result<Self> { Self::load(authority, session, SavedReplayKind::Deep, key) }
    pub(super) fn order(authority: Arc<NamespaceStorageAuthority>, session: &SessionScope, key: &str) -> Result<Self> { Self::load(authority, session, SavedReplayKind::Order, key) }
    pub(super) fn redemption(persistence: PrivatePersistence, session: &SessionScope, key: &str) -> Result<Self> {
        let record = persistence.read_retained_redemption(session, key)?;
        let payer = record.billing_account_group_id.clone();
        let body = serde_json::json!({"code": record.code, "client_request_id": record.client_request_id});
        let source = serde_json::to_value(record)?;
        Ok(Self { authority: None, redemption: Some(persistence), session: session.clone(), kind: SavedReplayKind::Redemption,
            key: key.into(), payer, source, body, path: "/v1/credits/redemptions".into() })
    }
    pub(super) fn verify(&self) -> Result<()> {
        let source = if let Some(persistence) = &self.redemption {
            serde_json::to_value(persistence.read_retained_redemption(&self.session, &self.key)?)?
        } else {
            Self::source(self.authority.as_ref().ok_or_else(|| anyhow!("saved replay authority unavailable"))?, self.kind, &self.key)?
        };
        anyhow::ensure!(source == self.source, "retained replay request changed");
        Ok(())
    }
    pub(super) fn session(&self) -> &SessionScope { &self.session }
    pub(super) fn payer(&self) -> &str { &self.payer }
    pub(super) fn key(&self) -> &str { &self.key }
    pub(super) fn path(&self) -> &str { &self.path }
    pub(super) fn body(&self) -> serde_json::Value { self.body.clone() }
}
fn saved_generation_create_path(record: &PendingGenerationRecord) -> Result<&'static str> {
    Ok(match record.task_type.as_str() {
        "image_watermark_removal" => "/v1/toolbox/watermark-removals",
        "image_colorization" => "/v1/toolbox/image-colorizations",
        "image_enhancement" => "/v1/toolbox/image-enhancements",
        "image_cutout" => "/v1/toolbox/image-cutouts",
        "image_generation" | "image_edit" | "image_upscale" | "image_to_video" => "/v1/generation/tasks",
        _ => anyhow::bail!("unsupported saved generation type; record preserved"),
    })
}
fn saved_generation_create_body(record: &PendingGenerationRecord) -> Result<serde_json::Value> {
    anyhow::ensure!(record.uploaded_file_ids.len() == record.reference_paths.len() || record.reference_paths.is_empty(), "saved reference uploads incomplete");
    Ok(match record.task_type.as_str() {
        "image_to_video" => {
            let request = record.video_request.as_ref().ok_or_else(|| anyhow!("original video quote/body missing; record preserved"))?;
            request.validate()?;
            anyhow::ensure!(request.client_request_id == record.client_request_id && request.task_type == record.task_type, "retained video request identity mismatch");
            serde_json::to_value(request)?
        }
        "image_watermark_removal" | "image_colorization" | "image_enhancement" | "image_cutout" => {
            anyhow::ensure!(record.uploaded_file_ids.len() == 1 && !record.uploaded_file_ids[0].trim().is_empty(), "saved toolbox source missing");
            let reference_file_id = record.uploaded_file_ids[0].clone();
            match record.task_type.as_str() {
                "image_watermark_removal" => serde_json::to_value(CreateWatermarkRemoval { client_request_id: record.client_request_id.clone(), reference_file_id })?,
                "image_colorization" => serde_json::to_value(CreateImageColorization { client_request_id: record.client_request_id.clone(), reference_file_id })?,
                "image_enhancement" => {
                    anyhow::ensure!(matches!(record.quality.as_str(), "2K" | "4K"), "saved enhancement quality invalid");
                    serde_json::to_value(CreateImageEnhancement { client_request_id: record.client_request_id.clone(), reference_file_id, target_quality: record.quality.clone() })?
                }
                _ => {
                    anyhow::ensure!(matches!(record.quality.as_str(), "general" | "portrait" | "avatar" | "skin" | "product" | "clothing" | "sky"), "saved cutout subject invalid");
                    serde_json::to_value(CreateImageCutout { client_request_id: record.client_request_id.clone(), reference_file_id, subject_type: record.quality.clone() })?
                }
            }
        }
        "image_generation" => serde_json::to_value(CreateGenerationTask {
            client_request_id: record.client_request_id.clone(), task_type: record.task_type.clone(),
            model_code: record.model_code.clone(), prompt: record.generation_prompt.clone(),
            quality: Some(record.quality.clone()), count: Some(record.count), aspect_ratio: Some(api_aspect_ratio(&record.ratio)),
            reference_file_ids: Some(record.uploaded_file_ids.clone()), target_language: None,
        })?,
        "image_upscale" => serde_json::to_value(CreateUpscaleGenerationTask {
            client_request_id: record.client_request_id.clone(), task_type: record.task_type.clone(),
            model_code: record.model_code.clone(), prompt: record.generation_prompt.clone(), quality: record.quality.clone(),
            reference_file_ids: record.uploaded_file_ids.clone(), target_width: record.target_width, target_height: record.target_height,
        })?,
        "image_edit" => {
            anyhow::ensure!(record.uploaded_file_ids.len() == 2, "saved image edit inputs missing");
            serde_json::to_value(CreateImageEditTask {
                client_request_id: record.client_request_id.clone(), task_type: record.task_type.clone(), model_code: record.model_code.clone(),
                prompt: record.generation_prompt.clone(), quality: record.quality.clone(), aspect_ratio: api_aspect_ratio(&record.ratio),
                source_file_id: record.uploaded_file_ids[0].clone(), mask_file_id: record.uploaded_file_ids[1].clone(),
            })?
        }
        _ => anyhow::bail!("unsupported saved generation type; record preserved"),
    })
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct PendingDeliveryRecord {
    pub(super) item_index: usize,
    pub(super) file_id: String,
    pub(super) sha256: String,
    pub(super) size_bytes: u64,
    #[serde(default)]
    pub(super) local_path: String,
    #[serde(default)]
    pub(super) acknowledged: bool,
    #[serde(default)]
    pub(super) failed_asset_id: String,
    #[serde(default)]
    pub(super) abandoned: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PendingGenerationRecord {
    #[serde(default)]
    pub(super) source_asset_id: String,
    #[serde(default)]
    pub(super) video_request: Option<CreateVideoGenerationTask>,
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) cancel_requested: bool,
    #[serde(default)]
    pub(super) created_at_epoch_ms: i64,
    pub(super) client_request_id: String,
    pub(super) owner_user_id: String,
    pub(super) billing_account_group_id: String,
    #[serde(default)]
    pub(super) auth_epoch: u64,
    pub(super) local_task_id: String,
    #[serde(default)]
    pub(super) server_task_id: String,
    pub(super) raw_prompt: String,
    pub(super) generation_prompt: String,
    #[serde(default)]
    pub(super) task_type: String,
    pub(super) category: String,
    pub(super) mode: String,
    pub(super) ratio: String,
    pub(super) quality: String,
    pub(super) model_code: String,
    pub(super) conversation_id: String,
    pub(super) count: i32,
    #[serde(default)]
    pub(super) target_width: u32,
    #[serde(default)]
    pub(super) target_height: u32,
    pub(super) create_conversation: bool,
    #[serde(default)]
    pub(super) reference_paths: Vec<String>,
    #[serde(default)]
    pub(super) reference_sha256: Vec<String>,
    #[serde(default)]
    pub(super) reference_size_bytes: Vec<u64>,
    #[serde(default)]
    pub(super) lineage_reference_paths: Vec<String>,
    #[serde(default)]
    pub(super) uploaded_file_ids: Vec<String>,
    #[serde(default)]
    pub(super) deliveries: Vec<PendingDeliveryRecord>,
    #[serde(default)]
    pub(super) terminal: bool,
    #[serde(default)]
    pub(super) expected_success_count: usize,
    #[serde(default)]
    pub(super) canvas_source_node_id: String,
    #[serde(default)]
    pub(super) canvas_ui_extraction: bool,
}

#[derive(Serialize, Deserialize)]
struct RecoveryFile {
    schema_version: u32,
    generations: Vec<PendingGenerationRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PendingOrderRecord {
    pub(super) schema_version: u32,
    pub(super) kind: String,
    pub(super) client_request_id: String,
    pub(super) owner_user_id: String,
    pub(super) billing_account_group_id: String,
    #[serde(default)]
    pub(super) auth_epoch: u64,
    #[serde(default)]
    pub(super) order_id: String,
    pub(super) product_code: String,
    #[serde(default)]
    pub(super) upgrade_quote_id: String,
    pub(super) created_at: String,
}

#[derive(Serialize, Deserialize)]
struct OrderRecoveryFile {
    schema_version: u32,
    orders: Vec<PendingOrderRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct PendingPromptTaskRecord {
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) created_at_epoch_ms: i64,
    pub(super) client_request_id: String,
    pub(super) owner_user_id: String,
    pub(super) billing_account_group_id: String,
    #[serde(default)]
    pub(super) auth_epoch: u64,
    #[serde(default)]
    pub(super) server_task_id: String,
    pub(super) task_type: String,
    pub(super) model_code: String,
    pub(super) prompt: String,
    #[serde(default)]
    pub(super) target_language: Option<String>,
    #[serde(default)]
    pub(super) optimize: bool,
    pub(super) target_kind: String,
    #[serde(default)]
    pub(super) target_id: String,
    #[serde(default)]
    pub(super) target_category: String,
    #[serde(default)]
    pub(super) target_input: String,
    #[serde(default)]
    pub(super) append_result: bool,
    #[serde(default)]
    pub(super) activity_kind: String,
    #[serde(default)]
    pub(super) reference_paths: Vec<String>,
    #[serde(default)]
    pub(super) reference_sha256: Vec<String>,
    #[serde(default)]
    pub(super) reference_size_bytes: Vec<u64>,
    #[serde(default)]
    pub(super) uploaded_file_ids: Vec<String>,
    #[serde(default)]
    pub(super) result_prompt: String,
    #[serde(default)]
    pub(super) terminal_error: String,
    #[serde(default)]
    pub(super) applied_to_target: bool,
    #[serde(default)]
    pub(super) result_committed: bool,
    #[serde(default)]
    pub(super) cancel_requested: bool,
    #[serde(default = "prompt_submission_may_have_started")]
    pub(super) submission_started: bool,
}

fn prompt_submission_may_have_started() -> bool { true }

#[derive(Serialize, Deserialize)]
struct PromptTaskRecoveryFile {
    schema_version: u32,
    prompt_tasks: Vec<PendingPromptTaskRecord>,
    deep_optimizations: Vec<PendingPromptOptimizationRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum PendingPromptOptimizationOperation {
    Create { request: CreatePromptOptimization },
    Retry { source_job_id: String },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingPromptOptimizationRecord {
    pub(super) schema_version: u32,
    pub(super) client_request_id: String,
    pub(super) owner_user_id: String,
    pub(super) auth_epoch: u64,
    pub(super) billing_account_group_id: String,
    pub(super) server_job_id: String,
    #[serde(default)]
    pub(super) presentation_dismissed: bool,
    pub(super) operation: PendingPromptOptimizationOperation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecoveryError {
    NamespaceRequired,
    InvalidDocument,
    ScopeMismatch,
    IdentityChanged,
    ConflictExhausted,
}
impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NamespaceRequired => "recovery requires a user namespace",
            Self::InvalidDocument => "recovery document is invalid",
            Self::ScopeMismatch => "recovery scope does not match",
            Self::IdentityChanged => "recovery identity cannot be changed",
            Self::ConflictExhausted => "recovery publication conflicted repeatedly",
        })
    }
}
impl std::error::Error for RecoveryError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecoveryRecordIdentity {
    owner_user_id: String,
    billing_account_group_id: String,
    client_request_id: String,
    auth_epoch: u64,
}
fn canonical_uuid(value: &str) -> bool {
    api::uuid_path_segment(value).is_ok_and(|canonical| canonical == value)
}
impl RecoveryRecordIdentity {
    fn validate(&self) -> Result<()> {
        if !canonical_uuid(&self.owner_user_id)
            || !canonical_uuid(&self.billing_account_group_id)
            || self.client_request_id.trim().is_empty()
        {
            return Err(RecoveryError::InvalidDocument.into());
        }
        Ok(())
    }
    fn require_authority(
        &self,
        authority: &NamespaceStorageAuthority,
        current_epoch: bool,
    ) -> Result<()> {
        self.validate()?;
        if self.owner_user_id != authority.user_public_id()
            || (current_epoch && self.auth_epoch != authority.lease().auth_epoch)
        {
            return Err(RecoveryError::ScopeMismatch.into());
        }
        Ok(())
    }
    fn require_scope(
        &self,
        authority: &NamespaceStorageAuthority,
        scope: &BillingScope,
    ) -> Result<()> {
        self.require_authority(authority, true)?;
        if self.owner_user_id != scope.request.session.owner_user_id
            || self.auth_epoch != scope.request.session.auth_epoch
            || self.billing_account_group_id != scope.request.account_group_id
        {
            return Err(RecoveryError::ScopeMismatch.into());
        }
        Ok(())
    }
}
trait RecoveryRow {
    fn identity(&self) -> RecoveryRecordIdentity;
    fn validate(&self, authority: &NamespaceStorageAuthority) -> Result<()>;
}
macro_rules! recovery_row {
    ($ty:ty) => {
        impl $ty {
            pub(super) fn identity(&self) -> RecoveryRecordIdentity {
                RecoveryRecordIdentity {
                    owner_user_id: self.owner_user_id.clone(),
                    billing_account_group_id: self.billing_account_group_id.clone(),
                    client_request_id: self.client_request_id.clone(),
                    auth_epoch: self.auth_epoch,
                }
            }
        }
        impl RecoveryRow for $ty {
            fn identity(&self) -> RecoveryRecordIdentity {
                self.identity()
            }
            fn validate(&self, authority: &NamespaceStorageAuthority) -> Result<()> {
                self.identity().validate()?;
                if self.schema_version != RECOVERY_SCHEMA_VERSION
                    || self.owner_user_id != authority.user_public_id()
                {
                    return Err(RecoveryError::InvalidDocument.into());
                }
                Ok(())
            }
        }
    };
}
recovery_row!(PendingGenerationRecord);
recovery_row!(PendingPromptTaskRecord);
recovery_row!(PendingOrderRecord);
recovery_row!(PendingPromptOptimizationRecord);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoveryDocument {
    Generations,
    PromptTasks,
    Orders,
}
impl RecoveryDocument {
    fn key(self) -> Result<ManagedFileKey> {
        ManagedFileKey::new(
            ManagedUserArea::Recovery,
            match self {
                Self::Generations => "pending-generations.json",
                Self::PromptTasks => "pending-prompt-tasks.json",
                Self::Orders => "pending-orders.json",
            },
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum RowKind {
    Generation,
    Prompt,
    Deep,
    Order,
}
type Identities = Vec<(RowKind, RecoveryRecordIdentity)>;

trait RecoveryEnvelope: Serialize + serde::de::DeserializeOwned {
    const DOCUMENT: RecoveryDocument;
    fn empty() -> Self;
    fn schema_version(&self) -> u32;
    fn rows(&self) -> Vec<(RowKind, &dyn RecoveryRow)>;
    fn retained_request_bodies(&self) -> BTreeMap<String, serde_json::Value> { BTreeMap::new() }
    fn validate_extra(&self) -> Result<()> {
        Ok(())
    }
    fn validate(&self, authority: &NamespaceStorageAuthority) -> Result<Identities> {
        if self.schema_version() != RECOVERY_SCHEMA_VERSION {
            return Err(RecoveryError::InvalidDocument.into());
        }
        let mut keys = BTreeSet::new();
        let mut identities = Vec::new();
        for (kind, row) in self.rows() {
            row.validate(authority)?;
            let identity = row.identity();
            if !keys.insert(identity.client_request_id.clone()) {
                return Err(RecoveryError::InvalidDocument.into());
            }
            identities.push((kind, identity));
        }
        self.validate_extra()?;
        Ok(identities)
    }

}
impl RecoveryEnvelope for RecoveryFile {
    fn validate_extra(&self) -> Result<()> {
        for row in &self.generations {
            if let Some(request) = &row.video_request {
                request.validate()?;
                anyhow::ensure!(request.client_request_id == row.client_request_id && request.task_type == row.task_type
                    && row.task_type == "image_to_video", "retained video request identity mismatch");
            }
        }
        Ok(())
    }
    fn retained_request_bodies(&self) -> BTreeMap<String, serde_json::Value> {
        self.generations.iter().filter(|row| row.task_type == "image_to_video" || row.video_request.is_some())
            .map(|row| (row.client_request_id.clone(), serde_json::json!({"task_type": row.task_type, "video_request": row.video_request, "source_asset_id": row.source_asset_id}))).collect()
    }
    const DOCUMENT: RecoveryDocument = RecoveryDocument::Generations;
    fn empty() -> Self {
        Self {
            schema_version: RECOVERY_SCHEMA_VERSION,
            generations: Vec::new(),
        }
    }
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn rows(&self) -> Vec<(RowKind, &dyn RecoveryRow)> {
        self.generations
            .iter()
            .map(|row| (RowKind::Generation, row as &dyn RecoveryRow))
            .collect()
    }
}
impl RecoveryEnvelope for OrderRecoveryFile {
    const DOCUMENT: RecoveryDocument = RecoveryDocument::Orders;
    fn empty() -> Self {
        Self {
            schema_version: RECOVERY_SCHEMA_VERSION,
            orders: Vec::new(),
        }
    }
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn rows(&self) -> Vec<(RowKind, &dyn RecoveryRow)> {
        self.orders
            .iter()
            .map(|row| (RowKind::Order, row as &dyn RecoveryRow))
            .collect()
    }
}
impl RecoveryEnvelope for PromptTaskRecoveryFile {
    const DOCUMENT: RecoveryDocument = RecoveryDocument::PromptTasks;
    fn empty() -> Self {
        Self {
            schema_version: RECOVERY_SCHEMA_VERSION,
            prompt_tasks: Vec::new(),
            deep_optimizations: Vec::new(),
        }
    }
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn rows(&self) -> Vec<(RowKind, &dyn RecoveryRow)> {
        self.prompt_tasks
            .iter()
            .map(|row| (RowKind::Prompt, row as &dyn RecoveryRow))
            .chain(
                self.deep_optimizations
                    .iter()
                    .map(|row| (RowKind::Deep, row as &dyn RecoveryRow)),
            )
            .collect()
    }
    fn validate_extra(&self) -> Result<()> {
        for row in &self.deep_optimizations {
            if row.presentation_dismissed && row.server_job_id.is_empty() {
                return Err(RecoveryError::InvalidDocument.into());
            }
            if !row.server_job_id.is_empty() && !canonical_uuid(&row.server_job_id) {
                return Err(RecoveryError::InvalidDocument.into());
            }
            match &row.operation {
                PendingPromptOptimizationOperation::Create { request }
                    if request.client_request_id != row.client_request_id =>
                {
                    return Err(RecoveryError::InvalidDocument.into())
                }
                PendingPromptOptimizationOperation::Retry { source_job_id }
                    if !canonical_uuid(source_job_id) =>
                {
                    return Err(RecoveryError::InvalidDocument.into())
                }
                _ => {}
            }
        }
        Ok(())
    }
}

enum MutationPermission {
    Preserve,
    Upsert(RowKind, RecoveryRecordIdentity),
    Remove(RowKind, RecoveryRecordIdentity),
    Rebind(RowKind, RecoveryRecordIdentity, u64),
}
fn validate_identity_delta(
    before: &Identities,
    after: &Identities,
    permission: &MutationPermission,
) -> Result<()> {
    if before == after {
        return Ok(());
    }
    let mut expected = before.clone();
    match permission {
        MutationPermission::Preserve => {}
        MutationPermission::Upsert(kind, identity) => {
            if before
                .iter()
                .any(|(_, saved)| saved.client_request_id == identity.client_request_id)
            {
                return Err(RecoveryError::IdentityChanged.into());
            }
            let position = expected.partition_point(|(saved_kind, _)| saved_kind <= kind);
            expected.insert(position, (*kind, identity.clone()));
        }
        MutationPermission::Remove(kind, identity) => {
            expected.retain(|saved| saved != &(*kind, identity.clone()));
        }
        MutationPermission::Rebind(kind, identity, epoch) => {
            if let Some((_, saved)) = expected
                .iter_mut()
                .find(|(saved_kind, saved)| saved_kind == kind && saved == identity)
            {
                saved.auth_epoch = *epoch;
            }
        }
    }
    if &expected != after {
        return Err(RecoveryError::IdentityChanged.into());
    }
    Ok(())
}
fn recovery_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
struct NamespaceRecoveryStore<'a> {
    authority: &'a NamespaceStorageAuthority,
}
impl NamespaceRecoveryStore<'_> {
    fn read<D: RecoveryEnvelope>(&self) -> Result<(D, Option<NamespaceManagedFile>)> {
        let mut destination = self.authority.open_optional_regular(&D::DOCUMENT.key()?)?;
        let document = match destination.as_mut() {
            Some(file) => {
                let mut bytes = Vec::new();
                self.authority.read_regular_to(file, &mut bytes)?;
                serde_json::from_slice(&bytes).map_err(|_| RecoveryError::InvalidDocument)?
            }
            None => D::empty(),
        };
        document.validate(self.authority)?;
        Ok((document, destination))
    }
    fn load<D: RecoveryEnvelope>(&self) -> Result<D> {
        let _guard = recovery_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.read().map(|(document, _)| document)
    }
    fn mutate<D: RecoveryEnvelope, T>(
        &self,
        permission: MutationPermission,
        mut update: impl FnMut(&mut D) -> Result<T>,
    ) -> Result<T> {
        let _mutation = self.authority.begin_ordinary_mutation()?;
        let _guard = recovery_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = D::DOCUMENT.key()?;
        for attempt in 0..3 {
            let (mut document, retained) = self.read::<D>()?;
            let before = document.validate(self.authority)?;
            let retained_bodies = document.retained_request_bodies();
            let tentative = update(&mut document)?;
            let after = document.validate(self.authority)?;
            validate_identity_delta(&before, &after, &permission)?;
            let next_bodies = document.retained_request_bodies();
            for key in next_bodies.keys() {
                anyhow::ensure!(!before.iter().any(|(_, identity)| &identity.client_request_id == key)
                    || retained_bodies.contains_key(key),
                    "an existing request cannot become a different video operation");
            }
            for (key, body) in &retained_bodies {
                if after.iter().any(|(_, identity)| &identity.client_request_id == key) {
                    anyhow::ensure!(next_bodies.get(key) == Some(body), "retained video request body cannot be replaced or cleared");
                }
            }
            let bytes = serde_json::to_vec_pretty(&document)?;
            let mut temporary = self.authority.create_temporary_regular_for(&key)?;
            let write_result = self
                .authority
                .write_new_regular_from(&mut temporary, &mut bytes.as_slice())
                .and_then(|_| self.authority.sync_regular(&mut temporary));
            if let Err(primary) = write_result {
                return Err(self.cleanup_error(temporary, primary));
            }
            let publication = match retained.as_ref() {
                Some(file) => NamespaceManagedPublication::Replace(file),
                None => NamespaceManagedPublication::Absent(&key),
            };
            match self.authority.publish_regular(&mut temporary, publication) {
                Ok(()) => return Ok(tentative),
                Err(primary) => {
                    let conflict = matches!(
                        primary.downcast_ref::<ManagedPublicationConflict>(),
                        Some(
                            ManagedPublicationConflict::DestinationAppeared
                                | ManagedPublicationConflict::DestinationChanged
                        )
                    );
                    // Cleanup can fail if the binding changed; retain the primary downcastable error
                    // and stop. Never turn a pathname into replacement cleanup authority.
                    if let Err(cleanup) = self.authority.unlink_regular(temporary) {
                        return Err(primary
                            .context(format!("recovery temporary cleanup failed: {cleanup}")));
                    }
                    if !conflict {
                        return Err(primary);
                    }
                    if attempt == 2 {
                        return Err(RecoveryError::ConflictExhausted.into());
                    }
                }
            }
        }
        unreachable!("three publication attempts either commit or return an error")
    }
    fn cleanup_error(
        &self,
        temporary: NamespaceManagedFile,
        primary: anyhow::Error,
    ) -> anyhow::Error {
        match self.authority.unlink_regular(temporary) {
            Ok(()) => primary,
            Err(cleanup) => {
                primary.context(format!("recovery temporary cleanup failed: {cleanup}"))
            }
        }
    }
    fn load_generations(&self) -> Result<RecoveryFile> {
        self.load()
    }
    fn load_prompt_tasks(&self) -> Result<PromptTaskRecoveryFile> {
        self.load()
    }
    fn load_orders(&self) -> Result<OrderRecoveryFile> {
        self.load()
    }
    fn mutate_generations<T>(
        &self,
        update: impl FnMut(&mut RecoveryFile) -> Result<T>,
    ) -> Result<T> {
        self.mutate(MutationPermission::Preserve, update)
    }
    fn mutate_prompt_tasks<T>(
        &self,
        update: impl FnMut(&mut PromptTaskRecoveryFile) -> Result<T>,
    ) -> Result<T> {
        self.mutate(MutationPermission::Preserve, update)
    }
    fn mutate_orders<T>(
        &self,
        update: impl FnMut(&mut OrderRecoveryFile) -> Result<T>,
    ) -> Result<T> {
        self.mutate(MutationPermission::Preserve, update)
    }
}

pub(super) fn load_pending_generations_for_namespace(
    authority: &NamespaceStorageAuthority,
) -> Result<Vec<PendingGenerationRecord>> {
    let mut records = NamespaceRecoveryStore { authority }
        .load_generations()?
        .generations;
    for record in &mut records {
        for path in record.reference_paths.iter_mut().chain(record.lineage_reference_paths.iter_mut()) {
            authority.lease().namespace.remap_path(path);
        }
    }
    Ok(records)
}
pub(super) fn load_pending_prompt_tasks_for_namespace(
    authority: &NamespaceStorageAuthority,
) -> Result<Vec<PendingPromptTaskRecord>> {
    let mut records = NamespaceRecoveryStore { authority }
        .load_prompt_tasks()?
        .prompt_tasks;
    for record in &mut records {
        for path in &mut record.reference_paths {
            authority.lease().namespace.remap_path(path);
        }
    }
    Ok(records)
}
pub(super) fn load_pending_prompt_optimizations_for_namespace(
    authority: &NamespaceStorageAuthority,
) -> Result<Vec<PendingPromptOptimizationRecord>> {
    Ok(NamespaceRecoveryStore { authority }
        .load_prompt_tasks()?
        .deep_optimizations)
}
pub(super) fn load_pending_orders_for_namespace(
    authority: &NamespaceStorageAuthority,
) -> Result<Vec<PendingOrderRecord>> {
    Ok(NamespaceRecoveryStore { authority }.load_orders()?.orders)
}
fn exact_index<R: RecoveryRow>(rows: &[R], expected: &RecoveryRecordIdentity) -> Option<usize> {
    let mut indexes = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.identity() == *expected)
        .map(|(index, _)| index);
    let index = indexes.next()?;
    indexes.next().is_none().then_some(index)
}
fn upsert_row<R: RecoveryRow + Clone>(rows: &mut Vec<R>, record: &R) -> Result<()> {
    let identity = record.identity();
    match rows
        .iter()
        .position(|row| row.identity().client_request_id == identity.client_request_id)
    {
        Some(index) if rows[index].identity() == identity => rows[index] = record.clone(),
        Some(_) => return Err(RecoveryError::IdentityChanged.into()),
        None => rows.push(record.clone()),
    }
    Ok(())
}

pub(super) fn upsert_pending_generation_for_namespace(
    authority: &NamespaceStorageAuthority,
    scope: &BillingScope,
    record: PendingGenerationRecord,
) -> Result<()> {
    let identity = record.identity();
    identity.require_scope(authority, scope)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Upsert(RowKind::Generation, identity),
        |file: &mut RecoveryFile| upsert_row(&mut file.generations, &record),
    )
}
pub(super) fn remove_pending_generation_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Remove(RowKind::Generation, expected.clone()),
        |file: &mut RecoveryFile| {
            let Some(index) = exact_index(&file.generations, expected) else {
                return Ok(false);
            };
            file.generations.remove(index);
            Ok(true)
        },
    )
}
pub(super) fn rebind_pending_generation_epoch_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    new_auth_epoch: u64,
) -> Result<bool> {
    expected.require_authority(authority, false)?;
    if new_auth_epoch != authority.lease().auth_epoch {
        return Err(RecoveryError::ScopeMismatch.into());
    }
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Rebind(RowKind::Generation, expected.clone(), new_auth_epoch),
        |file: &mut RecoveryFile| {
            let Some(index) = exact_index(&file.generations, expected) else {
                return Ok(false);
            };
            file.generations[index].auth_epoch = new_auth_epoch;
            Ok(true)
        },
    )
}

pub(super) fn upsert_pending_prompt_task_for_namespace(
    authority: &NamespaceStorageAuthority,
    scope: &BillingScope,
    record: PendingPromptTaskRecord,
) -> Result<()> {
    let identity = record.identity();
    identity.require_scope(authority, scope)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Upsert(RowKind::Prompt, identity),
        |file: &mut PromptTaskRecoveryFile| upsert_row(&mut file.prompt_tasks, &record),
    )
}
pub(super) fn remove_pending_prompt_task_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Remove(RowKind::Prompt, expected.clone()),
        |file: &mut PromptTaskRecoveryFile| {
            let Some(index) = exact_index(&file.prompt_tasks, expected) else {
                return Ok(false);
            };
            file.prompt_tasks.remove(index);
            Ok(true)
        },
    )
}
pub(super) fn rebind_pending_prompt_task_epoch_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    new_auth_epoch: u64,
) -> Result<bool> {
    expected.require_authority(authority, false)?;
    if new_auth_epoch != authority.lease().auth_epoch {
        return Err(RecoveryError::ScopeMismatch.into());
    }
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Rebind(RowKind::Prompt, expected.clone(), new_auth_epoch),
        |file: &mut PromptTaskRecoveryFile| {
            let Some(index) = exact_index(&file.prompt_tasks, expected) else {
                return Ok(false);
            };
            file.prompt_tasks[index].auth_epoch = new_auth_epoch;
            Ok(true)
        },
    )
}

pub(super) fn upsert_pending_order_for_namespace(
    authority: &NamespaceStorageAuthority,
    scope: &BillingScope,
    record: PendingOrderRecord,
) -> Result<()> {
    let identity = record.identity();
    identity.require_scope(authority, scope)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Upsert(RowKind::Order, identity),
        |file: &mut OrderRecoveryFile| upsert_row(&mut file.orders, &record),
    )
}
pub(super) fn remove_pending_order_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Remove(RowKind::Order, expected.clone()),
        |file: &mut OrderRecoveryFile| {
            let Some(index) = exact_index(&file.orders, expected) else {
                return Ok(false);
            };
            file.orders.remove(index);
            Ok(true)
        },
    )
}
pub(super) fn rebind_pending_order_epoch_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    new_auth_epoch: u64,
) -> Result<bool> {
    expected.require_authority(authority, false)?;
    if new_auth_epoch != authority.lease().auth_epoch {
        return Err(RecoveryError::ScopeMismatch.into());
    }
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Rebind(RowKind::Order, expected.clone(), new_auth_epoch),
        |file: &mut OrderRecoveryFile| {
            let Some(index) = exact_index(&file.orders, expected) else {
                return Ok(false);
            };
            file.orders[index].auth_epoch = new_auth_epoch;
            Ok(true)
        },
    )
}

pub(super) fn upsert_pending_prompt_optimization_for_namespace(
    authority: &NamespaceStorageAuthority,
    scope: &BillingScope,
    record: PendingPromptOptimizationRecord,
) -> Result<()> {
    let identity = record.identity();
    identity.require_scope(authority, scope)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Upsert(RowKind::Deep, identity),
        |file: &mut PromptTaskRecoveryFile| {
            if let Some(saved) = file
                .deep_optimizations
                .iter()
                .find(|saved| saved.client_request_id == record.client_request_id)
            {
                if serde_json::to_value(&saved.operation)?
                    != serde_json::to_value(&record.operation)?
                    || saved.server_job_id != record.server_job_id
                    || (saved.presentation_dismissed && !record.presentation_dismissed)
                {
                    return Err(RecoveryError::IdentityChanged.into());
                }
            }
            upsert_row(&mut file.deep_optimizations, &record)
        },
    )
}
/// Presentation-only closure of an exact retained job. The caller separately
/// verifies the server terminal state; this never rewrites historical authority.
pub(super) fn mark_pending_prompt_optimization_presentation_dismissed_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    expected_server_job_id: &str,
) -> Result<bool> {
    expected.require_authority(authority, false)?;
    anyhow::ensure!(canonical_uuid(expected_server_job_id), "dismissal requires an exact canonical job");
    let _unit = authority.begin_ordinary_mutation()?;
    let store = NamespaceRecoveryStore { authority };
    let retained = store.load_prompt_tasks()?;
    let Some(index) = exact_index(&retained.deep_optimizations, expected) else { return Ok(false); };
    let row = &retained.deep_optimizations[index];
    if row.server_job_id != expected_server_job_id { return Ok(false); }
    if row.presentation_dismissed { return Ok(true); }
    store.mutate(MutationPermission::Preserve, |file: &mut PromptTaskRecoveryFile| {
        let Some(index) = exact_index(&file.deep_optimizations, expected) else { return Ok(false); };
        let row = &mut file.deep_optimizations[index];
        if row.server_job_id != expected_server_job_id { return Ok(false); }
        row.presentation_dismissed = true;
        Ok(true)
    })
}
pub(super) fn remove_pending_prompt_optimization_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Remove(RowKind::Deep, expected.clone()),
        |file: &mut PromptTaskRecoveryFile| {
            let Some(index) = exact_index(&file.deep_optimizations, expected) else {
                return Ok(false);
            };
            file.deep_optimizations.remove(index);
            Ok(true)
        },
    )
}
pub(super) fn rebind_pending_prompt_optimization_epoch_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    new_auth_epoch: u64,
) -> Result<bool> {
    expected.require_authority(authority, false)?;
    if new_auth_epoch != authority.lease().auth_epoch {
        return Err(RecoveryError::ScopeMismatch.into());
    }
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Rebind(RowKind::Deep, expected.clone(), new_auth_epoch),
        |file: &mut PromptTaskRecoveryFile| {
            let Some(index) = exact_index(&file.deep_optimizations, expected) else {
                return Ok(false);
            };
            file.deep_optimizations[index].auth_epoch = new_auth_epoch;
            Ok(true)
        },
    )
}

#[derive(Clone)]
pub(super) enum GenerationRecoveryPatch {
    RequestCancellation,
    BeginSubmission,
    UploadedFileIds(Vec<String>),
    UploadedAndReleaseInputs(Vec<String>),
    Accepted {
        server_task_id: String,
        uploaded_file_ids: Vec<String>,
        clear_reference_inputs: bool,
    },
    ReleaseReferenceInputs,
    ClearDeliveryLocalPaths(BTreeSet<String>),
    Terminal {
        expected_success_count: usize,
    },
}
#[derive(Clone)]
pub(super) enum PromptTaskRecoveryPatch {
    RequestCancellation,
    BeginSubmission,
    UploadedFileIds(Vec<String>),
    ServerTaskId(String),
    TerminalError(String),
    ResultPrompt(String),
    ResultCommitted,
    AppliedToTarget,
    ReleaseCustomPromptResult,
}
fn release_reference_inputs(record: &mut PendingGenerationRecord) {
    record.reference_paths.clear();
    record.reference_sha256.clear();
    record.reference_size_bytes.clear();
}
pub(super) fn apply_generation_patch_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    patch: GenerationRecoveryPatch,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    NamespaceRecoveryStore { authority }.mutate_generations(|file| {
        let Some(index) = exact_index(&file.generations, expected) else {
            return Ok(false);
        };
        let record = &mut file.generations[index];
        match patch.clone() {
            GenerationRecoveryPatch::RequestCancellation => record.cancel_requested = true,
            GenerationRecoveryPatch::BeginSubmission => {
                anyhow::ensure!(!record.cancel_requested, "cancelled generation cannot be submitted");
            }
            GenerationRecoveryPatch::UploadedFileIds(ids) => record.uploaded_file_ids = ids,
            GenerationRecoveryPatch::UploadedAndReleaseInputs(ids) => {
                record.uploaded_file_ids = ids;
                release_reference_inputs(record);
            }
            GenerationRecoveryPatch::Accepted {
                server_task_id,
                uploaded_file_ids,
                clear_reference_inputs,
            } => {
                record.server_task_id = server_task_id;
                record.uploaded_file_ids = uploaded_file_ids;
                if clear_reference_inputs {
                    release_reference_inputs(record);
                }
            }
            GenerationRecoveryPatch::ReleaseReferenceInputs => release_reference_inputs(record),
            GenerationRecoveryPatch::ClearDeliveryLocalPaths(ids) => {
                for delivery in &mut record.deliveries {
                    if ids.contains(&delivery.file_id) {
                        delivery.local_path.clear();
                    }
                }
            }
            GenerationRecoveryPatch::Terminal {
                expected_success_count,
            } => {
                record.terminal = true;
                record.expected_success_count = expected_success_count;
            }
        }
        Ok(true)
    })
}
pub(super) fn apply_prompt_task_patch_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    patch: PromptTaskRecoveryPatch,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    NamespaceRecoveryStore { authority }.mutate_prompt_tasks(|file| {
        let Some(index) = exact_index(&file.prompt_tasks, expected) else {
            return Ok(false);
        };
        let record = &mut file.prompt_tasks[index];
        match patch.clone() {
            PromptTaskRecoveryPatch::RequestCancellation => record.cancel_requested = true,
            PromptTaskRecoveryPatch::BeginSubmission => {
                anyhow::ensure!(!record.cancel_requested, "cancelled prompt cannot be submitted");
                record.submission_started = true;
            }
            PromptTaskRecoveryPatch::UploadedFileIds(ids) => record.uploaded_file_ids = ids,
            PromptTaskRecoveryPatch::ServerTaskId(id) => record.server_task_id = id,
            PromptTaskRecoveryPatch::TerminalError(error) => record.terminal_error = error,
            PromptTaskRecoveryPatch::ResultPrompt(prompt) => record.result_prompt = prompt,
            PromptTaskRecoveryPatch::ResultCommitted => record.result_committed = true,
            PromptTaskRecoveryPatch::AppliedToTarget => record.applied_to_target = true,
            PromptTaskRecoveryPatch::ReleaseCustomPromptResult => {
                record.applied_to_target = false;
                record.result_committed = true;
            }
        }
        Ok(true)
    })
}

pub(super) fn update_pending_order_id_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    value: &str,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    if value.trim().is_empty() {
        return Err(RecoveryError::InvalidDocument.into());
    }
    NamespaceRecoveryStore { authority }.mutate_orders(|file: &mut OrderRecoveryFile| {
        let Some(index) = exact_index(&file.orders, expected) else {
            return Ok(false);
        };
        file.orders[index].order_id = value.to_owned();
        Ok(true)
    })
}

pub(super) fn update_pending_order_quote_id_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    value: &str,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    if value.trim().is_empty() {
        return Err(RecoveryError::InvalidDocument.into());
    }
    NamespaceRecoveryStore { authority }.mutate_orders(|file: &mut OrderRecoveryFile| {
        let Some(index) = exact_index(&file.orders, expected) else {
            return Ok(false);
        };
        file.orders[index].upgrade_quote_id = value.to_owned();
        Ok(true)
    })
}

pub(super) fn update_pending_prompt_optimization_job_id_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    value: &str,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    if value.trim().is_empty() || !canonical_uuid(value) {
        return Err(RecoveryError::InvalidDocument.into());
    }
    NamespaceRecoveryStore { authority }.mutate_prompt_tasks(|file: &mut PromptTaskRecoveryFile| {
        let Some(index) = exact_index(&file.deep_optimizations, expected) else {
            return Ok(false);
        };
        file.deep_optimizations[index].server_job_id = value.to_owned();
        Ok(true)
    })
}

fn delivery_index(
    record: &PendingGenerationRecord,
    predicate: impl Fn(&PendingDeliveryRecord) -> bool,
) -> Result<Option<usize>> {
    let mut matching = record
        .deliveries
        .iter()
        .enumerate()
        .filter(|(_, item)| predicate(item))
        .map(|(index, _)| index);
    let found = matching.next();
    if matching.next().is_some() {
        return Err(RecoveryError::InvalidDocument.into());
    }
    Ok(found)
}
fn save_delivery(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    delivery: &DeliveryConfirmation,
    value: &str,
    failed: bool,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    NamespaceRecoveryStore { authority }.mutate_generations(|file| {
        let Some(index) = exact_index(&file.generations, expected) else {
            return Ok(false);
        };
        let record = &mut file.generations[index];
        if let Some(index) = delivery_index(record, |item| item.file_id == delivery.file_id)? {
            if failed {
                record.deliveries[index].failed_asset_id = value.to_owned();
            } else {
                record.deliveries[index].local_path = value.to_owned();
            }
        } else {
            record.deliveries.push(PendingDeliveryRecord {
                item_index: delivery.item_index,
                file_id: delivery.file_id.clone(),
                sha256: delivery.sha256.clone(),
                size_bytes: delivery.size_bytes,
                local_path: if failed {
                    String::new()
                } else {
                    value.to_owned()
                },
                failed_asset_id: if failed {
                    value.to_owned()
                } else {
                    String::new()
                },
                acknowledged: false,
                abandoned: false,
            });
        }
        Ok(true)
    })
}
pub(super) fn pending_delivery_saved_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    delivery: &DeliveryConfirmation,
    local_path: &str,
) -> Result<bool> {
    save_delivery(authority, expected, delivery, local_path, false)
}
pub(super) fn pending_delivery_failed_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    delivery: &DeliveryConfirmation,
    failed_asset_id: &str,
) -> Result<bool> {
    save_delivery(authority, expected, delivery, failed_asset_id, true)
}
fn settle_delivery(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    value: &str,
    abandon: bool,
) -> Result<bool> {
    expected.require_authority(authority, true)?;
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Remove(RowKind::Generation, expected.clone()),
        |file: &mut RecoveryFile| {
            let Some(index) = exact_index(&file.generations, expected) else {
                return Ok(false);
            };
            let record = &mut file.generations[index];
            if value.trim().is_empty() {
                return Ok(false);
            }
            let Some(item_index) = delivery_index(record, |item| {
                if abandon {
                    item.failed_asset_id == value
                } else {
                    item.file_id == value
                }
            })?
            else {
                return Ok(false);
            };
            // A duplicated file identity cannot authorize an ambiguous acknowledgement/abandon.
            let file_id = record.deliveries[item_index].file_id.clone();
            delivery_index(record, |item| item.file_id == file_id)?;
            if abandon {
                record.deliveries[item_index].abandoned = true;
            } else {
                record.deliveries[item_index].acknowledged = true;
            }
            if generation_record_complete(record) {
                file.generations.remove(index);
            }
            Ok(true)
        },
    )
}
pub(super) fn settle_acknowledged_cutout_delivery_for_namespace(
    receipt: &AcknowledgedCutoutDelivery,
) -> Result<bool> {
    let authority = receipt.authority();
    let original = receipt.record();
    let expected = original.identity();
    let confirmation = receipt.confirmation();
    expected.require_authority(authority, true)?;
    anyhow::ensure!(original.task_type == "image_cutout" && original.count == 1
        && original.reference_paths.len() == 1 && original.reference_sha256.len() == 1
        && original.reference_size_bytes.len() == 1
        && confirmation.client_request_id == original.client_request_id
        && confirmation.task_id == original.server_task_id && confirmation.item_index == 0,
        "cutout acknowledgement provenance is incomplete");
    NamespaceRecoveryStore { authority }.mutate(
        MutationPermission::Remove(RowKind::Generation, expected.clone()),
        |file: &mut RecoveryFile| {
            let Some(index) = exact_index(&file.generations, &expected) else { return Ok(false); };
            let record = &mut file.generations[index];
            let stable = |record: &PendingGenerationRecord| -> Result<serde_json::Value> {
                let mut record = record.clone();
                record.deliveries.clear(); record.terminal = false; record.expected_success_count = 0;
                for path in record.reference_paths.iter_mut().chain(record.lineage_reference_paths.iter_mut()) {
                    authority.lease().namespace.remap_path(path);
                }
                Ok(serde_json::to_value(record)?)
            };
            anyhow::ensure!(stable(record)? == stable(original)?
                && record.terminal && record.expected_success_count == 1 && record.deliveries.len() == 1,
                "cutout acknowledgement retained body or terminal state changed");
            let Some(item) = delivery_index(record, |item| item.file_id == confirmation.file_id
                || item.item_index == confirmation.item_index)? else { return Ok(false); };
            let delivery = &record.deliveries[item];
            anyhow::ensure!(delivery.file_id == confirmation.file_id
                && delivery.item_index == confirmation.item_index
                && delivery.sha256 == confirmation.sha256
                && delivery.size_bytes == confirmation.size_bytes
                && delivery.local_path == receipt.remote_path() && !delivery.abandoned
                && !delivery.acknowledged
                && delivery.failed_asset_id.is_empty() && confirmation.failed_asset_id.is_none(),
                "cutout acknowledgement original remote confirmation changed");
            record.deliveries[item].acknowledged = true;
            record.reference_paths.clear();
            record.reference_sha256.clear();
            record.reference_size_bytes.clear();
            if generation_record_complete(record) { file.generations.remove(index); }
            Ok(true)
        },
    )
}

pub(super) fn pending_delivery_acknowledged_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    file_id: &str,
) -> Result<bool> {
    settle_delivery(authority, expected, file_id, false)
}
pub(super) fn abandon_pending_delivery_for_namespace(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
    failed_asset_id: &str,
) -> Result<bool> {
    settle_delivery(authority, expected, failed_asset_id, true)
}
fn recoverable_delivery(
    records: &[PendingGenerationRecord],
    epoch: u64,
    failed_asset_id: &str,
) -> Option<(PendingGenerationRecord, PendingDeliveryRecord)> {
    if failed_asset_id.trim().is_empty() {
        return None;
    }
    let mut matches = records
        .iter()
        .filter(|record| record.auth_epoch == epoch)
        .flat_map(|record| {
            record
                .deliveries
                .iter()
                .filter(move |item| {
                    item.failed_asset_id == failed_asset_id && !item.acknowledged && !item.abandoned
                })
                .map(move |item| (record.clone(), item.clone()))
        });
    let result = matches.next();
    if matches.next().is_some() {
        None
    } else {
        result
    }
}
pub(super) fn recoverable_delivery_for_failed_asset_for_namespace(
    authority: &NamespaceStorageAuthority,
    failed_asset_id: &str,
) -> Result<Option<(PendingGenerationRecord, PendingDeliveryRecord)>> {
    Ok(recoverable_delivery(
        &load_pending_generations_for_namespace(authority)?,
        authority.lease().auth_epoch,
        failed_asset_id,
    ))
}
pub(super) fn recoverable_failed_asset_ids_for_namespace(
    authority: &NamespaceStorageAuthority,
) -> Result<BTreeSet<String>> {
    let records = load_pending_generations_for_namespace(authority)?;
    Ok(records
        .iter()
        .flat_map(|record| record.deliveries.iter())
        .map(|item| item.failed_asset_id.clone())
        .filter(|id| recoverable_delivery(&records, authority.lease().auth_epoch, id).is_some())
        .collect())
}
pub(super) fn pending_recovery_file_references_for_namespace(
    authority: &NamespaceStorageAuthority,
) -> Result<Vec<(String, String, String)>> {
    let generations = load_pending_generations_for_namespace(authority)?;
    let prompts = load_pending_prompt_tasks_for_namespace(authority)?;
    let mut references = Vec::new();
    for record in generations {
        for path in record
            .reference_paths
            .iter()
            .chain(record.lineage_reference_paths.iter())
        {
            references.push((
                "pending-generation".to_owned(),
                record.client_request_id.clone(),
                path.clone(),
            ));
        }
        for delivery in record.deliveries {
            if !delivery.local_path.trim().is_empty() {
                references.push((
                    "pending-delivery".to_owned(),
                    record.client_request_id.clone(),
                    delivery.local_path,
                ));
            }
        }
    }
    for record in prompts {
        for path in record.reference_paths {
            references.push((
                "pending-prompt-task".to_owned(),
                record.client_request_id.clone(),
                path,
            ));
        }
    }
    Ok(references)
}
fn generation_record_complete(record: &PendingGenerationRecord) -> bool {
    record.terminal
        && record.reference_paths.is_empty()
        && record.reference_sha256.is_empty()
        && record.reference_size_bytes.is_empty()
        && record
            .deliveries
            .iter()
            .filter(|item| item.acknowledged || item.abandoned)
            .count()
            >= record.expected_success_count
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn load_pending_orders_checked() -> Result<Vec<PendingOrderRecord>> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn upsert_pending_order(_record: PendingOrderRecord) -> Result<()> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn update_pending_order_id(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
    _order_id: &str,
) -> Result<()> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn update_pending_order_quote_id(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
    _upgrade_quote_id: &str,
) -> Result<()> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn remove_pending_order(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
) -> Result<()> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn claim_pending_order_epoch(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _new_auth_epoch: u64,
    _client_request_id: &str,
) -> Result<()> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn claim_legacy_pending_order(
    _owner_user_id: &str,
    _new_auth_epoch: u64,
    _client_request_id: &str,
    _order_id: &str,
) -> Result<()> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn load_generation_recovery_candidates_checked(
    _owner_user_id: &str,
    _auth_epoch: u64,
) -> Result<Vec<PendingGenerationRecord>> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn upsert_pending_generation_scoped(
    _record: PendingGenerationRecord,
    _owner_user_id: &str,
    _auth_epoch: u64,
) -> Result<()> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn update_pending_generation_scoped(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
    _update: impl FnOnce(&mut PendingGenerationRecord),
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn remove_pending_generation_scoped(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn rebind_pending_generation_epoch(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _new_auth_epoch: u64,
    _client_request_id: &str,
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn claim_legacy_pending_generation(
    _owner_user_id: &str,
    _new_auth_epoch: u64,
    _client_request_id: &str,
    _server_task_id: &str,
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn load_pending_prompt_tasks() -> Vec<PendingPromptTaskRecord> {
    Vec::new()
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn load_pending_prompt_tasks_checked() -> Result<Vec<PendingPromptTaskRecord>> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn upsert_pending_prompt_task(_record: PendingPromptTaskRecord) -> Result<()> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn update_pending_prompt_task_scoped(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
    _update: impl FnOnce(&mut PendingPromptTaskRecord),
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn remove_pending_prompt_task_scoped(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn pending_delivery_saved(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
    _delivery: &DeliveryConfirmation,
    _local_path: &str,
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn pending_delivery_failed(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
    _delivery: &DeliveryConfirmation,
    _failed_asset_id: &str,
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn recoverable_delivery_for_failed_asset(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _failed_asset_id: &str,
) -> Result<Option<(PendingGenerationRecord, PendingDeliveryRecord)>> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn recoverable_failed_asset_ids(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
) -> Result<BTreeSet<String>> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn abandon_pending_delivery(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _failed_asset_id: &str,
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn pending_delivery_acknowledged(
    _owner_user_id: &str,
    _expected_auth_epoch: u64,
    _client_request_id: &str,
    _file_id: &str,
) -> Result<bool> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): remove in Task 10; strings confer no namespace authority.
pub(super) fn pending_recovery_file_references() -> Result<Vec<(String, String, String)>> {
    Err(RecoveryError::NamespaceRequired.into())
}

// TEMP(team-accounts): deletion remains closed until its namespace protocol is activated.
pub(super) fn pending_recovery_may_reference_files() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_deep_dismissal_preserves_exact_historical_identity_and_rejects_upsert_rollback() {
        let (_root, authority, scope) = fixture(9);
        let mut record = deep_record();
        record.server_job_id = OTHER.into();
        upsert_pending_prompt_optimization_for_namespace(&authority, &scope, record.clone()).unwrap();
        let identity = record.identity();
        assert!(!mark_pending_prompt_optimization_presentation_dismissed_for_namespace(&authority, &identity, PAYER).unwrap());
        assert!(mark_pending_prompt_optimization_presentation_dismissed_for_namespace(&authority, &identity, OTHER).unwrap());
        assert!(mark_pending_prompt_optimization_presentation_dismissed_for_namespace(&authority, &identity, OTHER).unwrap());
        let saved = load_pending_prompt_optimizations_for_namespace(&authority).unwrap().remove(0);
        let mut expected = serde_json::to_value(&record).unwrap();
        expected["presentation_dismissed"] = serde_json::json!(true);
        assert_eq!(serde_json::to_value(&saved).unwrap(), expected);
        assert!(upsert_pending_prompt_optimization_for_namespace(&authority, &scope, record).is_err());
        assert_eq!(serde_json::to_value(load_pending_prompt_optimizations_for_namespace(&authority).unwrap().remove(0)).unwrap(), expected);
        let new_lease = NamespaceLease { auth_epoch: 10, namespace_epoch: 2, ..authority.lease().clone() };
        let current = NamespaceStorageAuthority::open(Arc::new(NamespaceFs::open_data_root(_root.path()).unwrap()), &new_lease).unwrap();
        assert!(mark_pending_prompt_optimization_presentation_dismissed_for_namespace(&current, &identity, OTHER).unwrap());
        assert_eq!(serde_json::to_value(load_pending_prompt_optimizations_for_namespace(&current).unwrap().remove(0)).unwrap(), expected);
    }
    #[test]
    fn core_deep_dismissal_refuses_missing_job_and_defaults_legacy_presentation_to_visible() {
        let (_root, authority, scope) = fixture(9);
        let record = deep_record();
        upsert_pending_prompt_optimization_for_namespace(&authority, &scope, record.clone()).unwrap();
        for id in ["", "not-a-job"] {
            assert!(mark_pending_prompt_optimization_presentation_dismissed_for_namespace(&authority, &record.identity(), id).is_err());
        }
        let mut wire = serde_json::to_value(&record).unwrap();
        wire.as_object_mut().unwrap().remove("presentation_dismissed");
        let legacy: PendingPromptOptimizationRecord = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap()["presentation_dismissed"], false);
        wire["presentation_dismissed"] = serde_json::json!(true);
        let dismissed_without_job: PendingPromptOptimizationRecord = serde_json::from_value(wire).unwrap();
        assert!(upsert_pending_prompt_optimization_for_namespace(&authority, &scope, dismissed_without_job).is_err());
        assert!(load_pending_prompt_optimizations_for_namespace(&authority).unwrap()[0].server_job_id.is_empty());
    }
    #[test]
    fn core_deep_dismissal_refuses_retired_or_upgrade_authority_without_changing_bytes() {
        use backend_generation::billing_capture_test_support::fixture;
        for retire in [false, true] {
            let fixture = fixture("http://127.0.0.1:9");
            let lease = fixture.authority.lease().clone();
            let index = FileIndex::initialize(fixture.root.path().join("dismiss-index.sqlite3")).unwrap();
            let root = fixture.context.data_root_capability.clone().unwrap();
            let authority = NamespaceStorageAuthority::open_active(root, &lease, fixture.backend.api.clone(), index).unwrap();
            let mut record = deep_record();
            record.owner_user_id = fixture.scope.request.session.owner_user_id.clone();
            record.auth_epoch = lease.auth_epoch;
            record.billing_account_group_id = fixture.scope.request.account_group_id.clone();
            record.server_job_id = OTHER.into();
            upsert_pending_prompt_optimization_for_namespace(&authority, &fixture.scope, record.clone()).unwrap();
            let before = bytes(&authority, RecoveryDocument::PromptTasks);
            if retire { fixture.context.user_activity.begin_quiesce(&lease).unwrap().retire(); }
            else { fixture.backend.api.upgrade_latch().trip(RequiredUpgrade { minimum_version: None }); }
            assert!(mark_pending_prompt_optimization_presentation_dismissed_for_namespace(&authority, &record.identity(), OTHER).is_err());
            assert_eq!(bytes(&authority, RecoveryDocument::PromptTasks), before);
        }
    }
#[test]
    fn core_runtime_recovery_mutations_reject_retired_namespace_and_exact_upgrade() {
        use backend_generation::billing_capture_test_support::fixture;
        for retire in [false, true] {
            let fixture = fixture("http://127.0.0.1:9");
            let lease = fixture.authority.lease().clone();
            let index = FileIndex::initialize(fixture.root.path().join("runtime-index.sqlite3")).unwrap();
            let root = fixture.context.data_root_capability.clone().unwrap();
            let authority = NamespaceStorageAuthority::open_active(root.clone(), &lease, fixture.backend.api.clone(), index).unwrap();
            let record = PendingOrderRecord { schema_version: 2, kind: "credit".into(), client_request_id: "exact-original-key".into(),
                owner_user_id: fixture.scope.request.session.owner_user_id.clone(), billing_account_group_id: fixture.scope.request.account_group_id.clone(),
                auth_epoch: lease.auth_epoch, order_id: String::new(), product_code: "original-pack".into(), upgrade_quote_id: String::new(), created_at: "fixture".into() };
            upsert_pending_order_for_namespace(&authority, &fixture.scope, record.clone()).unwrap();
            let before = serde_json::to_value(load_pending_orders_for_namespace(&authority).unwrap()).unwrap();
            if retire {
                fixture.context.user_activity.begin_quiesce(&lease).unwrap().retire();
            } else {
                fixture.backend.api.upgrade_latch().trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
            }
            assert!(update_pending_order_id_for_namespace(&authority, &record.identity(), "not-authorized").is_err());
            assert!(remove_pending_order_for_namespace(&authority, &record.identity()).is_err());
            assert_eq!(serde_json::to_value(load_pending_orders_for_namespace(&authority).unwrap()).unwrap(), before);
            let preparation = NamespaceStorageAuthority::open_prepublication(root, &lease).unwrap();
            assert!(preparation.begin_ordinary_mutation().is_err());
        }
    }
    const OWNER: &str = "11111111-1111-4111-8111-111111111111";
    const PAYER: &str = "22222222-2222-4222-8222-222222222222";
    const OTHER: &str = "33333333-3333-4333-8333-333333333333";
    #[test]
    fn core_cancelled_generation_is_durable_and_cannot_be_admitted_or_replayed() {
        let (_root, authority, scope) = fixture(9);
        let mut record = pending_record();
        record.server_task_id.clear();
        upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
        let captured = SavedReplayRequest::generation(authority.clone(), &scope.request.session, &record.client_request_id).unwrap();
        assert!(apply_generation_patch_for_namespace(&authority, &record.identity(), GenerationRecoveryPatch::RequestCancellation).unwrap());
        assert!(apply_generation_patch_for_namespace(&authority, &record.identity(), GenerationRecoveryPatch::BeginSubmission).is_err());
        assert!(SavedReplayRequest::generation(authority.clone(), &scope.request.session, &record.client_request_id).is_err());
        assert!(captured.verify().is_err());
        let saved = load_pending_generations_for_namespace(&authority).unwrap();
        assert!(serde_json::to_value(&saved[0]).unwrap()["cancel_requested"].as_bool().unwrap());
        assert_eq!(saved[0].billing_account_group_id, PAYER);
        assert_eq!(saved[0].client_request_id, "request_123");
        assert_eq!(saved[0].generation_prompt, "prompt");
    }
    #[test]
    fn core_saved_replay_binds_original_record_without_selected_billing_epoch() {
        let (_root, authority, scope) = fixture(9);
        let mut record = pending_record();
        record.server_task_id.clear(); record.terminal = false; record.deliveries.clear();
        upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
        let replay = SavedReplayRequest::generation(authority.clone(), &scope.request.session, &record.client_request_id).unwrap();
        assert_eq!(replay.payer(), PAYER);
        assert_eq!(replay.key(), "request_123");
        assert_eq!(replay.path(), "/v1/generation/tasks");
        assert_eq!(replay.body(), serde_json::json!({"client_request_id":"request_123","task_type":"image_generation","model_code":"openai_image","prompt":"prompt","quality":"1K","count":1,"aspect_ratio":"1:1","reference_file_ids":[]}));
        assert!(replay.verify().is_ok());
        assert!(SavedReplayRequest::generation(authority.clone(), &SessionScope { owner_user_id: OTHER.into(), auth_epoch: 9 }, &record.client_request_id).is_err());
        assert!(SavedReplayRequest::generation(authority.clone(), &scope.request.session, "other-key").is_err());
        record.generation_prompt = "changed body".into();
        upsert_pending_generation_for_namespace(&authority, &scope, record).unwrap();
        assert!(replay.verify().is_err());
    }

    fn core_retained_video_row() -> (PendingGenerationRecord, serde_json::Value) {
        let request = serde_json::json!({"client_request_id":"request_123","task_type":"image_to_video",
            "model_code":"video-original","prompt":"original video body","source_file_id":"original-file","reference_file_ids":["original-file"],
            "aspect_ratio":"16:9","resolution":"720P","duration_secs":8,"quote_id":"original-quote"});
        let mut row = serde_json::to_value(pending_record()).unwrap();
        row["task_type"] = serde_json::json!("image_to_video");
        row["server_task_id"] = serde_json::json!("");
        row["video_request"] = request.clone();
        row["terminal"] = serde_json::json!(false);
        row["deliveries"] = serde_json::json!([]);
        (serde_json::from_value(row).unwrap(), request)
    }
    #[test]
    fn core_video_saved_replay_uses_complete_original_quote_body_and_payer() {
        let (_root, authority, scope) = fixture(9);
        let (record, request) = core_retained_video_row();
        upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
        let replay = SavedReplayRequest::generation(authority.clone(), &scope.request.session, &record.client_request_id).unwrap();
        assert_eq!(replay.body(), request);
        assert_eq!(replay.payer(), PAYER);
        assert_eq!(replay.key(), "request_123");
        assert_eq!(replay.path(), "/v1/generation/tasks");
        assert!(replay.verify().is_ok());
        let saved = load_pending_generations_for_namespace(&authority).unwrap();
        assert_eq!(serde_json::to_value(&saved[0]).unwrap()["video_request"], request);
        assert!(SavedReplayRequest::generation(authority.clone(), &SessionScope { owner_user_id: OTHER.into(), auth_epoch: 9 }, "request_123").is_err());
    }
    #[test]
    fn core_video_retained_source_association_survives_and_cannot_be_changed_by_same_key(){
        let (_root,authority,scope)=fixture(9);
        let (record,_)=core_retained_video_row();
        let mut value=serde_json::to_value(record).unwrap();value["source_asset_id"]=serde_json::json!("source-A");
        let record:PendingGenerationRecord=serde_json::from_value(value).unwrap();
        upsert_pending_generation_for_namespace(&authority,&scope,record.clone()).unwrap();
        let rows=load_pending_generations_for_namespace(&authority).unwrap();
        assert_eq!(serde_json::to_value(&rows[0]).unwrap()["source_asset_id"],"source-A");
        let before=bytes(&authority,RecoveryDocument::Generations);
        let mut changed=serde_json::to_value(&rows[0]).unwrap();changed["source_asset_id"]=serde_json::json!("source-B");
        assert!(upsert_pending_generation_for_namespace(&authority,&scope,serde_json::from_value(changed.clone()).unwrap()).is_err());
        assert!(NamespaceRecoveryStore{authority:&authority}.mutate_generations(|file|{
            file.generations[0]=serde_json::from_value(changed.clone())?;Ok(())
        }).is_err());
        assert_eq!(bytes(&authority,RecoveryDocument::Generations),before);
    }
    #[test]
    fn core_video_legacy_empty_source_association_is_not_adopted_by_current_image(){
        let (_root,authority,scope)=fixture(9);
        let (record,_)=core_retained_video_row();
        let mut value=serde_json::to_value(record).unwrap();value.as_object_mut().unwrap().remove("source_asset_id");
        upsert_pending_generation_for_namespace(&authority,&scope,serde_json::from_value(value).unwrap()).unwrap();
        let row=load_pending_generations_for_namespace(&authority).unwrap().remove(0);
        assert_eq!(serde_json::to_value(&row).unwrap()["source_asset_id"],"");
        let before=bytes(&authority,RecoveryDocument::Generations);
        let mut changed=serde_json::to_value(row).unwrap();changed["source_asset_id"]=serde_json::json!("currently-open-image");
        assert!(upsert_pending_generation_for_namespace(&authority,&scope,serde_json::from_value(changed).unwrap()).is_err());
        assert_eq!(bytes(&authority,RecoveryDocument::Generations),before);
    }
    #[test]
    fn core_video_retained_body_is_immutable_through_upsert_and_generic_mutation() {
        let (_root, authority, scope) = fixture(9);
        let (record, _) = core_retained_video_row();
        upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
        let before = bytes(&authority, RecoveryDocument::Generations);
        for replacement in [serde_json::Value::Null, {
            let mut value = serde_json::to_value(&record).unwrap()["video_request"].clone();
            value["quote_id"] = serde_json::json!("replacement-quote"); value
        }] {
            let mut changed = serde_json::to_value(&record).unwrap();
            changed["video_request"] = replacement.clone();
            assert!(upsert_pending_generation_for_namespace(&authority, &scope, serde_json::from_value(changed).unwrap()).is_err());
            assert_eq!(bytes(&authority, RecoveryDocument::Generations), before);
            assert!(NamespaceRecoveryStore { authority: &authority }.mutate_generations(|file| {
                let mut changed = serde_json::to_value(&file.generations[0])?;
                changed["video_request"] = replacement.clone();
                file.generations[0] = serde_json::from_value(changed)?;
                Ok(())
            }).is_err());
            assert_eq!(bytes(&authority, RecoveryDocument::Generations), before);
        }
    }
    #[test]
    fn core_video_cannot_replace_an_existing_image_operation_at_the_same_retained_key() {
        let (_root, authority, scope) = fixture(9);
        let mut image = pending_record();
        image.server_task_id.clear(); image.terminal=false; image.deliveries.clear();
        upsert_pending_generation_for_namespace(&authority,&scope,image).unwrap();
        let before=bytes(&authority,RecoveryDocument::Generations);
        let (video,_) = core_retained_video_row();
        assert!(upsert_pending_generation_for_namespace(&authority,&scope,video.clone()).is_err());
        assert_eq!(bytes(&authority,RecoveryDocument::Generations),before);
        assert!(NamespaceRecoveryStore { authority:&authority }.mutate_generations(|file| {
            file.generations[0]=video.clone();
            Ok(())
        }).is_err());
        assert_eq!(bytes(&authority,RecoveryDocument::Generations),before);
    }
    #[test]
    fn core_video_request_validation_refuses_changed_key_type_and_empty_original_quote() {
        for (field, value) in [("client_request_id","other-key"), ("task_type","image_generation"), ("quote_id","")] {
            let (_root, authority, scope) = fixture(9);
            let (record, request) = core_retained_video_row();
            let mut row = serde_json::to_value(record).unwrap();
            row["video_request"] = request;
            row["video_request"][field] = serde_json::json!(value);
            assert!(upsert_pending_generation_for_namespace(&authority, &scope, serde_json::from_value(row).unwrap()).is_err(), "{field}");
            assert!(load_pending_generations_for_namespace(&authority).unwrap().is_empty());
        }
    }
    #[test]
    fn core_video_missing_original_request_remains_blocked_and_old_images_stay_readable() {
        let (_root, authority, scope) = fixture(9);
        let mut legacy = serde_json::to_value(pending_record()).unwrap();
        legacy.as_object_mut().unwrap().remove("video_request");
        legacy["server_task_id"] = serde_json::json!("");
        let image: PendingGenerationRecord = serde_json::from_value(legacy.clone()).unwrap();
        upsert_pending_generation_for_namespace(&authority, &scope, image).unwrap();
        assert!(SavedReplayRequest::generation(authority.clone(), &scope.request.session, "request_123").is_ok());
        legacy["task_type"] = serde_json::json!("image_to_video");
        put(&authority, RecoveryDocument::Generations, &serde_json::to_vec(&serde_json::json!({"schema_version":2,"generations":[legacy]})).unwrap());
        let before = bytes(&authority, RecoveryDocument::Generations);
        assert_eq!(load_pending_generations_for_namespace(&authority).unwrap().len(), 1);
        assert!(SavedReplayRequest::generation(authority.clone(), &scope.request.session, "request_123").is_err());
        assert_eq!(bytes(&authority, RecoveryDocument::Generations), before);
    }
    #[test]
    fn core_toolbox_saved_body_dispatch_preserves_original_uploaded_source_and_payer() {
        for (kind, path) in [("image_watermark_removal","/v1/toolbox/watermark-removals"), ("image_colorization","/v1/toolbox/image-colorizations")] {
            let (_root, authority, scope) = fixture(9);
            let mut record = pending_record();
            record.task_type = kind.into(); record.server_task_id.clear();
            record.uploaded_file_ids = vec!["original-source-file".into()];
            upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
            let replay = SavedReplayRequest::generation(authority.clone(), &scope.request.session, &record.client_request_id).unwrap();
            assert_eq!(replay.path(), path);
            assert_eq!(replay.body(), serde_json::json!({"client_request_id":"request_123","reference_file_id":"original-source-file"}));
            assert_eq!(replay.payer(), PAYER);
        }
    }

    #[test]
    fn core_saved_video_and_four_toolbox_http_replay_keeps_original_path_key_body_and_payer_after_selection() {
        use backend_generation::billing_capture_test_support::{fixture as runtime_fixture, listener, capture_response, assert_capture};
        for (kind, path, extra) in [
            ("image_watermark_removal", "/v1/toolbox/watermark-removals", None),
            ("image_colorization", "/v1/toolbox/image-colorizations", None),
            ("image_enhancement", "/v1/toolbox/image-enhancements", Some(("target_quality", "2K"))),
            ("image_cutout", "/v1/toolbox/image-cutouts", Some(("subject_type", "general"))),
            ("image_to_video", "/v1/generation/tasks", None),
        ] {
            let (listener, url) = listener();
            let fixture = runtime_fixture(&url);
            let mut record = if kind == "image_to_video" { core_retained_video_row().0 } else { pending_record() };
            record.task_type = kind.into(); record.server_task_id.clear();
            record.auth_epoch = fixture.scope.request.session.auth_epoch;
            record.uploaded_file_ids = vec!["original-source-file".into()];
            if let Some((_, value)) = extra { record.quality = value.into(); }
            upsert_pending_generation_for_namespace(&fixture.authority, &fixture.scope, record.clone()).unwrap();
            let replay = SavedReplayRequest::generation(fixture.authority.clone(), &fixture.scope.request.session, &record.client_request_id).unwrap();
            let manager = &fixture.context.billing_context;
            manager.bind_authenticated_session(fixture.scope.request.session.clone()).unwrap();
            for group in [PAYER, OTHER] {
                let snapshot: AccountSnapshot = serde_json::from_value(serde_json::json!({
                    "user":{"id":OWNER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
                    "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},"credits":null,"quota":null,
                    "billing_group":{"group_id":group,"name":"selected","group_status":"active","role":"owner","member_id":null,
                        "relationship_status":null,"readable_context":true,"selectable":true,"group_version":"1","membership_version":null,"capabilities":["bill"],"quota":null}
                })).unwrap();
                let ticket = manager.begin_switch(&fixture.scope.request.session, "fixture-device", group, PreviousBillingAuthority::StillValid).unwrap();
                let staged = manager.stage_confirmation(&ticket, snapshot.billing_group.clone(), snapshot).unwrap();
                manager.publish_persisted(ticket, staged);
            }
            let before = bytes(&fixture.authority, RecoveryDocument::Generations);
            let (release, worker) = capture_response(listener, fixture.authority.clone(), "pending-generations.json", "403 Forbidden",
                r#"{"request_id":"retained-admission","data":null,"error":{"code":"account_group_not_selectable","message":"original payer denied","details":null},"meta":null}"#);
            release.send(()).unwrap();
            let result = fixture.backend.api.replay_saved::<serde_json::Value>(&replay);
            let captured = worker.join().unwrap();
            assert!(result.is_err());
            assert_capture(&captured, "generations");
            assert!(captured.request.starts_with(&format!("POST {path} ")));
            let header = captured.request.lines().find_map(|line| line.split_once(':').filter(|(name,_)| name.eq_ignore_ascii_case("idempotency-key")).map(|(_,value)|value.trim()));
            assert_eq!(header, Some("request_123"));
            let actual: serde_json::Value = serde_json::from_str(captured.request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            let mut expected = if kind == "image_to_video" { core_retained_video_row().1 }
                else { serde_json::json!({"client_request_id":"request_123","reference_file_id":"original-source-file"}) };
            if let Some((key, value)) = extra { expected[key] = serde_json::json!(value); }
            assert_eq!(actual, expected);
            assert_eq!(bytes(&fixture.authority, RecoveryDocument::Generations), before);
            assert_eq!(manager.confirmed_scope().unwrap().request.account_group_id, OTHER);
        }
    }
    fn pending_record() -> PendingGenerationRecord {
        PendingGenerationRecord {
            source_asset_id: String::new(),            video_request: None,
            schema_version: 2,
            cancel_requested: false,
            created_at_epoch_ms: Local::now().timestamp_millis(),
            client_request_id: "request_123".to_string(),
            owner_user_id: OWNER.to_owned(),
            billing_account_group_id: PAYER.to_owned(),
            auth_epoch: 9,
            local_task_id: "local".to_string(),
            server_task_id: "server".to_string(),
            raw_prompt: "prompt".to_string(),
            generation_prompt: "prompt".to_string(),
            task_type: "image_generation".to_string(),
            category: "character".to_string(),
            mode: "game".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model_code: "openai_image".to_string(),
            conversation_id: "conversation".to_string(),
            count: 1,
            target_width: 0,
            target_height: 0,
            create_conversation: true,
            reference_paths: vec![],
            reference_sha256: vec![],
            reference_size_bytes: vec![],
            lineage_reference_paths: vec![],
            uploaded_file_ids: vec![],
            deliveries: vec![PendingDeliveryRecord {
                acknowledged: false,
                ..PendingDeliveryRecord::default()
            }],
            terminal: true,
            expected_success_count: 1,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }
    fn pending_order_record(owner_user_id: &str, auth_epoch: u64) -> PendingOrderRecord {
        PendingOrderRecord {
            schema_version: 2,
            kind: "membership_upgrade".to_string(),
            client_request_id: "payment-request".to_string(),
            owner_user_id: owner_user_id.to_string(),
            billing_account_group_id: PAYER.to_owned(),
            auth_epoch,
            order_id: String::new(),
            product_code: "pro-yearly".to_string(),
            upgrade_quote_id: String::new(),
            created_at: "2026-08-10T00:00:00+08:00".to_string(),
        }
    }
    fn pending_prompt_record() -> PendingPromptTaskRecord {
        PendingPromptTaskRecord {
            schema_version: 2,
            created_at_epoch_ms: 1,
            client_request_id: "prompt-request".to_string(),
            cancel_requested: false,
            submission_started: false,
            owner_user_id: OWNER.to_owned(),
            billing_account_group_id: PAYER.to_owned(),
            auth_epoch: 9,
            server_task_id: String::new(),
            task_type: "prompt_optimize".to_string(),
            model_code: "prompt-model".to_string(),
            prompt: "draft".to_string(),
            target_language: None,
            optimize: true,
            target_kind: "composer".to_string(),
            target_id: String::new(),
            target_category: "character".to_string(),
            target_input: "draft".to_string(),
            append_result: false,
            activity_kind: "optimize".to_string(),
            reference_paths: Vec::new(),
            reference_sha256: Vec::new(),
            reference_size_bytes: Vec::new(),
            uploaded_file_ids: Vec::new(),
            result_prompt: String::new(),
            terminal_error: String::new(),
            applied_to_target: false,
            result_committed: false,
        }
    }

    #[test]
    fn recovery_v2_requires_saved_owner_and_payer() {
        let value = serde_json::to_value(pending_order_record(
            "11111111-1111-4111-8111-111111111111",
            9,
        ))
        .unwrap();
        for field in ["owner_user_id", "billing_account_group_id"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<PendingOrderRecord>(missing).is_err(),
                "missing {field} must be rejected"
            );
        }
    }

    #[test]
    fn recovery_v2_requires_complete_envelopes() {
        assert!(serde_json::from_str::<RecoveryFile>("{}").is_err());
        assert!(serde_json::from_str::<OrderRecoveryFile>(r#"{"schema_version":2}"#).is_err());
        assert!(serde_json::from_str::<PromptTaskRecoveryFile>(
            r#"{"schema_version":2,"prompt_tasks":[]}"#
        )
        .is_err());
    }

    #[test]
    fn recovery_v2_legacy_document_is_preserved() {
        let directory =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let owner = "11111111-1111-4111-8111-111111111111";
        let lease = NamespaceLease {
            namespace: UserNamespace::new(directory.path(), owner).unwrap(),
            auth_epoch: 9,
            namespace_epoch: 1,
        };
        let authority = NamespaceStorageAuthority::open(
            Arc::new(NamespaceFs::open_data_root(directory.path()).unwrap()),
            &lease,
        )
        .unwrap();
        let key =
            ManagedFileKey::new(ManagedUserArea::Recovery, "pending-generations.json").unwrap();
        let mut file = authority.create_new_regular(&key).unwrap();
        let bytes = br#"{"schema_version":1,"generations":[]}"#;
        authority
            .write_new_regular_from(&mut file, &mut &bytes[..])
            .unwrap();
        authority.sync_regular(&mut file).unwrap();
        let path = directory
            .path()
            .join("accounts")
            .join(owner)
            .join("recovery/pending-generations.json");
        let invoked = std::cell::Cell::new(false);
        let result = NamespaceRecoveryStore {
            authority: &authority,
        }
        .mutate_generations(|_| {
            invoked.set(true);
            Ok(())
        });
        assert!(
            result.is_err(),
            "legacy documents cannot authorize mutation"
        );
        assert!(!invoked.get());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn recovery_v2_requires_captured_payer() {
        let mut value = serde_json::to_value(pending_order_record(
            "11111111-1111-4111-8111-111111111111",
            9,
        ))
        .unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("billing_account_group_id");
        assert!(
            serde_json::from_value::<PendingOrderRecord>(value).is_err(),
            "a row without a captured payer must be rejected"
        );
    }

    #[test]
    fn legacy_delivery_defaults_to_non_recoverable_and_not_abandoned() {
        let delivery: PendingDeliveryRecord = serde_json::from_value(serde_json::json!({
            "item_index": 0,
            "file_id": "file-1",
            "sha256": "abc",
            "size_bytes": 3,
            "local_path": "",
            "acknowledged": false
        }))
        .unwrap();
        assert!(delivery.failed_asset_id.is_empty());
        assert!(!delivery.abandoned);
    }

    #[test]
    fn terminal_record_is_complete_only_after_every_success_is_acknowledged() {
        let mut record = pending_record();
        assert!(!generation_record_complete(&record));
        record.deliveries[0].acknowledged = true;
        assert!(generation_record_complete(&record));
    }

    #[test]
    fn partial_success_recovery_waits_for_every_delivery_ack() {
        let mut record = pending_record();
        record.count = 4;
        record.expected_success_count = 2;
        record.deliveries = vec![
            PendingDeliveryRecord {
                file_id: "file-1".to_string(),
                acknowledged: true,
                ..PendingDeliveryRecord::default()
            },
            PendingDeliveryRecord {
                file_id: "file-2".to_string(),
                acknowledged: false,
                ..PendingDeliveryRecord::default()
            },
        ];
        assert!(!generation_record_complete(&record));
        record.deliveries[1].acknowledged = true;
        assert!(generation_record_complete(&record));
    }

    fn fixture(
        epoch: u64,
    ) -> (
        tempfile::TempDir,
        Arc<NamespaceStorageAuthority>,
        BillingScope,
    ) {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let lease = NamespaceLease {
            namespace: UserNamespace::new(root.path(), OWNER).unwrap(),
            auth_epoch: epoch,
            namespace_epoch: 1,
        };
        let authority = Arc::new(
            NamespaceStorageAuthority::open(
                Arc::new(NamespaceFs::open_data_root(root.path()).unwrap()),
                &lease,
            )
            .unwrap(),
        );
        let scope = BillingScope {
            request: GroupRequestScope {
                session: SessionScope {
                    owner_user_id: OWNER.into(),
                    auth_epoch: epoch,
                },
                account_group_id: PAYER.into(),
            },
            context_epoch: 23,
        };
        (root, authority, scope)
    }
    fn put(authority: &NamespaceStorageAuthority, document: RecoveryDocument, bytes: &[u8]) {
        let key = document.key().unwrap();
        let existing = authority.open_optional_regular(&key).unwrap();
        let mut temporary = authority.create_temporary_regular_for(&key).unwrap();
        authority
            .write_new_regular_from(&mut temporary, &mut &bytes[..])
            .unwrap();
        authority.sync_regular(&mut temporary).unwrap();
        authority
            .publish_regular(
                &mut temporary,
                match existing.as_ref() {
                    Some(file) => NamespaceManagedPublication::Replace(file),
                    None => NamespaceManagedPublication::Absent(&key),
                },
            )
            .unwrap();
    }
    fn bytes(authority: &NamespaceStorageAuthority, document: RecoveryDocument) -> Vec<u8> {
        let mut file = authority
            .open_existing_regular(&document.key().unwrap())
            .unwrap();
        let mut bytes = Vec::new();
        authority.read_regular_to(&mut file, &mut bytes).unwrap();
        bytes
    }
    fn deep_record() -> PendingPromptOptimizationRecord {
        PendingPromptOptimizationRecord {
            schema_version: 2,
            client_request_id: "deep-request".into(),
            owner_user_id: OWNER.into(),
            auth_epoch: 9,
            billing_account_group_id: PAYER.into(),
            server_job_id: String::new(),
            presentation_dismissed: false,
            operation: PendingPromptOptimizationOperation::Create {
                request: CreatePromptOptimization {
                    client_request_id: "deep-request".into(),
                    prompt: "draft".into(),
                    run_mode: "manual".into(),
                    focus_mode: "balanced".into(),
                    max_rounds: 3,
                    target_score: 90,
                },
            },
        }
    }
    fn load_document(
        authority: &NamespaceStorageAuthority,
        document: RecoveryDocument,
    ) -> Result<()> {
        match document {
            RecoveryDocument::Generations => {
                load_pending_generations_for_namespace(authority).map(|_| ())
            }
            RecoveryDocument::PromptTasks => {
                load_pending_prompt_tasks_for_namespace(authority).map(|_| ())
            }
            RecoveryDocument::Orders => load_pending_orders_for_namespace(authority).map(|_| ()),
        }
    }
    #[test]
    fn all_documents_require_canonical_owned_identities_and_complete_schema() {
        let (_root, authority, _scope) = fixture(9);
        let cases = [
            (
                RecoveryDocument::Generations,
                serde_json::json!({"schema_version":2,"generations":[pending_record()]}),
                "generations",
            ),
            (
                RecoveryDocument::Orders,
                serde_json::json!({"schema_version":2,"orders":[pending_order_record(OWNER,9)]}),
                "orders",
            ),
            (
                RecoveryDocument::PromptTasks,
                serde_json::json!({"schema_version":2,"prompt_tasks":[pending_prompt_record()],"deep_optimizations":[]}),
                "prompt_tasks",
            ),
            (
                RecoveryDocument::PromptTasks,
                serde_json::json!({"schema_version":2,"prompt_tasks":[],"deep_optimizations":[deep_record()]}),
                "deep_optimizations",
            ),
        ];
        for (document, valid, vector) in cases {
            for field in [
                "schema_version",
                "owner_user_id",
                "billing_account_group_id",
                "client_request_id",
            ] {
                let mut invalid = valid.clone();
                invalid[vector][0].as_object_mut().unwrap().remove(field);
                let original = serde_json::to_vec(&invalid).unwrap();
                put(&authority, document, &original);
                let error = load_document(&authority, document).unwrap_err();
                assert_eq!(
                    error.downcast_ref::<RecoveryError>(),
                    Some(&RecoveryError::InvalidDocument),
                    "missing {field}"
                );
                assert_eq!(bytes(&authority, document), original);
            }
            for (field, value) in [
                ("schema_version", serde_json::json!(1)),
                ("owner_user_id", serde_json::json!(OTHER)),
                (
                    "owner_user_id",
                    serde_json::json!("11111111111141118111111111111111"),
                ),
                (
                    "billing_account_group_id",
                    serde_json::json!(" 22222222-2222-4222-8222-222222222222"),
                ),
                ("billing_account_group_id", serde_json::json!("not-a-payer")),
                ("client_request_id", serde_json::json!("")),
            ] {
                let mut invalid = valid.clone();
                invalid[vector][0][field] = value;
                let original = serde_json::to_vec(&invalid).unwrap();
                put(&authority, document, &original);
                assert!(load_document(&authority, document).is_err(), "{field}");
                assert_eq!(bytes(&authority, document), original);
            }
            for field in ["schema_version", vector] {
                let mut invalid = valid.clone();
                invalid.as_object_mut().unwrap().remove(field);
                put(&authority, document, &serde_json::to_vec(&invalid).unwrap());
                assert!(load_document(&authority, document).is_err());
            }
            let mut duplicate = valid.clone();
            let row = duplicate[vector][0].clone();
            duplicate[vector].as_array_mut().unwrap().push(row);
            put(
                &authority,
                document,
                &serde_json::to_vec(&duplicate).unwrap(),
            );
            assert!(load_document(&authority, document).is_err());
        }
    }
    #[test]
    fn namespace_upserts_capture_all_four_rows_and_reject_wrong_billing_scope() {
        let (_root, authority, scope) = fixture(9);
        upsert_pending_generation_for_namespace(&authority, &scope, pending_record()).unwrap();
        upsert_pending_prompt_task_for_namespace(&authority, &scope, pending_prompt_record())
            .unwrap();
        upsert_pending_order_for_namespace(&authority, &scope, pending_order_record(OWNER, 9))
            .unwrap();
        upsert_pending_prompt_optimization_for_namespace(&authority, &scope, deep_record())
            .unwrap();
        assert_eq!(
            load_pending_generations_for_namespace(&authority).unwrap()[0].billing_account_group_id,
            PAYER
        );
        assert_eq!(
            load_pending_prompt_tasks_for_namespace(&authority).unwrap()[0]
                .billing_account_group_id,
            PAYER
        );
        assert_eq!(
            load_pending_orders_for_namespace(&authority).unwrap()[0].billing_account_group_id,
            PAYER
        );
        assert_eq!(
            load_pending_prompt_optimizations_for_namespace(&authority).unwrap()[0]
                .billing_account_group_id,
            PAYER
        );
        let original = bytes(&authority, RecoveryDocument::Generations);
        for mutation in 0..3 {
            let mut wrong = scope.clone();
            match mutation {
                0 => wrong.request.account_group_id = OTHER.into(),
                1 => wrong.request.session.owner_user_id = OTHER.into(),
                _ => wrong.request.session.auth_epoch = 10,
            }
            assert!(
                upsert_pending_generation_for_namespace(&authority, &wrong, pending_record())
                    .is_err()
            );
            assert!(upsert_pending_prompt_task_for_namespace(
                &authority,
                &wrong,
                pending_prompt_record()
            )
            .is_err());
            assert!(upsert_pending_order_for_namespace(
                &authority,
                &wrong,
                pending_order_record(OWNER, 9)
            )
            .is_err());
            assert!(upsert_pending_prompt_optimization_for_namespace(
                &authority,
                &wrong,
                deep_record()
            )
            .is_err());
        }
        assert_eq!(bytes(&authority, RecoveryDocument::Generations), original);
    }
    #[test]
    fn ordinary_callbacks_cannot_change_identity_presence_or_order() {
        let (_root, authority, scope) = fixture(9);
        let first = pending_record();
        upsert_pending_generation_for_namespace(&authority, &scope, first.clone()).unwrap();
        let mut second = first.clone();
        second.client_request_id = "other-request".into();
        upsert_pending_generation_for_namespace(&authority, &scope, second.clone()).unwrap();
        let original = bytes(&authority, RecoveryDocument::Generations);
        for mutation in 0..7 {
            let result = NamespaceRecoveryStore {
                authority: &authority,
            }
            .mutate_generations(|file| {
                match mutation {
                    0 => file.generations[0].owner_user_id = OTHER.into(),
                    1 => file.generations[0].billing_account_group_id = OTHER.into(),
                    2 => file.generations[0].client_request_id = "changed-request".into(),
                    3 => file.generations[0].auth_epoch = 10,
                    4 => {
                        file.generations.remove(0);
                    }
                    5 => {
                        let mut row = file.generations.remove(0);
                        row.billing_account_group_id = OTHER.into();
                        file.generations.insert(0, row);
                    }
                    _ => file.generations.swap(0, 1),
                }
                Ok(())
            });
            assert!(result.is_err(), "mutation {mutation}");
            assert_eq!(bytes(&authority, RecoveryDocument::Generations), original);
        }
        let mut changed_payer = first;
        changed_payer.billing_account_group_id = OTHER.into();
        let mut changed_scope = scope;
        changed_scope.request.account_group_id = OTHER.into();
        assert_eq!(
            upsert_pending_generation_for_namespace(&authority, &changed_scope, changed_payer)
                .unwrap_err()
                .downcast_ref::<RecoveryError>(),
            Some(&RecoveryError::IdentityChanged)
        );
        assert_eq!(bytes(&authority, RecoveryDocument::Generations), original);
    }
    #[test]
    fn explicit_epoch_rebinding_preserves_payer_and_rejects_stale_mutation() {
        let (_root, authority, _scope) = fixture(10);
        let generation = pending_record();
        let prompt = pending_prompt_record();
        let order = pending_order_record(OWNER, 9);
        let deep = deep_record();
        put(
            &authority,
            RecoveryDocument::Generations,
            &serde_json::to_vec(
                &serde_json::json!({"schema_version":2,"generations":[generation]}),
            )
            .unwrap(),
        );
        put(
            &authority,
            RecoveryDocument::Orders,
            &serde_json::to_vec(&serde_json::json!({"schema_version":2,"orders":[order]})).unwrap(),
        );
        put(&authority, RecoveryDocument::PromptTasks, &serde_json::to_vec(&serde_json::json!({"schema_version":2,"prompt_tasks":[prompt],"deep_optimizations":[deep]})).unwrap());
        let generation = load_pending_generations_for_namespace(&authority)
            .unwrap()
            .remove(0);
        let prompt = load_pending_prompt_tasks_for_namespace(&authority)
            .unwrap()
            .remove(0);
        let order = load_pending_orders_for_namespace(&authority)
            .unwrap()
            .remove(0);
        let deep = load_pending_prompt_optimizations_for_namespace(&authority)
            .unwrap()
            .remove(0);
        assert!(apply_generation_patch_for_namespace(
            &authority,
            &generation.identity(),
            GenerationRecoveryPatch::Terminal {
                expected_success_count: 0
            }
        )
        .is_err());
        assert!(rebind_pending_generation_epoch_for_namespace(
            &authority,
            &generation.identity(),
            10
        )
        .unwrap());
        assert!(
            rebind_pending_prompt_task_epoch_for_namespace(&authority, &prompt.identity(), 10)
                .unwrap()
        );
        assert!(
            rebind_pending_order_epoch_for_namespace(&authority, &order.identity(), 10).unwrap()
        );
        assert!(rebind_pending_prompt_optimization_epoch_for_namespace(
            &authority,
            &deep.identity(),
            10
        )
        .unwrap());
        assert!(!rebind_pending_generation_epoch_for_namespace(
            &authority,
            &generation.identity(),
            10
        )
        .unwrap());
        assert!(
            rebind_pending_order_epoch_for_namespace(&authority, &order.identity(), 11).is_err()
        );
        let generation = load_pending_generations_for_namespace(&authority)
            .unwrap()
            .remove(0);
        assert_eq!(generation.auth_epoch, 10);
        assert_eq!(generation.billing_account_group_id, PAYER);
        assert!(
            remove_pending_generation_for_namespace(&authority, &generation.identity()).unwrap()
        );
        assert!(remove_pending_prompt_task_for_namespace(
            &authority,
            &load_pending_prompt_tasks_for_namespace(&authority).unwrap()[0].identity()
        )
        .unwrap());
        assert!(remove_pending_order_for_namespace(
            &authority,
            &load_pending_orders_for_namespace(&authority).unwrap()[0].identity()
        )
        .unwrap());
        assert!(remove_pending_prompt_optimization_for_namespace(
            &authority,
            &load_pending_prompt_optimizations_for_namespace(&authority).unwrap()[0].identity()
        )
        .unwrap());
        assert!(load_pending_generations_for_namespace(&authority)
            .unwrap()
            .is_empty());
        assert!(load_pending_prompt_tasks_for_namespace(&authority)
            .unwrap()
            .is_empty());
        assert!(load_pending_orders_for_namespace(&authority)
            .unwrap()
            .is_empty());
        assert!(load_pending_prompt_optimizations_for_namespace(&authority)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn deep_create_retry_and_shared_prompt_keys_are_validated() {
        let (_root, authority, scope) = fixture(9);
        let original = deep_record();
        upsert_pending_prompt_optimization_for_namespace(&authority, &scope, original.clone())
            .unwrap();
        let before = bytes(&authority, RecoveryDocument::PromptTasks);
        let mut changed = original.clone();
        if let PendingPromptOptimizationOperation::Create { request } = &mut changed.operation {
            request.prompt = "different".into();
        }
        assert!(
            upsert_pending_prompt_optimization_for_namespace(&authority, &scope, changed).is_err()
        );
        assert_eq!(bytes(&authority, RecoveryDocument::PromptTasks), before);
        assert!(update_pending_prompt_optimization_job_id_for_namespace(
            &authority,
            &original.identity(),
            OTHER
        )
        .unwrap());
        assert!(update_pending_prompt_optimization_job_id_for_namespace(
            &authority,
            &original.identity(),
            "not-a-job"
        )
        .is_err());
        for mutation in 0..5 {
            let mut deep = original.clone();
            match mutation {
                0 => {
                    if let PendingPromptOptimizationOperation::Create { request } =
                        &mut deep.operation
                    {
                        request.client_request_id = "wrong".into();
                    }
                }
                1 => {
                    deep.operation = PendingPromptOptimizationOperation::Retry {
                        source_job_id: String::new(),
                    }
                }
                2 => {
                    deep.operation = PendingPromptOptimizationOperation::Retry {
                        source_job_id: "33333333333343338333333333333333".into(),
                    }
                }
                3 => deep.server_job_id = "not-a-job".into(),
                _ => deep.client_request_id = "prompt-request".into(),
            }
            put(&authority,RecoveryDocument::PromptTasks,&serde_json::to_vec(&serde_json::json!({"schema_version":2,"prompt_tasks":[pending_prompt_record()],"deep_optimizations":[deep]})).unwrap());
            assert!(
                load_pending_prompt_tasks_for_namespace(&authority).is_err(),
                "mutation {mutation}"
            );
        }
        let mut retry = original;
        retry.operation = PendingPromptOptimizationOperation::Retry {
            source_job_id: OTHER.into(),
        };
        put(&authority,RecoveryDocument::PromptTasks,&serde_json::to_vec(&serde_json::json!({"schema_version":2,"prompt_tasks":[],"deep_optimizations":[retry]})).unwrap());
        assert_eq!(
            load_pending_prompt_optimizations_for_namespace(&authority)
                .unwrap()
                .len(),
            1
        );
        let mut unknown = serde_json::to_value(deep_record()).unwrap();
        unknown["operation"]["untrusted"] = serde_json::json!(true);
        assert!(serde_json::from_value::<PendingPromptOptimizationRecord>(unknown).is_err());
    }
    #[test]
    fn release_inputs_clears_three_vectors_and_preserves_lineage_and_terminal_row() {
        let (_root, authority, scope) = fixture(9);
        for patch in [
            GenerationRecoveryPatch::ReleaseReferenceInputs,
            GenerationRecoveryPatch::UploadedAndReleaseInputs(vec!["upload".into()]),
            GenerationRecoveryPatch::Accepted {
                server_task_id: "server".into(),
                uploaded_file_ids: vec!["upload".into()],
                clear_reference_inputs: true,
            },
        ] {
            let mut record = pending_record();
            record.reference_paths = vec!["input".into()];
            record.reference_sha256 = vec!["sha".into()];
            record.reference_size_bytes = vec![3];
            record.lineage_reference_paths = vec!["ancestor".into()];
            upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
            apply_generation_patch_for_namespace(&authority, &record.identity(), patch).unwrap();
            let saved = load_pending_generations_for_namespace(&authority)
                .unwrap()
                .remove(0);
            assert!(
                saved.reference_paths.is_empty()
                    && saved.reference_sha256.is_empty()
                    && saved.reference_size_bytes.is_empty()
            );
            assert_eq!(saved.lineage_reference_paths, vec!["ancestor"]);
            apply_generation_patch_for_namespace(
                &authority,
                &saved.identity(),
                GenerationRecoveryPatch::Terminal {
                    expected_success_count: 0,
                },
            )
            .unwrap();
            assert_eq!(
                load_pending_generations_for_namespace(&authority)
                    .unwrap()
                    .len(),
                1
            );
        }
    }
    #[test]
    fn delivery_mutations_preserve_confirmation_and_do_not_fake_acknowledgement() {
        let (_root, authority, scope) = fixture(9);
        let mut record = pending_record();
        record.deliveries.clear();
        record.expected_success_count = 2;
        upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
        let delivery = DeliveryConfirmation {
            client_request_id: record.client_request_id.clone(),
            item_index: 3,
            task_id: "server".into(),
            file_id: "file".into(),
            sha256: "sha".into(),
            size_bytes: 8,
            failed_asset_id: None,
        };
        assert!(pending_delivery_failed_for_namespace(
            &authority,
            &record.identity(),
            &delivery,
            "failed"
        )
        .unwrap());
        let mut changed = delivery.clone();
        changed.sha256 = "wrong-sha".into();
        changed.item_index = 99;
        pending_delivery_saved_for_namespace(&authority, &record.identity(), &changed, "local")
            .unwrap();
        let saved = load_pending_generations_for_namespace(&authority)
            .unwrap()
            .remove(0);
        assert_eq!(saved.deliveries[0].sha256, "sha");
        assert_eq!(saved.deliveries[0].item_index, 3);
        assert_eq!(saved.deliveries[0].failed_asset_id, "failed");
        assert!(
            abandon_pending_delivery_for_namespace(&authority, &record.identity(), "failed")
                .unwrap()
        );
        let saved = load_pending_generations_for_namespace(&authority)
            .unwrap()
            .remove(0);
        assert!(saved.deliveries[0].abandoned);
        assert!(!saved.deliveries[0].acknowledged);
        let second = DeliveryConfirmation {
            file_id: "file-2".into(),
            ..delivery
        };
        pending_delivery_saved_for_namespace(&authority, &record.identity(), &second, "second")
            .unwrap();
        assert!(!pending_delivery_acknowledged_for_namespace(
            &authority,
            &record.identity(),
            "missing"
        )
        .unwrap());
        assert!(pending_delivery_acknowledged_for_namespace(
            &authority,
            &record.identity(),
            "file-2"
        )
        .unwrap());
        assert!(load_pending_generations_for_namespace(&authority)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn namespace_delivery_settlement_retains_each_required_input_until_explicit_release() {
        for abandon in [false, true] {
            for vector in 0..3 {
                let (_root, authority, scope) = fixture(9);
                let mut record = pending_record();
                record.deliveries[0].file_id = "settled-file".into();
                record.deliveries[0].failed_asset_id = "failed-card".into();
                record.lineage_reference_paths = vec!["lineage".into()];
                record.uploaded_file_ids = vec!["uploaded".into()];
                match vector {
                    0 => record.reference_paths = vec!["required-input".into()],
                    1 => record.reference_sha256 = vec!["required-hash".into()],
                    _ => record.reference_size_bytes = vec![7],
                }
                upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
                let settle = || {
                    if abandon {
                        abandon_pending_delivery_for_namespace(&authority, &record.identity(), "failed-card")
                    } else {
                        pending_delivery_acknowledged_for_namespace(&authority, &record.identity(), "settled-file")
                    }
                };
                assert!(settle().unwrap());
                let rows = load_pending_generations_for_namespace(&authority).unwrap();
                assert_eq!(rows.len(), 1, "settlement must retain required input vector {vector}, abandon={abandon}");
                assert_eq!(rows[0].reference_paths, record.reference_paths);
                assert_eq!(rows[0].reference_sha256, record.reference_sha256);
                assert_eq!(rows[0].reference_size_bytes, record.reference_size_bytes);
                assert_eq!(rows[0].deliveries[0].acknowledged, !abandon);
                assert_eq!(rows[0].deliveries[0].abandoned, abandon);
                apply_generation_patch_for_namespace(&authority, &record.identity(), GenerationRecoveryPatch::ReleaseReferenceInputs).unwrap();
                let rows = load_pending_generations_for_namespace(&authority).unwrap();
                assert_eq!(rows.len(), 1, "input release is not an implicit remove");
                assert_eq!(rows[0].lineage_reference_paths, ["lineage"]);
                assert_eq!(rows[0].uploaded_file_ids, ["uploaded"]);
                assert!(settle().unwrap());
                assert!(load_pending_generations_for_namespace(&authority).unwrap().is_empty());
            }
        }
    }
    #[test]
    fn ambiguous_delivery_matches_preserve_bytes_and_asset_queries_fail_closed() {
        let (_root, authority, scope) = fixture(9);
        let mut record = pending_record();
        record.deliveries = vec![
            PendingDeliveryRecord {
                file_id: "file".into(),
                failed_asset_id: "failed".into(),
                ..Default::default()
            };
            2
        ];
        upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
        let original = bytes(&authority, RecoveryDocument::Generations);
        assert!(pending_delivery_acknowledged_for_namespace(
            &authority,
            &record.identity(),
            "file"
        )
        .is_err());
        assert!(
            abandon_pending_delivery_for_namespace(&authority, &record.identity(), "failed")
                .is_err()
        );
        assert!(
            recoverable_delivery_for_failed_asset_for_namespace(&authority, "failed")
                .unwrap()
                .is_none()
        );
        assert!(recoverable_failed_asset_ids_for_namespace(&authority)
            .unwrap()
            .is_empty());
        assert_eq!(bytes(&authority, RecoveryDocument::Generations), original);
    }
    #[test]
    fn prompt_and_order_semantic_updates_preserve_identity() {
        let (_root, authority, scope) = fixture(9);
        let prompt = pending_prompt_record();
        let order = pending_order_record(OWNER, 9);
        upsert_pending_prompt_task_for_namespace(&authority, &scope, prompt.clone()).unwrap();
        upsert_pending_order_for_namespace(&authority, &scope, order.clone()).unwrap();
        for patch in [
            PromptTaskRecoveryPatch::UploadedFileIds(vec!["file".into()]),
            PromptTaskRecoveryPatch::ServerTaskId("server".into()),
            PromptTaskRecoveryPatch::TerminalError("error".into()),
            PromptTaskRecoveryPatch::ResultPrompt("result".into()),
            PromptTaskRecoveryPatch::ResultCommitted,
            PromptTaskRecoveryPatch::AppliedToTarget,
            PromptTaskRecoveryPatch::ReleaseCustomPromptResult,
        ] {
            assert!(
                apply_prompt_task_patch_for_namespace(&authority, &prompt.identity(), patch)
                    .unwrap()
            );
        }
        let saved = load_pending_prompt_tasks_for_namespace(&authority)
            .unwrap()
            .remove(0);
        assert_eq!(saved.identity(), prompt.identity());
        assert!(saved.result_committed && !saved.applied_to_target);
        assert_eq!(saved.result_prompt, "result");
        assert!(update_pending_order_id_for_namespace(&authority, &order.identity(), "").is_err());
        assert!(
            update_pending_order_quote_id_for_namespace(&authority, &order.identity(), " ")
                .is_err()
        );
        update_pending_order_id_for_namespace(&authority, &order.identity(), "order").unwrap();
        update_pending_order_quote_id_for_namespace(&authority, &order.identity(), "quote")
            .unwrap();
        let saved = load_pending_orders_for_namespace(&authority)
            .unwrap()
            .remove(0);
        assert_eq!(saved.identity(), order.identity());
        assert_eq!(saved.order_id, "order");
        assert_eq!(saved.upgrade_quote_id, "quote");
    }
    #[test]
    fn legacy_adapters_never_invoke_callbacks_or_take_the_recovery_lock() {
        let _guard = recovery_lock().lock().unwrap_or_else(|p| p.into_inner());
        let error = update_pending_generation_scoped(OWNER, 9, "request", |_| {
            panic!("legacy callback invoked")
        })
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<RecoveryError>(),
            Some(&RecoveryError::NamespaceRequired)
        );
        assert!(
            update_pending_prompt_task_scoped(OWNER, 9, "request", |_| panic!(
                "legacy callback invoked"
            ))
            .is_err()
        );
        assert!(claim_legacy_pending_generation(OWNER, 9, "request", "server").is_err());
        assert!(claim_legacy_pending_order(OWNER, 9, "request", "server").is_err());
        assert!(load_pending_prompt_tasks().is_empty());
        assert!(pending_recovery_file_references().is_err());
        assert!(pending_recovery_may_reference_files());
    }

    fn publication_conflict_case(initial: bool, conflicts: usize) {
        let (_root, authority, scope) = fixture(9);
        if initial {
            upsert_pending_order_for_namespace(&authority, &scope, pending_order_record(OWNER, 9))
                .unwrap();
        }
        let sentinel_key = ManagedFileKey::new(ManagedUserArea::Recovery, "unrelated.tmp").unwrap();
        let mut sentinel = authority.create_new_regular(&sentinel_key).unwrap();
        authority
            .write_new_regular_from(&mut sentinel, &mut &b"unrelated"[..])
            .unwrap();
        authority.sync_regular(&mut sentinel).unwrap();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(0);
        let (done_tx, done_rx) = mpsc::sync_channel::<()>(0);
        let contender_authority = authority.clone();
        let contender = std::thread::spawn(move || {
            for index in 0..conflicts {
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let mut file = if contender_authority
                    .open_optional_regular(&RecoveryDocument::Orders.key().unwrap())
                    .unwrap()
                    .is_some()
                {
                    serde_json::from_slice::<OrderRecoveryFile>(&bytes(
                        &contender_authority,
                        RecoveryDocument::Orders,
                    ))
                    .unwrap()
                } else {
                    OrderRecoveryFile::empty()
                };
                let mut winner = pending_order_record(OWNER, 9);
                winner.client_request_id = format!("winner-{index}");
                file.orders.push(winner);
                put(
                    &contender_authority,
                    RecoveryDocument::Orders,
                    &serde_json::to_vec(&file).unwrap(),
                );
                done_tx.send(()).unwrap();
            }
        });
        let mut attempts = 0;
        let result = NamespaceRecoveryStore {
            authority: &authority,
        }
        .mutate_orders(|file| {
            attempts += 1;
            // Only synchronization in the callback; contender owns all fixture I/O.
            if attempts <= conflicts {
                ready_tx.send(()).unwrap();
                done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            if let Some(row) = file.orders.first_mut() {
                row.product_code = "committed-edit".into();
            }
            Ok(file.orders.len())
        });
        contender.join().unwrap();
        if conflicts == 3 {
            assert_eq!(
                result.unwrap_err().downcast_ref::<RecoveryError>(),
                Some(&RecoveryError::ConflictExhausted)
            );
            assert_eq!(attempts, 3);
        } else {
            assert_eq!(result.unwrap(), usize::from(initial) + conflicts);
            assert_eq!(attempts, conflicts + 1);
            let saved = load_pending_orders_for_namespace(&authority).unwrap();
            if !saved.is_empty() {
                assert_eq!(saved[0].product_code, "committed-edit");
            }
        }
        let names = authority
            .enumerate_regular_names(ManagedUserArea::Recovery)
            .unwrap();
        let names = names
            .iter()
            .map(|name| name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from(["pending-orders.json", "unrelated.tmp"])
        );
        let mut retained = authority.open_existing_regular(&sentinel_key).unwrap();
        let mut content = Vec::new();
        authority
            .read_regular_to(&mut retained, &mut content)
            .unwrap();
        assert_eq!(content, b"unrelated");
        assert_eq!(
            load_pending_orders_for_namespace(&authority).unwrap().len(),
            usize::from(initial) + conflicts
        );
    }
    #[test]
    fn first_appearance_reloads_winner_and_returns_only_committed_value() {
        publication_conflict_case(false, 1);
    }
    #[test]
    fn stale_replacement_reloads_winner_without_losing_unrelated_rows() {
        publication_conflict_case(true, 1);
    }
    #[test]
    fn exactly_three_publication_conflicts_exhaust_without_a_fourth_callback() {
        publication_conflict_case(false, 3);
    }

    #[derive(Debug)]
    struct CallbackFailure;
    impl std::fmt::Display for CallbackFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("callback failure")
        }
    }
    impl std::error::Error for CallbackFailure {}
    #[test]
    fn callback_error_is_not_retried_and_retains_its_primary_type() {
        let (_root, authority, scope) = fixture(9);
        upsert_pending_order_for_namespace(&authority, &scope, pending_order_record(OWNER, 9))
            .unwrap();
        let before = bytes(&authority, RecoveryDocument::Orders);
        let mut calls = 0;
        let error = NamespaceRecoveryStore {
            authority: &authority,
        }
        .mutate_orders::<()>(|file| {
            calls += 1;
            file.orders[0].product_code = "tentative".into();
            Err(CallbackFailure.into())
        })
        .unwrap_err();
        assert!(error.downcast_ref::<CallbackFailure>().is_some());
        assert_eq!(calls, 1);
        assert_eq!(bytes(&authority, RecoveryDocument::Orders), before);
        assert_eq!(
            authority
                .enumerate_regular_names(ManagedUserArea::Recovery)
                .unwrap()
                .len(),
            1
        );
    }
    #[cfg(unix)]
    #[test]
    fn failed_owned_temporary_cleanup_preserves_primary_error_and_attacker_bytes() {
        let (root, authority, _scope) = fixture(9);
        let key = RecoveryDocument::Orders.key().unwrap();
        let temporary = authority.create_temporary_regular_for(&key).unwrap();
        let parent = root.path().join("accounts").join(OWNER).join("recovery");
        let retained = root.path().join("retained-recovery");
        std::fs::rename(&parent, &retained).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("attacker.txt"), b"unchanged").unwrap();
        let error = NamespaceRecoveryStore {
            authority: &authority,
        }
        .cleanup_error(temporary, CallbackFailure.into());
        assert!(error.downcast_ref::<CallbackFailure>().is_some());
        assert!(error.to_string().contains("temporary cleanup failed"));
        assert_eq!(
            std::fs::read(parent.join("attacker.txt")).unwrap(),
            b"unchanged"
        );
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
        // Failed cleanup retains the originally bound temporary; no pathname fallback.
        assert_eq!(std::fs::read_dir(&retained).unwrap().count(), 1);
    }
    #[test]
    fn empty_delivery_identifier_cannot_abandon_a_saved_result() {
        let (_root, authority, scope) = fixture(9);
        let mut record = pending_record();
        record.terminal = true;
        record.expected_success_count = 1;
        record.deliveries.push(PendingDeliveryRecord {
            file_id: "saved-file".into(),
            local_path: "saved-result".into(),
            ..Default::default()
        });
        upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
        let before = bytes(&authority, RecoveryDocument::Generations);
        assert!(
            !abandon_pending_delivery_for_namespace(&authority, &record.identity(), "").unwrap()
        );
        assert_eq!(bytes(&authority, RecoveryDocument::Generations), before);
    }
    #[test]
    fn invalid_document_stops_before_the_callback_and_preserves_unrelated_temporary() {
        let (_root, authority, _scope) = fixture(9);
        put(&authority, RecoveryDocument::Orders, b"not-json");
        let key = ManagedFileKey::new(ManagedUserArea::Recovery, "unrelated.tmp").unwrap();
        let _unrelated = authority.create_new_regular(&key).unwrap();
        let error = NamespaceRecoveryStore {
            authority: &authority,
        }
        .mutate_orders::<()>(|_| panic!("invalid input invoked callback"))
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<RecoveryError>(),
            Some(&RecoveryError::InvalidDocument)
        );
        assert_eq!(bytes(&authority, RecoveryDocument::Orders), b"not-json");
        assert!(authority.open_existing_regular(&key).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn all_fixed_documents_reject_swapped_parent_before_temp_creation() {
        for document in [
            RecoveryDocument::Generations,
            RecoveryDocument::PromptTasks,
            RecoveryDocument::Orders,
        ] {
            let (root, authority, _scope) = fixture(9);
            let original = match document {
                RecoveryDocument::Generations => {
                    serde_json::to_vec(&RecoveryFile::empty()).unwrap()
                }
                RecoveryDocument::PromptTasks => {
                    serde_json::to_vec(&PromptTaskRecoveryFile::empty()).unwrap()
                }
                RecoveryDocument::Orders => {
                    serde_json::to_vec(&OrderRecoveryFile::empty()).unwrap()
                }
            };
            put(&authority, document, &original);
            let parent = root.path().join("accounts").join(OWNER).join("recovery");
            let retained = root.path().join("retained-recovery");
            let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(0);
            let (done_tx, done_rx) = mpsc::sync_channel::<()>(0);
            let parent_for_worker = parent.clone();
            let retained_for_worker = retained.clone();
            let contender = std::thread::spawn(move || {
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                std::fs::rename(&parent_for_worker, &retained_for_worker).unwrap();
                std::fs::create_dir(&parent_for_worker).unwrap();
                std::fs::write(parent_for_worker.join("attacker.txt"), b"unchanged").unwrap();
                done_tx.send(()).unwrap();
            });
            let mut calls = 0;
            let mut synchronize = || -> Result<()> {
                calls += 1;
                ready_tx.send(()).unwrap();
                done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(())
            };
            let store = NamespaceRecoveryStore {
                authority: &authority,
            };
            let result = match document {
                RecoveryDocument::Generations => store.mutate_generations(|_| synchronize()),
                RecoveryDocument::PromptTasks => store.mutate_prompt_tasks(|_| synchronize()),
                RecoveryDocument::Orders => store.mutate_orders(|_| synchronize()),
            };
            contender.join().unwrap();
            assert!(result.is_err());
            assert_eq!(calls, 1);
            assert_eq!(
                std::fs::read(parent.join("attacker.txt")).unwrap(),
                b"unchanged"
            );
            assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
            let name = match document {
                RecoveryDocument::Generations => "pending-generations.json",
                RecoveryDocument::PromptTasks => "pending-prompt-tasks.json",
                RecoveryDocument::Orders => "pending-orders.json",
            };
            assert_eq!(std::fs::read(retained.join(name)).unwrap(), original);
        }
    }
    #[cfg(unix)]
    #[test]
    fn temporary_creation_permission_failure_stops_after_one_callback() {
        use std::os::unix::fs::PermissionsExt;
        let (root, authority, scope) = fixture(9);
        upsert_pending_order_for_namespace(&authority, &scope, pending_order_record(OWNER, 9))
            .unwrap();
        let parent = root.path().join("accounts").join(OWNER).join("recovery");
        let before = bytes(&authority, RecoveryDocument::Orders);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let mut calls = 0;
        let result = NamespaceRecoveryStore {
            authority: &authority,
        }
        .mutate_orders(|file| {
            calls += 1;
            file.orders[0].product_code = "tentative".into();
            Ok(())
        });
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert_eq!(calls, 1);
        assert!(result
            .unwrap_err()
            .downcast_ref::<ManagedPublicationConflict>()
            .is_none());
        assert_eq!(bytes(&authority, RecoveryDocument::Orders), before);
    }
    #[test]
    fn reference_metadata_includes_owned_old_epochs_but_asset_queries_do_not() {
        let (_root, authority, _scope) = fixture(10);
        let mut record = pending_record();
        record.reference_paths = vec!["input".into()];
        record.lineage_reference_paths = vec!["lineage".into()];
        record.deliveries = vec![PendingDeliveryRecord {
            file_id: "file".into(),
            failed_asset_id: "failed".into(),
            local_path: "result".into(),
            ..Default::default()
        }];
        let prompt = pending_prompt_record();
        put(
            &authority,
            RecoveryDocument::Generations,
            &serde_json::to_vec(&serde_json::json!({"schema_version":2,"generations":[record]}))
                .unwrap(),
        );
        put(&authority,RecoveryDocument::PromptTasks,&serde_json::to_vec(&serde_json::json!({"schema_version":2,"prompt_tasks":[prompt],"deep_optimizations":[]})).unwrap());
        assert!(
            recoverable_delivery_for_failed_asset_for_namespace(&authority, "failed")
                .unwrap()
                .is_none()
        );
        let paths = pending_recovery_file_references_for_namespace(&authority)
            .unwrap()
            .into_iter()
            .map(|(_, _, path)| path)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            paths,
            BTreeSet::from(["input".into(), "lineage".into(), "result".into()])
        );
    }
    #[test]
    fn production_recovery_inventory_has_no_path_or_direct_filesystem_authority() {
        let production = include_str!("recovery.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in [
            "std::fs",
            "fs::",
            "&Path",
            "PathBuf",
            "app_data_dir",
            "restore_json_backup",
            "tempfile",
            "generation_recovery_path",
            "order_recovery_path",
            "prompt_task_recovery_path",
        ] {
            assert!(
                !production.contains(forbidden),
                "forbidden production boundary: {forbidden}"
            );
        }
    }
}
