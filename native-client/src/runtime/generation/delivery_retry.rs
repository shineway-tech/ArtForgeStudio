use super::*;

macro_rules! ensure_delivery {
    ($condition:expr, $message:expr) => {
        if !$condition {
            return Err(anyhow!($message).into());
        }
    };
}

/// Constructed only by the real retained-delivery consumer after server task/payer
/// verification and a complete read of the held canonical output. This is not a
/// generic file-index repair capability and cannot be caller-constructed.
pub(super) struct VerifiedDeliveryIndexContent<'a> {
    authority: &'a NamespaceStorageAuthority,
    api: &'a GenerationApi,
    scope: &'a SessionScope,
    record: &'a PendingGenerationRecord,
    confirmation: &'a DeliveryConfirmation,
    source_path: &'a str,
    file: &'a NamespaceManagedFile,
    metadata: ManagedFileMetadata,
    terminal_success_count: Option<usize>,
    derived: Option<DerivedCutoutIndexContent<'a>>,
}
struct DerivedCutoutIndexContent<'a> {
    remote: &'a NamespaceDeliveryProof,
    source: &'a NamespaceManagedFile,
    source_metadata: &'a ManagedFileMetadata,
    sha256: &'a str,
    size: u64,
}
impl VerifiedDeliveryIndexContent<'_> {
    pub(super) fn authority(&self) -> &NamespaceStorageAuthority { self.authority }
    pub(super) fn file(&self) -> &NamespaceManagedFile { self.file }
    pub(super) fn metadata(&self) -> &ManagedFileMetadata { &self.metadata }
    pub(super) fn require_current(&self) -> Result<()> {
        self.api.ensure_scope_active(self.scope)?;
        ensure_delivery!(self.scope.owner_user_id == self.authority.user_public_id()
            && self.scope.auth_epoch == self.authority.lease().auth_epoch
            && self.record.owner_user_id == self.scope.owner_user_id
            && self.record.auth_epoch == self.scope.auth_epoch,
            "delivery reconciliation namespace changed");
        let current = load_exact_delivery_record(self.authority, &self.record.identity())?;
        // Sibling images are committed/acknowledged while this image downloads.
        // Compare immutable task metadata and this item's retained row only;
        // another item's progress must not invalidate verified local content.
        ensure_delivery!(delivery_task_metadata(&current)? == delivery_task_metadata(self.record)?,
            "delivery reconciliation retained record changed");
        ensure_delivery!(self.record.client_request_id == self.confirmation.client_request_id
            && self.record.server_task_id == self.confirmation.task_id
            && self.confirmation.item_index < self.record.count as usize,
            "delivery reconciliation task changed");
        let saved = exact_saved_delivery(&current, self.confirmation)?;
        let original = exact_saved_delivery(self.record, self.confirmation)?;
        ensure_delivery!(serde_json::to_value(saved)? == serde_json::to_value(original)?,
            "delivery reconciliation selected item changed");
        ensure_delivery!(
            (!self.record.terminal || (current.terminal
                && current.expected_success_count == self.record.expected_success_count))
                && (!current.terminal || self.terminal_success_count
                    .is_some_and(|count| count == current.expected_success_count)),
            "delivery reconciliation terminal progress changed");
        let path = self.authority.lease().namespace.path(self.file.key().area())
            .join(self.file.key().relative_name().as_str());
        ensure_delivery!(path.to_str() == Some(self.source_path),
            "delivery reconciliation canonical path changed");
        let expected_size = if let Some(derived) = &self.derived {
            ensure_delivery!(self.record.task_type == "image_cutout"
                && self.file.key().area() == ManagedUserArea::Output
                && self.file.key().relative_name().as_str() == format!("{}-cutout-v1.png", self.confirmation.file_id)
                && derived.sha256.len() == 64
                && derived.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "derived cutout index provenance changed");
            derived.remote.ensure_current()?;
            derived.remote.ensure_index_current()?;
            ensure_delivery!(self.authority.inspect_regular(derived.source)? == *derived.source_metadata,
                "derived cutout original input changed");
            derived.size
        } else { self.confirmation.size_bytes };
        let metadata = self.authority.inspect_regular(self.file)?;
        ensure_delivery!(metadata == self.metadata && metadata.link_count == 1
            && metadata.byte_size == expected_size,
            "delivery reconciliation held content changed");
        Ok(())
    }
}

pub(super) struct NamespaceDeliveryProof {
    authority: Arc<NamespaceStorageAuthority>,
    api: GenerationApi,
    index: FileIndex,
    scope: SessionScope,
    record: PendingGenerationRecord,
    confirmation: DeliveryConfirmation,
    file: NamespaceManagedFile,
    indexed: ManagedFileRecord,
    source_path: String,
    terminal_success_count: Option<usize>,
    derived_cutout: Option<DerivedCutoutDelivery>,
}

pub(super) struct PreparedNamespaceDelivery { proof: NamespaceDeliveryProof, preview: PreparedDeliveryPreview }
pub(super) struct PreparedNamespaceVideoDelivery { proof: NamespaceDeliveryProof }
impl std::ops::Deref for PreparedNamespaceDelivery {
    type Target=NamespaceDeliveryProof;
    fn deref(&self)->&Self::Target { &self.proof }
}
impl std::ops::Deref for PreparedNamespaceVideoDelivery {
    type Target=NamespaceDeliveryProof;
    fn deref(&self)->&Self::Target { &self.proof }
}
impl PreparedNamespaceDelivery {
    pub(super) fn preview(&self)->&PreparedDeliveryPreview { &self.preview }
    pub(super) fn into_proof(self)->NamespaceDeliveryProof { self.proof }
}
impl PreparedNamespaceVideoDelivery {
    pub(super) fn into_proof(self)->NamespaceDeliveryProof { self.proof }
}

pub(super) fn prepare_runtime_image_delivery(
    api: &GenerationApi, authority: Arc<NamespaceStorageAuthority>, key: &str, item: usize,
) -> std::result::Result<Option<PreparedNamespaceDelivery>, DeliveryRetryError> {
    let record = load_pending_generations_for_namespace(&authority)?.into_iter()
        .find(|row| row.client_request_id == key).ok_or_else(|| anyhow!("original delivery record missing"))?;
    let index = authority.delivery_index()?;
    prepare_namespace_delivery(api, authority, index, &record.identity(), item).map(Some)
}

