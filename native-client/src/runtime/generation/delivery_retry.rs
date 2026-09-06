use super::*;

macro_rules! ensure_delivery {
    ($condition:expr, $message:expr) => {
        if !$condition {
            return Err(anyhow!($message).into());
        }
    };
}

pub(super) struct PreparedNamespaceDelivery {
    authority: Arc<NamespaceStorageAuthority>,
    api: GenerationApi,
    index: FileIndex,
    scope: SessionScope,
    record: PendingGenerationRecord,
    confirmation: DeliveryConfirmation,
    file: NamespaceManagedFile,
    indexed: ManagedFileRecord,
    preview: PreparedDeliveryPreview,
    source_path: String,
    terminal_success_count: Option<usize>,
}

pub(super) fn prepare_namespace_delivery(
    api: &GenerationApi,
    authority: Arc<NamespaceStorageAuthority>,
    index: FileIndex,
    expected: &RecoveryRecordIdentity,
    item_index: usize,
) -> std::result::Result<PreparedNamespaceDelivery, DeliveryRetryError> {
    let scope = SessionScope {
        owner_user_id: authority.user_public_id().to_owned(),
        auth_epoch: authority.lease().auth_epoch,
    };
    api.ensure_scope_active(&scope)?;
    let record = load_exact_delivery_record(&authority, expected)?;
    ensure_delivery!(
        record.auth_epoch == scope.auth_epoch,
        "delivery session mismatch"
    );
    ensure_delivery!(
        record.count > 0 && item_index < record.count as usize,
        "invalid delivery item index"
    );
    ensure_delivery!(
        record.canvas_source_node_id.is_empty()
            && matches!(
                record.task_type.as_str(),
                "image_generation" | "image_edit" | "image_upscale"
            ),
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
    let extension = match remote.mime_type.as_str() {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };
    let destination = ManagedFileKey::new(
        ManagedUserArea::Output,
        &format!("{}.{extension}", remote.id),
    )?;
    let source_path = authority
        .lease()
        .namespace
        .path(ManagedUserArea::Output)
        .join(destination.relative_name().as_str())
        .to_string_lossy()
        .into_owned();
    if let Some(saved) = exact_saved_delivery(&record, &confirmation)? {
        ensure_delivery!(
            saved.local_path.is_empty() || saved.local_path == source_path,
            "saved delivery output path mismatch"
        );
    }
    let (file, preview) = if let Some(mut file) = authority.open_optional_regular(&destination)? {
        verify_namespace_delivery_file(&authority, &mut file, &confirmation)?;
        let preview = prepare_delivery_preview_for_namespace(&authority, &mut file)?;
        (file, preview)
    } else {
        let mut temporary = authority.create_temporary_regular_for(&destination)?;
        let preparation = (|| -> std::result::Result<_, DeliveryRetryError> {
            api.download_verified_for_namespace(remote, &scope, &authority, &mut temporary)?;
            let preview = prepare_delivery_preview_for_namespace(&authority, &mut temporary)?;
            api.ensure_scope_active(&scope)?;
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
                let preview = prepare_delivery_preview_for_namespace(&authority, &mut winner)?;
                (winner, preview)
            }
        }
    };
    api.ensure_scope_active(&scope)?;
    let retained = authority.inspect_regular(&file)?;
    let registration_file = authority.open_existing_regular(&destination)?;
    ensure_delivery!(
        authority.inspect_regular(&registration_file)?.identity == retained.identity,
        "delivery changed before index registration"
    );
    let registration = NamespacedManagedFileRegistration::new(
        &authority,
        registration_file,
        "generation",
        "durable",
    )
    .map_err(anyhow::Error::from)?;
    let indexed = index
        .register_file_for_namespace(&authority, &registration)
        .map_err(anyhow::Error::from)?;
    let prepared = PreparedNamespaceDelivery {
        authority,
        api: api.clone(),
        index,
        scope,
        record,
        confirmation,
        file,
        indexed,
        preview,
        source_path,
        terminal_success_count: detail.terminal().then_some(successes),
    };
    prepared.ensure_current()?;
    Ok(prepared)
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
    let identity = prepared.record.identity();
    let current = load_exact_delivery_record(&prepared.authority, &identity)?;
    // Delivery rows and terminal progress can advance independently, but the
    // captured task/prompt/input metadata may not silently change underneath us.
    let stable = |record: &PendingGenerationRecord| -> Result<serde_json::Value> {
        let mut record = record.clone();
        record.deliveries.clear();
        record.terminal = false;
        record.expected_success_count = 0;
        Ok(serde_json::to_value(record)?)
    };
    ensure_delivery!(
        stable(&current)? == stable(&prepared.record)?,
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
    pending_delivery_acknowledged_for_namespace(
        &prepared.authority,
        &identity,
        &prepared.confirmation.file_id,
    )
    .map_err(Into::into)
}

impl PreparedNamespaceDelivery {
    pub(super) fn record(&self) -> &PendingGenerationRecord {
        &self.record
    }
    pub(super) fn confirmation(&self) -> &DeliveryConfirmation {
        &self.confirmation
    }
    pub(super) fn preview(&self) -> &PreparedDeliveryPreview {
        &self.preview
    }
    pub(super) fn source_path(&self) -> &str {
        &self.source_path
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

pub(super) fn refresh_delivery_download_flags(app: &AppWindow, context: &AppContext) {
    let Some(scope) = current_generation_session_scope(context) else {
        let mut store = context.store.borrow_mut();
        for asset in &mut store.generations {
            asset.delivery_downloading = false;
        }
        push_generations(app, &store);
        return;
    };
    let downloads = context.generations.delivery_downloads.borrow();
    let mut store = context.store.borrow_mut();
    for asset in &mut store.generations {
        if !asset.delivery_recoverable {
            asset.delivery_downloading = false;
            continue;
        }
        asset.delivery_downloading = recoverable_delivery_for_failed_asset(
            &scope.owner_user_id,
            scope.auth_epoch,
            &asset.id,
        )
        .ok()
        .flatten()
        .is_some_and(|(record, delivery)| {
            downloads.contains_key(&DeliveryDownloadKey::new(
                &scope,
                &record.client_request_id,
                &delivery.file_id,
            ))
        });
    }
    drop(downloads);
    push_generations(app, &store);
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
    push_generations(app, &store);
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
    let state = app.global::<AppState>();
    let Some(backend) = context.backend.clone() else {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let Some(scope) = current_generation_session_scope(&context) else {
        state.set_generation_status("请先登录后再重新下载".into());
        return;
    };
    let (record, delivery) = match recoverable_delivery_for_failed_asset(
        &scope.owner_user_id,
        scope.auth_epoch,
        &failed_asset_id,
    ) {
        Ok(Some(value)) => value,
        Ok(None) => {
            clear_failed_delivery_recovery(app, &context, &failed_asset_id);
            state.set_generation_status("文件已过期，请重新生成".into());
            return;
        }
        Err(_) => {
            state.set_generation_status("本地生成恢复记录无法读取，请重启后重试".into());
            return;
        }
    };
    let pair = (record.client_request_id.clone(), delivery.file_id.clone());
    let Some(mut reservations) = try_reserve_delivery_download_pairs(
        &context.generations,
        &scope,
        std::slice::from_ref(&pair),
    ) else {
        return;
    };
    let reservation = reservations
        .pop()
        .expect("a non-empty delivery pair reserves one key");
    set_failed_delivery_downloading(app, &context, &failed_asset_id, true);
    state.set_generation_status("正在重新下载图片...".into());

    let worker_scope = scope.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        let result = run_failed_delivery_retry(&api, &worker_scope, &record, &delivery);
        let _ = sender.send(result);
    });
    poll_failed_delivery_retry(
        app.as_weak(),
        context,
        scope,
        failed_asset_id,
        reservation,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_failed_delivery_retry(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    scope: SessionScope,
    failed_asset_id: String,
    reservation: DeliveryDownloadReservation,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<RetrySuccess, DeliveryRetryError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let result = {
            let mut receiver = receiver.borrow_mut();
            let Some(channel) = receiver.as_ref() else {
                drop(receiver);
                if let Some(app) = app_weak.upgrade() {
                    complete_delivery_download(&app, &context, &reservation);
                } else {
                    release_delivery_download_reservations(
                        &context,
                        std::slice::from_ref(&reservation),
                    );
                }
                return;
            };
            match channel.try_recv() {
                Ok(result) => {
                    receiver.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    receiver.take();
                    Some(Err(DeliveryRetryError::Local(anyhow!(
                        "delivery retry worker disconnected"
                    ))))
                }
            }
        };
        let Some(result) = result else {
            poll_failed_delivery_retry(
                app_weak,
                context,
                scope,
                failed_asset_id,
                reservation,
                receiver,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            release_delivery_download_reservations(
                &context,
                std::slice::from_ref(&reservation),
            );
            return;
        };
        if !generation_scope_matches_context(&context, &scope) {
            complete_delivery_download(&app, &context, &reservation);
            set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
            return;
        }
        match result {
            Ok(success) => {
                let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
                let delivery_for_persistence = success.delivery.clone();
                let completion = local_save_then_record_and_acknowledge_with(
                    || {
                        replace_failed_delivery_asset_checked(
                            &app,
                            &context.store,
                            &failed_asset_id,
                            &success.staged_path,
                            &time,
                        )
                        .map(|(source_path, _)| source_path)
                    },
                    |source_path| {
                        pending_delivery_saved(
                            &scope.owner_user_id,
                            scope.auth_epoch,
                            &delivery_for_persistence.client_request_id,
                            &delivery_for_persistence,
                            source_path,
                        )
                    },
                    || {
                        acknowledge_delivery_after_local_save(
                            app.as_weak(),
                            context.clone(),
                            scope.clone(),
                            success.delivery,
                        );
                    },
                );
                match completion {
                    Ok((_, true)) => app
                        .global::<AppState>()
                        .set_generation_status("图片下载完成".into()),
                    Ok((_, false)) | Err(DeliveryCompletionError::Recovery) => {
                        app.global::<AppState>().set_generation_status(
                            "图片已保存，但恢复记录更新失败；稍后将继续清理远端文件".into(),
                        );
                    }
                    Err(DeliveryCompletionError::Local(error)) => {
                        cleanup_failed_delivery_staging(&success.staged_path);
                        set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                        app.global::<AppState>().set_generation_status(
                            format!("图片下载失败：{}", zh_error(&error.to_string())).into(),
                        );
                    }
                }
            }
            Err(DeliveryRetryError::Expired) => {
                if matches!(
                    abandon_pending_delivery(
                        &scope.owner_user_id,
                        scope.auth_epoch,
                        &failed_asset_id,
                    ),
                    Ok(true)
                ) {
                    clear_failed_delivery_recovery(&app, &context, &failed_asset_id);
                    app.global::<AppState>()
                        .set_generation_status("文件已过期，请重新生成".into());
                } else {
                    set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                    app.global::<AppState>()
                        .set_generation_status("本地生成恢复记录无法更新，请重启后重试".into());
                }
            }
            Err(DeliveryRetryError::AuthenticationRequired) => {
                set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                app.global::<AppState>()
                    .set_generation_status("登录状态已变化，请重新登录后下载".into());
            }
            Err(DeliveryRetryError::Api(error)) => {
                set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                app.global::<AppState>().set_generation_status(
                    format!("图片下载失败：{}", error.generation_message()).into(),
                );
            }
            Err(DeliveryRetryError::Local(error)) => {
                set_failed_delivery_downloading(&app, &context, &failed_asset_id, false);
                app.global::<AppState>().set_generation_status(
                    format!("图片下载失败：{}", zh_error(&error.to_string())).into(),
                );
            }
        }
        complete_delivery_download(&app, &context, &reservation);
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
            schema_version: 2,
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