pub(super) fn prepare_namespace_delivery(
    api: &GenerationApi,
    authority: Arc<NamespaceStorageAuthority>,
    index: FileIndex,
    expected: &RecoveryRecordIdentity,
    item_index: usize,
) -> std::result::Result<PreparedNamespaceDelivery, DeliveryRetryError> {
    let (proof,preview)=prepare_namespace_delivery_proof(api,authority,index,expected,item_index,NamespaceDeliveryKind::Image)?;
    Ok(PreparedNamespaceDelivery {proof,preview:preview.ok_or_else(||anyhow!("image delivery preview missing"))?})
}
pub(super) fn prepare_namespace_video_delivery(
    api:&GenerationApi,authority:Arc<NamespaceStorageAuthority>,index:FileIndex,
    expected:&RecoveryRecordIdentity,item_index:usize,
)->std::result::Result<PreparedNamespaceVideoDelivery,DeliveryRetryError> {
    let (proof,preview)=prepare_namespace_delivery_proof(api,authority,index,expected,item_index,NamespaceDeliveryKind::Video)?;
    ensure_delivery!(preview.is_none(),"video delivery cannot carry an image presentation");
    Ok(PreparedNamespaceVideoDelivery {proof})
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum NamespaceDeliveryKind { Image, Video, Cutout }
fn prepare_namespace_delivery_proof(
    api:&GenerationApi,authority:Arc<NamespaceStorageAuthority>,index:FileIndex,
    expected:&RecoveryRecordIdentity,item_index:usize,kind:NamespaceDeliveryKind,
)->std::result::Result<(NamespaceDeliveryProof,Option<PreparedDeliveryPreview>),DeliveryRetryError> {
    let video = kind == NamespaceDeliveryKind::Video;
    let scope = SessionScope {
        owner_user_id: authority.user_public_id().to_owned(),
        auth_epoch: authority.lease().auth_epoch,
    };
    api.ensure_scope_active(&scope)?;
    let mut record = load_exact_delivery_record(&authority, expected)?;
    ensure_delivery!(
        record.auth_epoch == scope.auth_epoch,
        "delivery session mismatch"
    );
    ensure_delivery!(
        record.count > 0 && item_index < record.count as usize,
        "invalid delivery item index"
    );
    ensure_delivery!(
        (record.canvas_source_node_id.is_empty() || (kind==NamespaceDeliveryKind::Image && record.task_type=="image_generation")) && if video {
            matches!(record.task_type.as_str(),"image_to_video"|"video_generation")
                && record.video_request.as_ref().is_some_and(|request| request.validate().is_ok()
                    && request.client_request_id==record.client_request_id && request.task_type==record.task_type)
        } else if kind == NamespaceDeliveryKind::Cutout { record.task_type == "image_cutout" && record.count == 1 }
        else { matches!(record.task_type.as_str(),
            "image_generation"|"image_edit"|"image_upscale"|"image_watermark_removal"|"image_colorization"|"image_enhancement") },
        "this delivery consumer requires an ordinary image task"
    );
    require_delivery_uuid(&record.server_task_id)?;
    let detail = api.task_scoped(&record.server_task_id, &scope)?;
    ensure_delivery!(
        detail.id == record.server_task_id,
        "delivery task identity mismatch"
    );
    api::require_saved_group(
        &record.billing_account_group_id,
        &detail.billing_account_group_id,
    )?;
    let mut indexes = BTreeSet::new();
    ensure_delivery!(
        detail
            .items
            .iter()
            .all(|item| item.index < record.count as usize && indexes.insert(item.index)),
        "ambiguous delivery item indexes"
    );
    let successes = detail
        .items
        .iter()
        .filter(|item| item.status == "succeeded")
        .count();
    ensure_delivery!(
        detail.success_count >= 0
            && detail.failure_count >= 0
            && detail.success_count as usize == successes
            && i64::from(detail.success_count) + i64::from(detail.failure_count)
                <= i64::from(record.count)
            && (detail.requested_count == 0 || detail.requested_count == record.count),
        "inconsistent delivery success count"
    );
    let item = detail
        .items
        .iter()
        .find(|item| item.index == item_index)
        .filter(|item| item.status == "succeeded")
        .ok_or_else(|| anyhow!("successful delivery item is unavailable"))?;
    let remote = item
        .file
        .as_ref()
        .filter(|file| file.status == "available")
        .ok_or(DeliveryRetryError::Expired)?;
    require_delivery_uuid(&remote.id)?;
    let size = remote
        .size_bytes
        .parse::<u64>()
        .ok()
        .filter(|size| *size > 0 && size.to_string() == remote.size_bytes)
        .ok_or_else(|| anyhow!("invalid delivery size"))?;
    ensure_delivery!(
        remote.sha256.len() == 64 && remote.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid delivery hash"
    );
    let mut confirmation = DeliveryConfirmation {
        client_request_id: record.client_request_id.clone(),
        item_index,
        task_id: record.server_task_id.clone(),
        file_id: remote.id.clone(),
        sha256: remote.sha256.clone(),
        size_bytes: size,
        failed_asset_id: None,
    };
    if let Some(saved) = exact_saved_delivery(&record, &confirmation)? {
        confirmation.failed_asset_id =
            (!saved.failed_asset_id.is_empty()).then(|| saved.failed_asset_id.clone());
    }
    ensure_delivery!(kind != NamespaceDeliveryKind::Cutout || remote.mime_type == "image/png",
        "cutout remote mask must be PNG");
    let extension = match (video,remote.mime_type.as_str()) {
        (false,"image/jpeg")=>"jpg",(false,"image/webp")=>"webp",(false,"image/png")=>"png",
        (true,"video/mp4")=>"mp4",(true,"video/webm")=>"webm",(true,"video/quicktime")=>"mov",
        _=>return Err(anyhow!("unsupported output MIME for retained task type").into()),
    };
    let area=if video {ManagedUserArea::Videos}else{ManagedUserArea::Output};
    let destination = ManagedFileKey::new(
        area,
        &format!("{}.{extension}", remote.id),
    )?;
    let source_path = authority
        .lease()
        .namespace
        .path(area)
        .join(destination.relative_name().as_str())
        .to_string_lossy()
        .into_owned();
    if exact_saved_delivery(&record, &confirmation)?.is_some_and(|saved| !saved.local_path.is_empty() && saved.local_path != source_path) {
        // The server task/file/payer and exact retained row have been verified.
        // Discard only this stale display path; never open or adopt its target.
        let ids = BTreeSet::from([confirmation.file_id.clone()]);
        ensure_delivery!(apply_generation_patch_for_namespace(&authority, expected, GenerationRecoveryPatch::ClearDeliveryLocalPaths(ids))?, "saved delivery path changed concurrently");
        for saved in &mut record.deliveries {
            if saved.file_id == confirmation.file_id { saved.local_path.clear(); }
        }
    }
    let (file, preview) = if let Some(mut file) = authority.open_optional_regular(&destination)? {
        verify_namespace_delivery_file(&authority, &mut file, &confirmation)?;
        let preview = if kind != NamespaceDeliveryKind::Image {None}else{Some(prepare_delivery_preview_for_namespace(&authority, &mut file)?)};
        (file, preview)
    } else {
        let mut temporary = authority.create_temporary_regular_for(&destination)?;
        let preparation = (|| -> std::result::Result<_, DeliveryRetryError> {
            api.download_verified_for_namespace(remote, &scope, &authority, &mut temporary)?;
            let preview = if kind != NamespaceDeliveryKind::Image {None}else{Some(prepare_delivery_preview_for_namespace(&authority, &mut temporary)?)};
            api.ensure_scope_active(&scope)?;
            let _publication = authority.begin_ordinary_mutation()?;
            authority.sync_regular(&mut temporary)?;
            authority.publish_regular(
                &mut temporary,
                NamespaceManagedPublication::Absent(&destination),
            )?;
            Ok(preview)
        })();
        match preparation {
            Ok(preview) => (temporary, preview),
            Err(error) => {
                let appeared = matches!(&error, DeliveryRetryError::Local(error)
                    if error.downcast_ref::<ManagedPublicationConflict>() == Some(&ManagedPublicationConflict::DestinationAppeared));
                // Only the owned temporary can be unlinked, even on binding replacement.
                // Preserve the primary typed failure if cleanup itself cannot validate.
                let _ = authority.unlink_regular(temporary);
                if !appeared {
                    return Err(error);
                }
                api.ensure_scope_active(&scope)?;
                let mut winner = authority.open_existing_regular(&destination)?;
                verify_namespace_delivery_file(&authority, &mut winner, &confirmation)?;
                let preview = if kind != NamespaceDeliveryKind::Image {None}else{Some(prepare_delivery_preview_for_namespace(&authority, &mut winner)?)};
                (winner, preview)
            }
        }
    };
    api.ensure_scope_active(&scope)?;
    let retained = authority.inspect_regular(&file)?;
    let mut registration_file = authority.open_existing_regular(&destination)?;
    let before_hash = authority.inspect_regular(&registration_file)?;
    ensure_delivery!(before_hash == retained && before_hash.link_count == 1,
        "delivery changed before index registration");
    // The private reconciliation capability is minted from the complete held
    // content, not from the path, suffix, size alone, or a generic registration.
    verify_namespace_delivery_file(&authority, &mut registration_file, &confirmation)?;
    let metadata = authority.inspect_regular(&registration_file)?;
    ensure_delivery!(metadata == before_hash, "delivery changed during content verification");
    let proof = VerifiedDeliveryIndexContent {
        authority: &authority, api, scope: &scope, record: &record,
        confirmation: &confirmation, source_path: &source_path,
        file: &registration_file, metadata, derived: None,
        terminal_success_count: detail.terminal().then_some(successes),
    };
    let indexed = {
        let _commit = authority.begin_ordinary_mutation()?;
        index.reconcile_verified_delivery_content(&proof).map_err(anyhow::Error::from)?
    };
    let prepared = NamespaceDeliveryProof {
        authority,
        api: api.clone(),
        index,
        scope,
        record,
        confirmation,
        file,
        indexed,
        source_path,
        terminal_success_count: detail.terminal().then_some(successes),
        derived_cutout: None,
    };
    prepared.ensure_current()?;
    Ok((prepared,preview))
}

pub(super) fn acknowledge_namespace_delivery(
    committed: CommittedNamespaceDelivery,
) -> std::result::Result<bool, DeliveryRetryError> {
    let mut prepared = committed.into_prepared();
    prepared.ensure_current()?;
    verify_namespace_delivery_file(
        &prepared.authority,
        &mut prepared.file,
        &prepared.confirmation,
    )?;
    prepared.ensure_index_current()?;
    if let Some(derived) = &mut prepared.derived_cutout {
        read_cutout_held_bytes(&prepared.authority, &mut derived.output,
            derived.output_metadata.byte_size, &derived.sha256)?;
        read_cutout_held_bytes(&prepared.authority, &mut derived.source,
            prepared.record.reference_size_bytes[0], &prepared.record.reference_sha256[0])?;
    }
    let identity = prepared.record.identity();
    let current = load_exact_delivery_record(&prepared.authority, &identity)?;
    // Delivery rows and terminal progress can advance independently, but the
    // captured task/prompt/input metadata may not silently change underneath us.
    ensure_delivery!(
        delivery_task_metadata(&current)? == delivery_task_metadata(&prepared.record)?,
        "saved delivery record changed"
    );
    let saved_delivery = exact_saved_delivery(&current, &prepared.confirmation)?;
    ensure_delivery!(
        saved_delivery
            .map(|delivery| delivery.failed_asset_id.as_str())
            .filter(|id| !id.is_empty())
            == prepared.confirmation.failed_asset_id.as_deref(),
        "saved failed-card identity changed"
    );
    if let Some(delivery) = saved_delivery {
        ensure_delivery!(
            delivery.local_path.is_empty() || delivery.local_path == prepared.source_path,
            "saved delivery path changed"
        );
    }
    if let Some(expected) = prepared.terminal_success_count {
        ensure_delivery!(
            !current.terminal || current.expected_success_count == expected,
            "saved terminal success count changed"
        );
    }
    if !pending_delivery_saved_for_namespace(
        &prepared.authority,
        &identity,
        &prepared.confirmation,
        &prepared.source_path,
    )? {
        return Ok(false);
    }
    if let Some(expected_success_count) = prepared.terminal_success_count {
        if !apply_generation_patch_for_namespace(
            &prepared.authority,
            &identity,
            GenerationRecoveryPatch::Terminal {
                expected_success_count,
            },
        )? {
            return Ok(false);
        }
    }
    prepared.ensure_current()?;
    prepared.ensure_index_current()?;
    prepared.api.acknowledge_delivery_scoped(
        &prepared.confirmation.task_id,
        &prepared.confirmation.file_id,
        &prepared.confirmation.sha256,
        prepared.confirmation.size_bytes,
        &prepared.scope,
    )?;
    prepared.api.ensure_scope_active(&prepared.scope)?;
    if prepared.derived_cutout.is_some() {
        prepared.ensure_current()?;
        prepared.ensure_index_current()?;
        return settle_acknowledged_cutout_delivery_for_namespace(&AcknowledgedCutoutDelivery(prepared)).map_err(Into::into);
    }
    pending_delivery_acknowledged_for_namespace(
        &prepared.authority,
        &identity,
        &prepared.confirmation.file_id,
    )
    .map_err(Into::into)
}

impl NamespaceDeliveryProof {
    pub(super) fn record(&self) -> &PendingGenerationRecord {
        &self.record
    }
    pub(super) fn confirmation(&self) -> &DeliveryConfirmation {
        &self.confirmation
    }
    pub(super) fn source_path(&self) -> &str {
        self.derived_cutout.as_ref().map_or(self.source_path.as_str(), |derived| derived.source_path.as_str())
    }
    pub(super) fn lease(&self) -> &NamespaceLease {
        self.authority.lease()
    }
    pub(super) fn ensure_current(&self) -> Result<()> {
        self.api.ensure_scope_active(&self.scope)?;
        let retained = self.authority.inspect_regular(&self.file)?;
        ensure_delivery!(
            retained.identity == self.indexed.physical_identity
                && retained.byte_size == self.confirmation.size_bytes,
            "retained delivery identity changed"
        );
        if let Some(derived) = &self.derived_cutout { derived.ensure_current(&self.authority)?; }
        Ok(())
    }
    fn ensure_index_current(&self) -> Result<()> {
        let indexed = self
            .index
            .find_file_by_path_for_namespace(
                &self.authority,
                self.file.key().area(),
                self.file.key().relative_name().as_str(),
            )?
            .ok_or_else(|| anyhow!("delivery index entry is missing"))?;
        ensure_delivery!(
            indexed.id == self.indexed.id
                && indexed.physical_identity == self.indexed.physical_identity
                && indexed.byte_size == self.confirmation.size_bytes
                && indexed.kind == "generation"
                && indexed.retention_policy == "durable"
                && !indexed.pending_delete,
            "delivery index identity changed"
        );
        if let Some(derived) = &self.derived_cutout { derived.ensure_index_current(&self.authority, &self.index)?; }
        Ok(())
    }
}

fn require_delivery_uuid(value: &str) -> Result<()> {
    ensure_delivery!(
        api::uuid_path_segment(value).is_ok_and(|canonical| canonical == value),
        "delivery identity is not a canonical UUID"
    );
    Ok(())
}
fn delivery_task_metadata(record: &PendingGenerationRecord) -> Result<serde_json::Value> {
    let mut metadata = record.clone();
    metadata.deliveries.clear();
    metadata.terminal = false;
    metadata.expected_success_count = 0;
    Ok(serde_json::to_value(metadata)?)
}

fn load_exact_delivery_record(
    authority: &NamespaceStorageAuthority,
    expected: &RecoveryRecordIdentity,
) -> Result<PendingGenerationRecord> {
    let mut records = load_pending_generations_for_namespace(authority)?
        .into_iter()
        .filter(|record| record.identity() == *expected);
    let record = records
        .next()
        .ok_or_else(|| anyhow!("exact saved delivery record is missing"))?;
    ensure_delivery!(
        records.next().is_none(),
        "saved delivery record is ambiguous"
    );
    Ok(record)
}
fn exact_saved_delivery<'a>(
    record: &'a PendingGenerationRecord,
    confirmation: &DeliveryConfirmation,
) -> Result<Option<&'a PendingDeliveryRecord>> {
    let mut matches = record.deliveries.iter().filter(|delivery| {
        delivery.item_index == confirmation.item_index || delivery.file_id == confirmation.file_id
    });
    let saved = matches.next();
    ensure_delivery!(matches.next().is_none(), "saved delivery is ambiguous");
    if let Some(saved) = saved {
        ensure_delivery!(
            saved.item_index == confirmation.item_index
                && saved.file_id == confirmation.file_id
                && saved.size_bytes == confirmation.size_bytes
                && saved.sha256 == confirmation.sha256
                && !saved.abandoned
                && !saved.acknowledged,
            "saved delivery confirmation mismatch"
        );
    }
    Ok(saved)
}
fn verify_namespace_delivery_file(
    authority: &NamespaceStorageAuthority,
    file: &mut NamespaceManagedFile,
    confirmation: &DeliveryConfirmation,
) -> Result<()> {
    use sha2::Digest;
    struct DigestSink {
        count: u64,
        expected: u64,
        hash: sha2::Sha256,
    }
    impl std::io::Write for DigestSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.count = self
                .count
                .checked_add(bytes.len() as u64)
                .filter(|count| *count <= self.expected)
                .ok_or_else(|| std::io::Error::other("delivery size mismatch"))?;
            self.hash.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut sink = DigestSink {
        count: 0,
        expected: confirmation.size_bytes,
        hash: sha2::Sha256::new(),
    };
    authority.read_regular_to(file, &mut sink)?;
    ensure_delivery!(
        sink.count == confirmation.size_bytes
            && format!("{:x}", sink.hash.finalize()).eq_ignore_ascii_case(&confirmation.sha256),
        "delivery integrity mismatch"
    );
    Ok(())
}

// Minted only by delivery_retry after a real committed Store receipt and the
// original remote mask acknowledgement. The constructor remains module-private.
pub(super) struct AcknowledgedCutoutDelivery(NamespaceDeliveryProof);
impl AcknowledgedCutoutDelivery {
    pub(super) fn authority(&self) -> &NamespaceStorageAuthority { &self.0.authority }
    pub(super) fn record(&self) -> &PendingGenerationRecord { &self.0.record }
    pub(super) fn confirmation(&self) -> &DeliveryConfirmation { &self.0.confirmation }
    pub(super) fn remote_path(&self) -> &str { &self.0.source_path }
}


// Remote acknowledgement always uses the enclosing
// NamespaceDeliveryProof's original file/confirmation, never these output bytes.
struct DerivedCutoutDelivery {
    source: NamespaceManagedFile,
    source_metadata: ManagedFileMetadata,
    output: NamespaceManagedFile,
    output_metadata: ManagedFileMetadata,
    indexed: ManagedFileRecord,
    source_path: String,
    sha256: String,
}

impl DerivedCutoutDelivery {
    fn ensure_current(&self, authority: &NamespaceStorageAuthority) -> Result<()> {
        anyhow::ensure!(authority.inspect_regular(&self.source)? == self.source_metadata,
            "original cutout input changed");
        anyhow::ensure!(authority.inspect_regular(&self.output)? == self.output_metadata,
            "derived cutout output changed");
        Ok(())
    }
    fn ensure_index_current(&self, authority: &NamespaceStorageAuthority, index: &FileIndex) -> Result<()> {
        let row = index.find_file_by_path_for_namespace(authority,
            self.output.key().area(), self.output.key().relative_name().as_str())?
            .ok_or_else(|| anyhow!("derived cutout index entry is missing"))?;
        anyhow::ensure!(row.id == self.indexed.id
            && row.physical_identity == self.output_metadata.identity
            && row.byte_size == self.output_metadata.byte_size
            && row.kind == "generation" && row.retention_policy == "durable"
            && row.managed && !row.pending_delete,
            "derived cutout index identity changed");
        Ok(())
    }
}

fn cutout_input_key(lease: &NamespaceLease, path: &Path) -> Result<ManagedFileKey> {
    anyhow::ensure!(path.is_absolute(), "cutout input requires an owned absolute path");
    let mut areas = vec![ManagedUserArea::Input, ManagedUserArea::Output,
        ManagedUserArea::Prompt, ManagedUserArea::Canvas, ManagedUserArea::CanvasUploads,
        ManagedUserArea::CanvasExports, ManagedUserArea::References,
        ManagedUserArea::ReferencesLibrary, ManagedUserArea::ReferencesImports,
        ManagedUserArea::Previews, ManagedUserArea::ToolboxCompressionInputs,
        ManagedUserArea::ToolboxCompressionResults, ManagedUserArea::ToolboxConversionInputs,
        ManagedUserArea::ToolboxConversionResults, ManagedUserArea::ToolboxCropInputs];
    areas.sort_by_key(|area| std::cmp::Reverse(lease.namespace.path(*area).components().count()));
    areas.into_iter().find_map(|area| path.strip_prefix(lease.namespace.path(area)).ok()
        .and_then(|name| name.to_str()).and_then(|name| ManagedFileKey::new(area, name).ok()))
        .ok_or_else(|| anyhow!("cutout input is outside owned image areas"))
}

fn read_cutout_held_bytes(authority: &NamespaceStorageAuthority,
    file: &mut NamespaceManagedFile, size: u64, sha256: &str) -> Result<Vec<u8>> {
    use sha2::Digest;
    anyhow::ensure!(size > 0 && size <= 100 * 1024 * 1024,
        "cutout input or output exceeds the owned image limit");
    let metadata = authority.inspect_regular(file)?;
    anyhow::ensure!(metadata.link_count == 1 && metadata.byte_size == size,
        "cutout held image size changed");
    let bytes = authority.with_regular_reader(file, |reader| {
        use std::io::Read;
        let mut bytes = Vec::new();
        reader.take(size + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    })?;
    anyhow::ensure!(bytes.len() as u64 == size
        && format!("{:x}", sha2::Sha256::digest(&bytes)).eq_ignore_ascii_case(sha256)
        && authority.inspect_regular(file)? == metadata,
        "cutout held image fingerprint changed");
    Ok(bytes)
}

pub(super) fn prepare_namespace_cutout_delivery(
    api: &GenerationApi, authority: Arc<NamespaceStorageAuthority>, index: FileIndex,
    expected: &RecoveryRecordIdentity, item_index: usize,
) -> std::result::Result<PreparedNamespaceDelivery, DeliveryRetryError> {
    use sha2::Digest;
    // The private kind admits only image_cutout here. Its original PNG mask is
    // separately verified/indexed; no ordinary image consumer can use this kind.
    let (mut proof, _) = prepare_namespace_delivery_proof(api, authority, index,
        expected, item_index, NamespaceDeliveryKind::Cutout)?;
    let record = &proof.record;
    ensure_delivery!(record.reference_paths.len() == 1
        && record.reference_sha256.len() == 1 && record.reference_size_bytes.len() == 1,
        "original cutout input fingerprint is missing");
    let source_path = Path::new(&record.reference_paths[0]);
    let mut source = proof.authority.open_existing_regular(
        &cutout_input_key(proof.lease(), source_path)?)?;
    let source_bytes = read_cutout_held_bytes(&proof.authority, &mut source,
        record.reference_size_bytes[0], &record.reference_sha256[0])?;
    let source_metadata = proof.authority.inspect_regular(&source)?;
    let mask_bytes = read_cutout_held_bytes(&proof.authority, &mut proof.file,
        proof.confirmation.size_bytes, &proof.confirmation.sha256)?;
    let (derived_bytes, _, _) = image_cutout_callbacks::decode_cutout_result_bytes(
        source_path, &source_bytes, &record.quality, &mask_bytes)?;
    let sha256 = format!("{:x}", sha2::Sha256::digest(&derived_bytes));
    let size = derived_bytes.len() as u64;
    ensure_delivery!(size > 0 && size <= 100 * 1024 * 1024,
        "derived cutout output exceeds the owned image limit");
    proof.ensure_current()?;
    ensure_delivery!(proof.authority.inspect_regular(&source)? == source_metadata,
        "cutout input changed during derivation");
    let key = ManagedFileKey::new(ManagedUserArea::Output,
        &format!("{}-cutout-v1.png", proof.confirmation.file_id))?;
    let output_path = proof.lease().namespace.path(ManagedUserArea::Output)
        .join(key.relative_name().as_str()).to_str()
        .ok_or_else(|| anyhow!("derived cutout path is not representable"))?.to_owned();
    let mut output = if let Some(mut output) = proof.authority.open_optional_regular(&key)? {
        read_cutout_held_bytes(&proof.authority, &mut output, size, &sha256)?;
        output
    } else {
        let mut output = proof.authority.create_temporary_regular_for(&key)?;
        let publication = (|| -> Result<()> {
            let _mutation = proof.authority.begin_ordinary_mutation()?;
            proof.authority.write_new_regular_from(&mut output, &mut derived_bytes.as_slice())?;
            read_cutout_held_bytes(&proof.authority, &mut output, size, &sha256)?;
            proof.authority.sync_regular(&mut output)?;
            proof.authority.publish_regular(&mut output, NamespaceManagedPublication::Absent(&key))?;
            Ok(())
        })();
        match publication {
            Ok(()) => output,
            Err(error) => {
                let appeared = error.downcast_ref::<ManagedPublicationConflict>()
                    == Some(&ManagedPublicationConflict::DestinationAppeared);
                let _ = proof.authority.unlink_regular(output);
                if !appeared { return Err(error.into()); }
                let mut winner = proof.authority.open_existing_regular(&key)?;
                read_cutout_held_bytes(&proof.authority, &mut winner, size, &sha256)?;
                winner
            }
        }
    };
    let output_metadata = proof.authority.inspect_regular(&output)?;
    let preview = prepare_delivery_preview_for_namespace(&proof.authority, &mut output)?;
    // A private descriptor binds this computed content to the original mask and
    // source proof. It is never exposed as a general index repair capability.
    let verified = VerifiedDeliveryIndexContent {
        authority: &proof.authority, api, scope: &proof.scope, record: &proof.record,
        confirmation: &proof.confirmation, source_path: &output_path, file: &output,
        metadata: output_metadata.clone(),
        terminal_success_count: proof.terminal_success_count,
        derived: Some(DerivedCutoutIndexContent { remote: &proof,
            source: &source, source_metadata: &source_metadata,
            sha256: &sha256, size }),
    };
    let indexed = {
        let _mutation = proof.authority.begin_ordinary_mutation()?;
        proof.index.reconcile_verified_delivery_content(&verified).map_err(anyhow::Error::from)?
    };
    drop(verified);
    proof.derived_cutout = Some(DerivedCutoutDelivery { source, source_metadata,
        output, output_metadata, indexed, source_path: output_path, sha256 });
    proof.ensure_current()?;
    proof.ensure_index_current()?;
    Ok(PreparedNamespaceDelivery { proof, preview })
}

#[derive(Debug, thiserror::Error)]
pub(super) enum DeliveryRetryError {
    #[error("authentication is required")]
    AuthenticationRequired,
    #[error("the generated file has expired")]
    Expired,
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Local(#[from] anyhow::Error),
}

impl DeliveryDownloadKey {
    fn new(session_scope: &SessionScope, client_request_id: &str, file_id: &str) -> Self {
        Self {
            owner_user_id: session_scope.owner_user_id.clone(),
            auth_epoch: session_scope.auth_epoch,
            client_request_id: client_request_id.to_string(),
            file_id: file_id.to_string(),
        }
    }

    fn belongs_to_scope(&self, session_scope: &SessionScope) -> bool {
        self.owner_user_id == session_scope.owner_user_id
            && self.auth_epoch == session_scope.auth_epoch
    }
}

pub(super) fn try_reserve_delivery_download_pairs(
    registry: &GenerationRegistry,
    session_scope: &SessionScope,
    pairs: &[(String, String)],
) -> Option<Vec<DeliveryDownloadReservation>> {
    let keys = pairs
        .iter()
        .map(|(client_request_id, file_id)| {
            DeliveryDownloadKey::new(session_scope, client_request_id, file_id)
        })
        .collect::<BTreeSet<_>>();
    let mut downloads = registry.delivery_downloads.borrow_mut();
    if keys.iter().any(|key| downloads.contains_key(key)) {
        return None;
    }
    if keys.is_empty() {
        return Some(Vec::new());
    }
    let mut reservation_id = registry
        .next_delivery_download_reservation_id
        .get()
        .wrapping_add(1);
    if reservation_id == 0 {
        reservation_id = 1;
    }
    registry
        .next_delivery_download_reservation_id
        .set(reservation_id);
    let reservations = keys
        .into_iter()
        .map(|key| DeliveryDownloadReservation {
            key,
            reservation_id,
        })
        .collect::<Vec<_>>();
    downloads.extend(
        reservations
            .iter()
            .map(|reservation| (reservation.key.clone(), reservation.reservation_id)),
    );
    Some(reservations)
}

pub(super) fn release_delivery_download_reservations_in_registry(
    registry: &GenerationRegistry,
    reservations: &[DeliveryDownloadReservation],
) -> bool {
    let mut downloads = registry.delivery_downloads.borrow_mut();
    let mut released = false;
    for reservation in reservations {
        if downloads.get(&reservation.key) == Some(&reservation.reservation_id) {
            downloads.remove(&reservation.key);
            released = true;
        }
    }
    released
}

pub(super) fn release_delivery_download_reservations(
    context: &AppContext,
    reservations: &[DeliveryDownloadReservation],
) -> bool {
    release_delivery_download_reservations_in_registry(&context.generations, reservations)
}

pub(super) fn complete_delivery_download(
    app: &AppWindow,
    context: &AppContext,
    reservation: &DeliveryDownloadReservation,
) {
    release_delivery_download_reservations(context, std::slice::from_ref(reservation));
    refresh_delivery_download_flags(app, context);
}

pub(super) fn release_delivery_downloads_for_scope(
    context: &AppContext,
    session_scope: &SessionScope,
) -> bool {
    let mut downloads = context.generations.delivery_downloads.borrow_mut();
    let previous_len = downloads.len();
    downloads.retain(|key, _| !key.belongs_to_scope(session_scope));
    downloads.len() != previous_len
}

pub(super) fn refresh_delivery_download_flags(app:&AppWindow,context:&AppContext) {
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    if !persistence.is_current(){return;}
    let work=spawn_delivery_preparation(&persistence,move|captured,_,_|{
        let authority=captured.storage_authority()?;
        let rows=load_pending_generations_for_namespace(&authority)?;
        Ok(rows.into_iter().flat_map(|record|{
            let key=record.client_request_id;
            record.deliveries.into_iter().filter(|delivery|!delivery.failed_asset_id.is_empty())
                .map(move|delivery|(delivery.failed_asset_id,key.clone(),delivery.file_id))
        }).collect::<Vec<_>>())
    });
    let Ok((cancel,receiver))=work else{return;};
    poll_delivery_download_flags(app.as_weak(),context.clone(),persistence,cancel,receiver);
}
fn poll_delivery_download_flags(weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,
    cancel:Arc<std::sync::atomic::AtomicBool>,
    receiver:mpsc::Receiver<std::result::Result<Vec<(String,String,String)>,DeliveryRetryError>>,
){
    slint::Timer::single_shot(Duration::from_millis(50),move||{
        match finish_delivery_preparation(&cancel) {
            Ok(true)=>{poll_delivery_download_flags(weak,context,persistence,cancel,receiver);return;},
            Err(_)=>return,
            Ok(false)=>{},
        }
        // A read failure never turns unknown recovery into an empty success.
        let Ok(Ok(rows))=receiver.try_recv()else{return;};
        let Some(app)=weak.upgrade()else{return;};
        if !retry_binding_current(&context,&persistence){return;}
        let lease=persistence.lease().clone();
        let scope=SessionScope{owner_user_id:lease.namespace.user_public_id().into(),auth_epoch:lease.auth_epoch};
        let _=context.apply_user_completion(&lease,||{
            let flags={
                let downloads=context.generations.delivery_downloads.borrow();
                let store=context.store.borrow();
                store.generations.iter().filter(|asset|asset.source_path=="failed").map(|asset|{
                    let downloading=asset.delivery_recoverable && rows.iter().filter(|(id,_,_)|id==&asset.id)
                        .any(|(_,request,file)|downloads.contains_key(&DeliveryDownloadKey::new(&scope,request,file)));
                    (asset.id.clone(),downloading)
                }).collect::<Vec<_>>()
            };
            for (id,downloading) in flags{set_failed_delivery_downloading(&app,&context,&id,downloading);}
        });
    });
}

fn set_failed_delivery_downloading(
    app: &AppWindow,
    context: &AppContext,
    failed_asset_id: &str,
    downloading: bool,
) {
    let mut store = context.store.borrow_mut();
    if let Some(asset) = store
        .generations
        .iter_mut()
        .find(|asset| asset.id == failed_asset_id && asset.source_path == "failed")
    {
        asset.delivery_downloading = downloading;
    }
    drop(store);
    let state=app.global::<AppState>();
    let update=|model:ModelRc<AssetItem>|{
        for row in 0..model.row_count(){
            if let Some(mut item)=model.row_data(row){
                if item.id.as_str()==failed_asset_id && item.source_path.as_str()=="failed"{
                    item.delivery_downloading=downloading;model.set_row_data(row,item);
                }
            }
        }
    };
    update(state.get_generations());
    for group in state.get_generation_groups().iter(){update(group.items);}
}

fn clear_failed_delivery_recovery(app: &AppWindow, context: &AppContext, failed_asset_id: &str) {
    let mut store = context.store.borrow_mut();
    if let Some(asset) = store
        .generations
        .iter_mut()
        .find(|asset| asset.id == failed_asset_id && asset.source_path == "failed")
    {
        asset.delivery_recoverable = false;
        asset.delivery_downloading = false;
    }
    push_generations(app, &store);
}

pub(super) fn reserve_recovered_delivery_download_pairs(
    registry: &GenerationRegistry,
    record: &PendingGenerationRecord,
) -> Option<Vec<DeliveryDownloadReservation>> {
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    let pairs = record
        .deliveries
        .iter()
        .filter(|delivery| {
            !delivery.failed_asset_id.trim().is_empty()
                && delivery.local_path.trim().is_empty()
                && !delivery.acknowledged
                && !delivery.abandoned
        })
        .map(|delivery| (record.client_request_id.clone(), delivery.file_id.clone()))
        .collect::<Vec<_>>();
    try_reserve_delivery_download_pairs(registry, &session_scope, &pairs)
}

pub(super) fn reserve_recovered_delivery_downloads(
    app: &AppWindow,
    context: &AppContext,
    record: &PendingGenerationRecord,
) -> Option<Vec<DeliveryDownloadReservation>> {
    let pending = record
        .deliveries
        .iter()
        .filter(|delivery| {
            !delivery.failed_asset_id.trim().is_empty()
                && delivery.local_path.trim().is_empty()
                && !delivery.acknowledged
                && !delivery.abandoned
        })
        .collect::<Vec<_>>();
    let reservations = reserve_recovered_delivery_download_pairs(&context.generations, record)?;
    if !pending.is_empty() {
        let failed_ids = pending
            .iter()
            .map(|delivery| delivery.failed_asset_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut store = context.store.borrow_mut();
        for asset in &mut store.generations {
            if failed_ids.contains(asset.id.as_str()) && asset.source_path == "failed" {
                asset.delivery_downloading = true;
            }
        }
        push_generations(app, &store);
    }
    Some(reservations)
}

pub(super) fn select_recoverable_task_file<'a>(
    detail: &'a GenerationTaskDetail,
    delivery: &PendingDeliveryRecord,
) -> std::result::Result<&'a TaskOutputFile, DeliveryRetryError> {
    let file = detail
        .items
        .iter()
        .find(|item| item.index == delivery.item_index && item.status == "succeeded")
        .and_then(|item| item.file.as_ref())
        .filter(|file| file.id == delivery.file_id)
        .filter(|file| file.status == "available")
        .filter(|file| {
            file.download_url
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty())
        })
        .ok_or(DeliveryRetryError::Expired)?;
    let size_bytes = file
        .size_bytes
        .parse::<u64>()
        .map_err(|_| {
            DeliveryRetryError::Api(ApiError::Protocol {
                message: "服务端返回了无效的文件大小".to_string(),
                request_id: None,
            })
        })?;
    if size_bytes != delivery.size_bytes || !file.sha256.eq_ignore_ascii_case(&delivery.sha256) {
        return Err(DeliveryRetryError::Api(ApiError::Protocol {
            message: "生成文件完整性信息与恢复记录不一致".to_string(),
            request_id: None,
        }));
    }
    Ok(file)
}

fn retry_api_error(error: ApiError) -> DeliveryRetryError {
    if matches!(&error, ApiError::Http { status: 404, .. })
        || matches!(
            error.code(),
            Some("generation_task_not_found" | "result_file_not_found" | "result_file_expired")
        )
    {
        DeliveryRetryError::Expired
    } else if matches!(error, ApiError::AuthenticationRequired) {
        DeliveryRetryError::AuthenticationRequired
    } else {
        DeliveryRetryError::Api(error)
    }
}

pub(super) fn pending_delivery_saved_then_acknowledge_with<P, A>(
    persist_saved_delivery: P,
    acknowledge: A,
) -> Result<bool>
where
    P: FnOnce() -> Result<bool>,
    A: FnOnce(),
{
    let saved = persist_saved_delivery()?;
    if saved {
        acknowledge();
    }
    Ok(saved)
}

enum DeliveryCompletionError {
    Local(anyhow::Error),
    Recovery,
}

fn local_save_then_record_and_acknowledge_with<L, P, A>(
    save_local: L,
    persist_saved_delivery: P,
    acknowledge: A,
) -> std::result::Result<(String, bool), DeliveryCompletionError>
where
    L: FnOnce() -> Result<String>,
    P: FnOnce(&str) -> Result<bool>,
    A: FnOnce(),
{
    let source_path = save_local().map_err(DeliveryCompletionError::Local)?;
    let saved = pending_delivery_saved_then_acknowledge_with(
        || persist_saved_delivery(&source_path),
        acknowledge,
    )
    .map_err(|_| DeliveryCompletionError::Recovery)?;
    Ok((source_path, saved))
}

pub(super) struct RetrySuccess {
    pub(super) staged_path: PathBuf,
    pub(super) delivery: DeliveryConfirmation,
}

pub(super) fn run_failed_delivery_retry(
    api: &GenerationApi,
    scope: &SessionScope,
    record: &PendingGenerationRecord,
    delivery: &PendingDeliveryRecord,
) -> std::result::Result<RetrySuccess, DeliveryRetryError> {
    if record.owner_user_id != scope.owner_user_id
        || record.auth_epoch != scope.auth_epoch
        || record.server_task_id.trim().is_empty()
        || delivery.file_id.trim().is_empty()
    {
        return Err(DeliveryRetryError::AuthenticationRequired);
    }
    let detail = api
        .task_scoped(&record.server_task_id, scope)
        .map_err(retry_api_error)?;
    let file = select_recoverable_task_file(&detail, delivery)?;
    let staging_path =
        generation_download_staging_path(&record.client_request_id, delivery.item_index, file);
    cleanup_failed_delivery_staging(&staging_path);
    if let Err(error) = api.download_verified_to_path_scoped(file, scope, &staging_path) {
        cleanup_failed_delivery_staging(&staging_path);
        return Err(retry_api_error(error));
    }
    if let Err(error) = inspect_image_dimensions(&staging_path) {
        cleanup_failed_delivery_staging(&staging_path);
        return Err(DeliveryRetryError::Local(error));
    }
    Ok(RetrySuccess {
        staged_path: staging_path,
        delivery: DeliveryConfirmation {
            client_request_id: record.client_request_id.clone(),
            item_index: delivery.item_index,
            task_id: detail.id,
            file_id: delivery.file_id.clone(),
            sha256: delivery.sha256.clone(),
            size_bytes: delivery.size_bytes,
            failed_asset_id: Some(delivery.failed_asset_id.clone()),
        },
    })
}

pub(super) fn retry_failed_delivery(app: &AppWindow, context: AppContext, failed_asset_id: String) {
    let Some(persistence)=context.store.borrow().private_persistence.clone()else{return;};
    let Some(backend)=context.backend.clone()else{return;};
    if !persistence.is_current(){return;}
    let original=context.store.borrow().generations.iter().find(|asset|asset.id==failed_asset_id
        && asset.source_path=="failed" && asset.delivery_recoverable && !asset.delivery_downloading).cloned();
    if original.is_none(){return;}
    let lease=persistence.lease().clone();
    if context.apply_user_completion(&lease,||{
        set_failed_delivery_downloading(app,&context,&failed_asset_id,true);
        app.global::<AppState>().set_generation_status("正在查找原始交付记录...".into());
    }).is_err(){return;}
    let id=failed_asset_id.clone();
    let work=spawn_delivery_preparation(&persistence,move|captured,_,_|{
        let authority=captured.storage_authority()?;
        recoverable_delivery_for_failed_asset_for_namespace(&authority,&id)?
            .ok_or_else(||DeliveryRetryError::Local(anyhow!("原始交付记录缺失，记录未删除")))
    });
    match work {
        Ok((cancel,receiver))=>poll_retry_discovery(app.as_weak(),context,persistence,backend,failed_asset_id,cancel,receiver),
        Err(_)=>{let _=context.apply_user_completion(&lease,||{
            set_failed_delivery_downloading(app,&context,&failed_asset_id,false);
            app.global::<AppState>().set_generation_status("无法启动下载恢复，原交付已保留".into());
        });}
    }
}
fn retry_binding_current(context:&AppContext,persistence:&PrivatePersistence)->bool {
    context.store.borrow().private_persistence.as_ref().is_some_and(|current|current.same_binding(persistence))
}
fn poll_retry_discovery(weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,
    backend:Arc<BackendRuntime>,failed_asset_id:String,cancel:Arc<std::sync::atomic::AtomicBool>,
    receiver:mpsc::Receiver<std::result::Result<(PendingGenerationRecord,PendingDeliveryRecord),DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50),move||{
        let finished=finish_delivery_preparation(&cancel);
        if matches!(finished,Ok(true)) {
            poll_retry_discovery(weak,context,persistence,backend,failed_asset_id,cancel,receiver);return;
        }
        let result=if let Err(error)=finished {Err(DeliveryRetryError::Local(error))}else{match receiver.try_recv(){
            Ok(result)=>result,
            Err(_)=>Err(DeliveryRetryError::Local(anyhow!("恢复读取线程已断开"))),
        }};
        let Some(app)=weak.upgrade()else{return;};
        if !retry_binding_current(&context,&persistence){return;}
        let lease=persistence.lease().clone();
        let (record,delivery)=match result {
            Ok(pair)=>pair,
            Err(_)=>{let _=context.apply_user_completion(&lease,||{
                set_failed_delivery_downloading(&app,&context,&failed_asset_id,false);
                app.global::<AppState>().set_generation_status("原始交付记录无法读取，数据已保留".into());
            });return;}
        };
        let scope=SessionScope{owner_user_id:lease.namespace.user_public_id().into(),auth_epoch:lease.auth_epoch};
        let pair=(record.client_request_id.clone(),delivery.file_id.clone());
        let reservation=context.apply_user_completion(&lease,||{
            try_reserve_delivery_download_pairs(&context.generations,&scope,std::slice::from_ref(&pair))
                .and_then(|mut values|values.pop())
        }).ok().flatten();
        let Some(reservation)=reservation else {
            let _=context.apply_user_completion(&lease,||set_failed_delivery_downloading(&app,&context,&failed_asset_id,false));
            return;
        };
        let work=spawn_delivery_preparation(&persistence,move|captured,activity,cancel|{
            if activity.is_quiescing() || cancel.load(Ordering::SeqCst){return Err(DeliveryRetryError::AuthenticationRequired);}
            let authority=captured.storage_authority()?;
            let api=GenerationApi::new(backend.api.clone()).with_saved_group(&record.billing_account_group_id);
            let index=authority.delivery_index()?;
            prepare_namespace_delivery(&api,authority,index,&record.identity(),delivery.item_index)
        });
        match work {
            Ok((cancel,receiver))=>poll_namespace_delivery_retry(app.as_weak(),context,persistence,failed_asset_id,reservation,cancel,receiver),
            Err(_)=>{
                release_delivery_download_reservations(&context,std::slice::from_ref(&reservation));
                let _=context.apply_user_completion(&lease,||{
                    set_failed_delivery_downloading(&app,&context,&failed_asset_id,false);
                    app.global::<AppState>().set_generation_status("无法启动原文件下载，记录已保留".into());
                });
            }
        }
    });
}
fn poll_namespace_delivery_retry(
    weak:Weak<AppWindow>,context:AppContext,persistence:PrivatePersistence,failed_asset_id:String,
    reservation:DeliveryDownloadReservation,cancel:Arc<std::sync::atomic::AtomicBool>,
    receiver:mpsc::Receiver<std::result::Result<PreparedNamespaceDelivery,DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50),move||{
        let finished=finish_delivery_preparation(&cancel);
        if matches!(finished,Ok(true)) {
            poll_namespace_delivery_retry(weak,context,persistence,failed_asset_id,reservation,cancel,receiver);return;
        }
        let result=if let Err(error)=finished {Err(DeliveryRetryError::Local(error))}else{match receiver.try_recv(){
            Ok(result)=>result,
            Err(_)=>Err(DeliveryRetryError::Local(anyhow!("delivery retry worker disconnected"))),
        }};
        let Some(app)=weak.upgrade()else{
            release_delivery_download_reservations(&context,std::slice::from_ref(&reservation));return;
        };
        if !retry_binding_current(&context,&persistence){
            release_delivery_download_reservations(&context,std::slice::from_ref(&reservation));return;
        }
        let lease=persistence.lease().clone();
        match result {
            Ok(prepared) if prepared.confirmation().failed_asset_id.as_deref()==Some(failed_asset_id.as_str())=>{
                start_image_delivery_commit(&app,context.clone(),prepared,Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    move|app,result|{
                        release_delivery_download_reservations(&context,std::slice::from_ref(&reservation));
                        set_failed_delivery_downloading(app,&context,&failed_asset_id,false);
                        app.global::<AppState>().set_generation_status(match result {
                            Ok((_,_,true))=>"图片已保存并确认交付",
                            Ok((_,_,false))=>"图片已保存，远端交付确认待重试",
                            Err(_)=>"本地保存尚未确认，原文件和交付记录已保留，请重试",
                        }.into());
                    });
            },
            other=>{
                let terminal=matches!(&other,Err(DeliveryRetryError::Api(error)) if error.is_terminal_session_error());
                drop(other);
                release_delivery_download_reservations(&context,std::slice::from_ref(&reservation));
                let _=context.apply_user_completion(&lease,||{
                    set_failed_delivery_downloading(&app,&context,&failed_asset_id,false);
                    app.global::<AppState>().set_generation_status("图片下载未完成，原交付记录已保留，请重试".into());
                });
                if terminal {
                    let scope=SessionScope{owner_user_id:lease.namespace.user_public_id().into(),auth_epoch:lease.auth_epoch};
                    if terminal_auth_scope_matches_context(&context,&scope){sign_out_locally(&app,&context,true,Some(scope.auth_epoch));}
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_file(id: &str, status: &str) -> TaskOutputFile {
        TaskOutputFile {
            id: id.to_string(),
            status: status.to_string(),
            mime_type: "image/png".to_string(),
            size_bytes: "3".to_string(),
            sha256: "abc".to_string(),
            width: Some(1),
            height: Some(1),
            download_url: Some(format!("https://example.invalid/{id}.png")),
        }
    }

    fn completed_task_with_two_files() -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "task-1".to_string(),
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".to_string(),
            status: "completed".to_string(),
            progress_percent: 100,
            success_count: 2,
            failure_count: 0,
            failure: None,
            prompt: None,
            result_prompt: None,
            request: serde_json::Value::Null,
            model: None,
            quality: "1K".to_string(),
            requested_count: 2,
            task_type: "image_generation".to_string(),
            items: vec![
                GenerationTaskItem {
                    index: 0,
                    status: "succeeded".to_string(),
                    credit_cost: "1".to_string(),
                    failure: None,
                    file: Some(task_file("file-1", "available")),
                },
                GenerationTaskItem {
                    index: 1,
                    status: "succeeded".to_string(),
                    credit_cost: "1".to_string(),
                    failure: None,
                    file: Some(task_file("file-2", "available")),
                },
            ],
        }
    }

    fn delivery(item_index: usize, file_id: &str) -> PendingDeliveryRecord {
        PendingDeliveryRecord {
            item_index,
            file_id: file_id.to_string(),
            sha256: "abc".to_string(),
            size_bytes: 3,
            failed_asset_id: "failed-1".to_string(),
            ..PendingDeliveryRecord::default()
        }
    }

    fn scope(owner_user_id: &str, auth_epoch: u64) -> SessionScope {
        SessionScope {
            owner_user_id: owner_user_id.to_string(),
            auth_epoch,
        }
    }

    fn recoverable_record(scope: &SessionScope) -> PendingGenerationRecord {
        PendingGenerationRecord {
            source_asset_id: String::new(),            video_request: None,
            schema_version: 2,
            cancel_requested: false,
            created_at_epoch_ms: 0,
            client_request_id: "request-a".to_string(),
            owner_user_id: scope.owner_user_id.clone(),
            billing_account_group_id: "22222222-2222-4222-8222-222222222222".to_owned(),
            auth_epoch: scope.auth_epoch,
            local_task_id: "local-task".to_string(),
            server_task_id: "server-task".to_string(),
            raw_prompt: "prompt".to_string(),
            generation_prompt: "prompt".to_string(),
            task_type: "image_generation".to_string(),
            category: "character".to_string(),
            mode: "game".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model_code: "model".to_string(),
            conversation_id: "conversation".to_string(),
            count: 1,
            target_width: 0,
            target_height: 0,
            create_conversation: true,
            reference_paths: Vec::new(),
            reference_sha256: Vec::new(),
            reference_size_bytes: Vec::new(),
            lineage_reference_paths: Vec::new(),
            uploaded_file_ids: Vec::new(),
            deliveries: vec![delivery(0, "file-1")],
            terminal: true,
            expected_success_count: 1,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }

    #[test]
    fn retry_selects_only_the_original_successful_file() {
        let detail = completed_task_with_two_files();
        let selected = select_recoverable_task_file(&detail, &delivery(1, "file-2")).unwrap();

        assert_eq!(selected.id, "file-2");
    }

    #[test]
    fn retry_rejects_a_different_file_at_the_original_item_index() {
        let detail = completed_task_with_two_files();

        assert!(matches!(
            select_recoverable_task_file(&detail, &delivery(1, "file-1")),
            Err(DeliveryRetryError::Expired)
        ));
    }

    #[test]
    fn retry_rejects_an_expired_or_deleted_original_file() {
        for status in ["expired", "deleted"] {
            let mut detail = completed_task_with_two_files();
            detail.items[1].file.as_mut().unwrap().status = status.to_string();

            assert!(matches!(
                select_recoverable_task_file(&detail, &delivery(1, "file-2")),
                Err(DeliveryRetryError::Expired)
            ));
        }
    }

    #[test]
    fn available_file_with_invalid_size_remains_recoverable() {
        let mut detail = completed_task_with_two_files();
        detail.items[1].file.as_mut().unwrap().size_bytes = "invalid".to_string();

        assert!(matches!(
            select_recoverable_task_file(&detail, &delivery(1, "file-2")),
            Err(DeliveryRetryError::Api(ApiError::Protocol { .. }))
        ));
    }

    #[test]
    fn available_file_with_integrity_mismatch_remains_recoverable() {
        let mut mismatched_size = completed_task_with_two_files();
        mismatched_size.items[1].file.as_mut().unwrap().size_bytes = "4".to_string();
        let mut mismatched_sha = completed_task_with_two_files();
        mismatched_sha.items[1].file.as_mut().unwrap().sha256 = "def".to_string();

        for detail in [&mismatched_size, &mismatched_sha] {
            assert!(matches!(
                select_recoverable_task_file(detail, &delivery(1, "file-2")),
                Err(DeliveryRetryError::Api(ApiError::Protocol { .. }))
            ));
        }
    }

    #[test]
    fn delivery_download_key_captures_scope_request_and_file_identity() {
        let scope_a = scope("user-a", 7);
        let scope_b = scope("user-b", 7);
        let newer_scope_a = scope("user-a", 8);
        assert_ne!(
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-1"),
            DeliveryDownloadKey::new(&scope_a, "request-b", "file-1")
        );
        assert_ne!(
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-1"),
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-2")
        );
        assert_ne!(
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-1"),
            DeliveryDownloadKey::new(&scope_b, "request-a", "file-1")
        );
        assert_ne!(
            DeliveryDownloadKey::new(&scope_a, "request-a", "file-1"),
            DeliveryDownloadKey::new(&newer_scope_a, "request-a", "file-1")
        );
    }

    #[test]
    fn delivery_download_pair_reservation_is_atomic_when_one_pair_is_in_flight() {
        let registry = GenerationRegistry::default();
        let scope = scope("user-a", 7);
        let occupied = [("request-a".to_string(), "file-1".to_string())];
        assert!(try_reserve_delivery_download_pairs(&registry, &scope, &occupied).is_some());
        let pairs = vec![
            ("request-a".to_string(), "file-2".to_string()),
            ("request-a".to_string(), "file-1".to_string()),
        ];

        assert!(try_reserve_delivery_download_pairs(&registry, &scope, &pairs).is_none());
        assert!(!registry.delivery_downloads.borrow().contains_key(
            &DeliveryDownloadKey::new(&scope, "request-a", "file-2")
        ));
    }

    #[test]
    fn stopping_an_automatically_recovered_task_releases_only_its_exact_reservations() {
        let context = AppContext::default();
        let scope = scope("user-a", 7);
        let category = "character".to_string();
        let task_pair = [("request-a".to_string(), "file-1".to_string())];
        let unrelated_pair = [("request-a".to_string(), "file-2".to_string())];
        let reservations = try_reserve_delivery_download_pairs(
            &context.generations,
            &scope,
            &task_pair,
        )
        .expect("reserve recovered task delivery");
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope,
            &unrelated_pair,
        )
        .is_some());
        insert_active_generation(
            &context,
            ActiveGeneration {
                task_id: "recovered-task".to_string(),
                client_request_id: Some(task_pair[0].0.clone()),
                category: category.clone(),
                session_scope: scope.clone(),
                delivery_download_reservations: reservations,
                ..ActiveGeneration::default()
            },
        );

        assert!(remove_active_generation(&context, &category, "recovered-task").is_some());

        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope,
            &task_pair,
        )
        .is_some());
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope,
            &unrelated_pair,
        )
        .is_none());
    }

    #[test]
    fn generation_account_teardown_clears_only_the_captured_scope_reservations() {
        let context = AppContext::default();
        let scope_a = scope("user-a", 7);
        let scope_b = scope("user-b", 7);
        let pair = [("request-a".to_string(), "file-1".to_string())];
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope_a,
            &pair,
        )
        .is_some());
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope_b,
            &pair,
        )
        .is_some());

        release_delivery_downloads_for_scope(&context, &scope_a);

        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope_a,
            &pair,
        )
        .is_some());
        assert!(try_reserve_delivery_download_pairs(
            &context.generations,
            &scope_b,
            &pair,
        )
        .is_none());
    }

    #[test]
    fn manual_and_automatic_recovery_mutually_exclude_the_same_scoped_pair() {
        let registry = GenerationRegistry::default();
        let scope = scope("user-a", 7);
        let record = recoverable_record(&scope);
        let pair = [(record.client_request_id.clone(), "file-1".to_string())];

        let automatic = reserve_recovered_delivery_download_pairs(&registry, &record)
            .expect("automatic recovery reserves pair");
        assert!(try_reserve_delivery_download_pairs(&registry, &scope, &pair).is_none());

        release_delivery_download_reservations_in_registry(&registry, &automatic);
        let manual = try_reserve_delivery_download_pairs(&registry, &scope, &pair)
            .expect("manual retry reserves released pair");
        assert!(reserve_recovered_delivery_download_pairs(&registry, &record).is_none());

        release_delivery_download_reservations_in_registry(&registry, &manual);
    }

    #[test]
    fn stale_reservation_cleanup_cannot_release_a_new_reservation_for_the_same_key() {
        let registry = GenerationRegistry::default();
        let scope = scope("user-a", 7);
        let pair = [("request-a".to_string(), "file-1".to_string())];
        let stale = try_reserve_delivery_download_pairs(&registry, &scope, &pair)
            .expect("reserve original worker");
        release_delivery_download_reservations_in_registry(&registry, &stale);
        let current = try_reserve_delivery_download_pairs(&registry, &scope, &pair)
            .expect("reserve replacement worker");

        release_delivery_download_reservations_in_registry(&registry, &stale);

        assert!(try_reserve_delivery_download_pairs(&registry, &scope, &pair).is_none());
        release_delivery_download_reservations_in_registry(&registry, &current);
    }

    #[test]
    fn only_not_found_or_expired_api_errors_abandon_manual_recovery() {
        let not_found = ApiError::Http {
            status: 404,
            code: "generation_task_not_found".to_string(),
            message: "missing".to_string(),
            request_id: None,
            details: None,
        };
        let transient = ApiError::Network {
            message: "offline".to_string(),
            timeout: false,
        };

        assert!(matches!(
            retry_api_error(not_found),
            DeliveryRetryError::Expired
        ));
        assert!(matches!(
            retry_api_error(transient),
            DeliveryRetryError::Api(_)
        ));
    }

    #[test]
    fn acknowledgement_runs_only_after_the_saved_delivery_record_is_durable() {
        let events = RefCell::new(Vec::new());

        let saved = pending_delivery_saved_then_acknowledge_with(
            || {
                events.borrow_mut().push("pending_delivery_saved");
                Ok(true)
            },
            || events.borrow_mut().push("acknowledge"),
        )
        .unwrap();

        assert!(saved);
        assert_eq!(
            events.into_inner(),
            vec!["pending_delivery_saved", "acknowledge"]
        );
    }

    #[test]
    fn persistence_error_never_invokes_the_acknowledgement_hook() {
        let acknowledged = std::cell::Cell::new(false);

        let result = pending_delivery_saved_then_acknowledge_with(
            || Err(anyhow!("recovery file is not durable")),
            || acknowledged.set(true),
        );

        assert!(result.is_err());
        assert!(!acknowledged.get());
    }

    #[test]
    fn local_persistence_error_stops_before_recovery_record_and_acknowledgement() {
        let events = RefCell::new(Vec::new());

        let result = local_save_then_record_and_acknowledge_with(
            || {
                events.borrow_mut().push("local_persistence");
                Err(anyhow!("disk full"))
            },
            |_| {
                events.borrow_mut().push("pending_delivery_saved");
                Ok(true)
            },
            || events.borrow_mut().push("acknowledge"),
        );

        assert!(matches!(result, Err(DeliveryCompletionError::Local(_))));
        assert_eq!(events.into_inner(), vec!["local_persistence"]);
    }
}
