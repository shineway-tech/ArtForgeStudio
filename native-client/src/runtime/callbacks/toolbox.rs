use super::*;

const MAX_COMPRESSION_IMAGES: usize = 50;
const MAX_CONVERSION_IMAGES: usize = 50;
const COLORIZATION_MAX_INPUT_BYTES: u64 = 10 * 1024 * 1024;
const COLORIZATION_MAX_EDGE_EXCLUSIVE: u32 = 3000;
const TOOLBOX_TEMP_FILE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
struct ToolboxReplayPlan {
    session: SessionScope,
    billing_account_group_id: String,
    client_request_id: String,
    task_type: String,
    reference_paths: Vec<String>,
    uploaded_file_ids: Vec<String>,
    server_task_id: String,
}

fn toolbox_replay_plan(
    record: &PendingGenerationRecord,
    expected_type: &str,
) -> Result<ToolboxReplayPlan> {
    anyhow::ensure!(record.task_type == expected_type, "saved toolbox operation changed");
    anyhow::ensure!(!record.owner_user_id.trim().is_empty(), "saved toolbox owner missing");
    anyhow::ensure!(
        !record.billing_account_group_id.trim().is_empty(),
        "saved toolbox payer missing"
    );
    anyhow::ensure!(!record.client_request_id.trim().is_empty(), "saved toolbox key missing");
    Ok(ToolboxReplayPlan {
        session: SessionScope {
            owner_user_id: record.owner_user_id.clone(),
            auth_epoch: record.auth_epoch,
        },
        billing_account_group_id: record.billing_account_group_id.clone(),
        client_request_id: record.client_request_id.clone(),
        task_type: record.task_type.clone(),
        reference_paths: record.reference_paths.clone(),
        uploaded_file_ids: record.uploaded_file_ids.clone(),
        server_task_id: record.server_task_id.clone(),
    })
}

struct ToolboxEffectPermit {
    _persistence: PrivatePersistence,
    _activity: UserActivityPermit,
    _effect: api::OrdinaryBlockingEffectPermit,
}

struct CapturedToolboxWork<T> {
    receiver: mpsc::Receiver<T>,
    worker: ToolboxWorkerTicket,
    pending: Option<T>,
}

struct ToolboxWorkerTicket {
    id: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

struct RegisteredToolboxWorker {
    id: u64,
    lease: NamespaceLease,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

#[derive(Default)]
struct ToolboxWorkerRegistry {
    closing: bool,
    failed: bool,
    workers: Vec<RegisteredToolboxWorker>,
    external_import_requests: BTreeMap<String, u64>,
}

fn toolbox_workers() -> &'static Mutex<ToolboxWorkerRegistry> {
    static WORKERS: std::sync::OnceLock<Mutex<ToolboxWorkerRegistry>> =
        std::sync::OnceLock::new();
    WORKERS.get_or_init(|| Mutex::new(ToolboxWorkerRegistry::default()))
}

fn spawn_toolbox_worker(
    lease: NamespaceLease,
    work: impl FnOnce(Arc<std::sync::atomic::AtomicBool>) + Send + 'static,
) -> Result<ToolboxWorkerTicket> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let mut registry = toolbox_workers()
        .lock()
        .map_err(|_| anyhow!("toolbox worker registry unavailable"))?;
    anyhow::ensure!(!registry.closing, "toolbox shutdown has started");
    anyhow::ensure!(!registry.failed, "a prior toolbox worker failed");
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let handle = std::thread::Builder::new()
        .name(format!("toolbox-{id}"))
        .spawn(move || work(worker_cancel))?;
    registry.workers.push(RegisteredToolboxWorker {
        id,
        lease,
        cancel: Arc::clone(&cancel),
        handle,
    });
    Ok(ToolboxWorkerTicket { id, cancel })
}

fn finish_toolbox_worker_if_ready(id: u64) -> Result<bool> {
    let worker = {
        let mut registry = toolbox_workers()
            .lock()
            .map_err(|_| anyhow!("toolbox worker registry unavailable"))?;
        let Some(index) = registry.workers.iter().position(|worker| worker.id == id) else {
            anyhow::ensure!(!registry.failed, "a toolbox worker failed");
            return Ok(true);
        };
        if !registry.workers[index].handle.is_finished() {
            return Ok(false);
        }
        registry.workers.swap_remove(index)
    };
    if worker.handle.join().is_err() {
        toolbox_workers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .failed = true;
        anyhow::bail!("toolbox worker panicked");
    }
    Ok(true)
}

fn reap_finished_toolbox_workers() -> Result<()> {
    let ids = {
        let registry = toolbox_workers()
            .lock()
            .map_err(|_| anyhow!("toolbox worker registry unavailable"))?;
        registry
            .workers
            .iter()
            .filter(|worker| worker.handle.is_finished())
            .map(|worker| worker.id)
            .collect::<Vec<_>>()
    };
    let mut failed = false;
    for id in ids {
        failed |= finish_toolbox_worker_if_ready(id).is_err();
    }
    anyhow::ensure!(
        !failed
            && !toolbox_workers()
            .lock()
            .map_err(|_| anyhow!("toolbox worker registry unavailable"))?
            .failed,
        "a toolbox worker failed"
    );
    Ok(())
}

fn schedule_toolbox_worker_reap() {
    slint::Timer::single_shot(Duration::from_millis(50), || {
        let _ = reap_finished_toolbox_workers();
        let pending_cancelled = toolbox_workers()
            .lock()
            .map(|registry| {
                registry
                    .workers
                    .iter()
                    .any(|worker| worker.cancel.load(Ordering::Acquire))
            })
            .unwrap_or(false);
        if pending_cancelled {
            schedule_toolbox_worker_reap();
        }
    });
}

pub(super) fn cancel_toolbox_workers_for_lease(lease: &NamespaceLease) {
    let registry = toolbox_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for worker in &registry.workers {
        if &worker.lease == lease {
            worker.cancel.store(true, Ordering::Release);
        }
    }
    drop(registry);
    schedule_toolbox_worker_reap();
}

#[cfg(test)]
pub(super) fn drain_toolbox_workers_for_lease_for_test(lease: &NamespaceLease) -> Result<()> {
    let workers = {
        let mut registry = toolbox_workers()
            .lock()
            .map_err(|_| anyhow!("toolbox worker registry unavailable"))?;
        for worker in &registry.workers {
            if &worker.lease == lease {
                worker.cancel.store(true, Ordering::Release);
            }
        }
        let mut workers = Vec::new();
        let mut index = registry.workers.len();
        while index > 0 {
            index -= 1;
            if &registry.workers[index].lease == lease {
                workers.push(registry.workers.swap_remove(index));
            }
        }
        workers
    };
    let failed = workers
        .into_iter()
        .fold(false, |failed, worker| worker.handle.join().is_err() || failed);
    if failed {
        toolbox_workers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .failed = true;
    }
    anyhow::ensure!(!failed, "toolbox fixture worker failed");
    Ok(())
}

// Shutdown-only: call after the event loop is no longer executing UI callbacks.
pub(super) fn drain_toolbox_workers_for_shutdown() -> Result<()> {
    let workers = {
        let mut registry = toolbox_workers()
            .lock()
            .map_err(|_| anyhow!("toolbox worker registry unavailable"))?;
        registry.closing = true;
        for worker in &registry.workers {
            worker.cancel.store(true, Ordering::Release);
        }
        std::mem::take(&mut registry.workers)
    };
    let mut failed = toolbox_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .failed;
    for worker in workers {
        failed |= worker.handle.join().is_err();
    }
    toolbox_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .failed = failed;
    anyhow::ensure!(!failed, "toolbox worker failed during shutdown");
    Ok(())
}

enum ToolboxAssetCompletion {
    Crop,
}

struct PendingToolboxAssetCommit {
    completion: ToolboxAssetCompletion,
}

enum ToolboxRemoteOutcome {
    Accepted { task_id: String },
    Progress { percent: i32 },
    Prepared(Box<PreparedNamespaceDelivery>),
    CreditInsufficient { message: ApiError },
    Failure { reason: String },
}

impl ToolboxRemoteOutcome {
    fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Prepared(_) | Self::CreditInsufficient { .. } | Self::Failure { .. }
        )
    }
}

#[derive(Clone, Copy)]
enum ToolboxRemoteKind {
    Watermark,
    Colorization,
}

fn validate_toolbox_task_detail(
    record: &PendingGenerationRecord,
    detail: &GenerationTaskDetail,
) -> Result<()> {
    api::require_saved_group(
        &record.billing_account_group_id,
        &detail.billing_account_group_id,
    )
    .map_err(|error| anyhow!(error.to_string()))?;
    anyhow::ensure!(!detail.id.trim().is_empty(), "toolbox task identity missing");
    anyhow::ensure!(
        record.server_task_id.is_empty() || detail.id == record.server_task_id,
        "toolbox task identity changed"
    );
    Ok(())
}

impl<T> CapturedToolboxWork<T> {
    fn new(receiver: mpsc::Receiver<T>, worker: ToolboxWorkerTicket) -> Self {
        Self {
            receiver,
            worker,
            pending: None,
        }
    }

    fn poll_message(&mut self) -> Result<Option<T>> {
        if let Some(value) = self.pending.take() {
            return Ok(Some(value));
        }
        match self.receiver.try_recv() {
            Ok(value) => Ok(Some(value)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                if !finish_toolbox_worker_if_ready(self.worker.id)? {
                    return Ok(None);
                }
                anyhow::bail!("toolbox worker ended without a terminal result");
            }
        }
    }

    fn finish_message(&mut self, value: T) -> Result<Option<T>> {
        if !finish_toolbox_worker_if_ready(self.worker.id)? {
            self.pending = Some(value);
            return Ok(None);
        }
        Ok(Some(value))
    }

    fn poll_terminal_message(&mut self) -> Result<Option<T>> {
        let Some(value) = self.poll_message()? else {
            return Ok(None);
        };
        self.finish_message(value)
    }
}

impl<T> Drop for CapturedToolboxWork<T> {
    fn drop(&mut self) {
        self.worker.cancel.store(true, Ordering::Release);
        schedule_toolbox_worker_reap();
    }
}

fn capture_toolbox_effect(store: &Rc<RefCell<Store>>) -> Option<ToolboxEffectPermit> {
    let persistence = store.borrow().private_persistence.clone()?;
    let (activity, effect) = persistence.begin_effect().ok()?;
    if !store
        .borrow()
        .private_persistence
        .as_ref()
        .is_some_and(|current| current.same_binding(&persistence))
    {
        return None;
    }
    Some(ToolboxEffectPermit {
        _persistence: persistence,
        _activity: activity,
        _effect: effect,
    })
}

fn recapture_toolbox_effect_for_binding(
    store: &Rc<RefCell<Store>>,
    persistence: &PrivatePersistence,
) -> Option<ToolboxEffectPermit> {
    let (activity, effect) = persistence.begin_effect().ok()?;
    if !persistence.is_current()
        || !store
            .borrow()
            .private_persistence
            .as_ref()
            .is_some_and(|current| current.same_binding(persistence))
    {
        return None;
    }
    Some(ToolboxEffectPermit {
        _persistence: persistence.clone(),
        _activity: activity,
        _effect: effect,
    })
}

fn toolbox_binding_is_current(
    store: &Rc<RefCell<Store>>,
    persistence: &PrivatePersistence,
) -> bool {
    persistence.is_current()
        && store
            .borrow()
            .private_persistence
            .as_ref()
            .is_some_and(|current| current.same_binding(persistence))
}

fn apply_toolbox_completion<R>(
    persistence: &PrivatePersistence,
    apply: impl FnOnce() -> R,
) -> Result<R> {
    let _activity = persistence.begin_activity()?;
    persistence
        .upgrade_latch()
        .apply_if_open(apply)
        .map_err(|required| anyhow!(required.as_error().user_message()))
}

fn load_toolbox_preview_for_authority(
    authority: &NamespaceStorageAuthority,
    path: &Path,
    purpose: PreviewPurpose,
) -> Result<(Image, u32, u32, u64)> {
    let bytes = authority.read_image_source(path, 100 * 1024 * 1024)?;
    let (decoded, _) = decode_image_bytes(path, &bytes)?;
    let (width, height) = (decoded.width(), decoded.height());
    let edge = match purpose {
        PreviewPurpose::Reference | PreviewPurpose::Toolbox => 256,
        PreviewPurpose::Gallery => 384,
        PreviewPurpose::Canvas | PreviewPurpose::Showcase => 1024,
        PreviewPurpose::Viewer => 2048,
    };
    let rgba = if width.max(height) > edge {
        decoded.thumbnail(edge, edge)
    } else {
        decoded
    }
    .to_rgba8();
    Ok((
        slint_image_from_rgba(&rgba, rgba.width(), rgba.height()),
        width,
        height,
        bytes.len() as u64,
    ))
}

fn write_toolbox_owned_output(
    authority: &NamespaceStorageAuthority,
    leaf: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    let _mutation = authority.begin_ordinary_mutation()?;
    let destination = ManagedFileKey::new(ManagedUserArea::Output, leaf)?;
    let mut temporary = authority.create_temporary_regular_for(&destination)?;
    authority.write_new_regular_from(&mut temporary, &mut std::io::Cursor::new(bytes))?;
    authority.sync_regular(&mut temporary)?;
    authority.publish_regular(
        &mut temporary,
        NamespaceManagedPublication::Absent(&destination),
    )?;
    Ok(authority.lease().namespace.path(ManagedUserArea::Output).join(leaf))
}

fn persist_toolbox_result_bytes(
    authority: &NamespaceStorageAuthority,
    title: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    let stem = sanitize_filename(&short_text(title, 18));
    let extension = image_extension(bytes);
    write_toolbox_owned_output(
        authority,
        &format!("{}-{}.{}", stem, Uuid::new_v4(), extension),
        bytes,
    )
}

fn unlink_toolbox_owned_output(persistence: &PrivatePersistence, path: &Path) -> Result<bool> {
    let authority = persistence.storage_authority()?;
    unlink_toolbox_owned_output_for_authority(&authority, path)
}

fn unlink_toolbox_owned_output_for_authority(
    authority: &NamespaceStorageAuthority,
    path: &Path,
) -> Result<bool> {
    let _mutation = authority.begin_ordinary_mutation()?;
    let Some(name) = path
        .strip_prefix(authority.lease().namespace.output_dir())
        .ok()
        .and_then(|relative| relative.to_str())
        .filter(|relative| !relative.contains('/'))
    else {
        return Ok(false);
    };
    let key = ManagedFileKey::new(ManagedUserArea::Output, name)?;
    let Some(file) = authority.open_optional_regular(&key)? else { return Ok(false); };
    authority.unlink_regular(file)?;
    Ok(true)
}

fn remember_toolbox_temporary_output(lease: &NamespaceLease, path: &Path) {
    // Retained for caller compatibility. Without a durable consumer graph,
    // registering a path cannot grant later deletion authority.
    let _ = (lease, path);
}

fn unlink_registered_toolbox_temporary_output(
    persistence: &PrivatePersistence,
    path: &Path,
) -> bool {
    // A generated result can immediately become another tool's selected input,
    // and the existing Store has no durable consumption graph. Lease + display
    // path therefore cannot prove deletion authority (the path may also have
    // been replaced with a new inode). Conservatively retain registered results
    // until a future indexed ownership graph can prove they are unreferenced.
    let _ = (persistence, path);
    false
}

fn enqueue_toolbox_asset_projection(
    app: &AppWindow,
    context: &AppContext,
    persistence: &PrivatePersistence,
    item: AssetData,
    notification: NotificationData,
) -> Result<mpsc::Receiver<WriteResult>> {
    let lease = persistence.lease().clone();
    let mut prepared = Some(persistence.prepare_ordered_save()?);
    let queued = context.apply_user_completion(&lease, || {
        let mut store = context.store.borrow_mut();
        store.assets.insert(0, item);
        store.notifications.insert(0, notification);
        prepared
            .take()
            .expect("single guarded toolbox enqueue")
            .enqueue(local_store_data(app, &store))
    });
    drop(prepared);
    match queued {
        Ok(Ok(receiver)) => Ok(receiver),
        Ok(Err(error)) => {
            let message = error.to_string();
            drop(error);
            Err(anyhow!(message))
        }
        Err(error) => {
            let message = error.user_message();
            drop(error);
            Err(anyhow!(message))
        }
    }
}

fn poll_toolbox_asset_ack<E: std::fmt::Display + 'static>(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    persistence: PrivatePersistence,
    receiver: mpsc::Receiver<std::result::Result<(), E>>,
    pending: PendingToolboxAssetCommit,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let saved = match receiver.try_recv() {
            Ok(Ok(())) => true,
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => false,
            Err(TryRecvError::Empty) => {
                poll_toolbox_asset_ack(app_weak, context, persistence, receiver, pending);
                return;
            }
        };
        let Some(_effect) = recapture_toolbox_effect_for_binding(&context.store, &persistence) else {
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let lease = persistence.lease().clone();
        if !saved {
            let _ = context.apply_user_completion(&lease, || {
                let state = app.global::<AppState>();
                match pending.completion {
                    ToolboxAssetCompletion::Crop => {
                        state.set_crop_processing(false);
                        state.set_crop_message("裁剪结果本地保存未确认，请重试".into());
                    }
                }
            });
            return;
        }
        let mut visuals = Some(prepare_delivery_visuals(&app, &context.store.borrow()));
        let mut visual_effects = None;
        let applied = context.apply_user_completion(&lease, || {
            visual_effects = Some(
                visuals
                    .take()
                    .expect("single toolbox delivery projection")
                    .publish_metadata(&app, persistence.clone()),
            );
            let state = app.global::<AppState>();
            match &pending.completion {
                ToolboxAssetCompletion::Crop => {
                    state.set_crop_processing(false);
                    state.set_crop_message(
                        if state.get_language().as_str() == "en" {
                            "Saved to My Assets"
                        } else {
                            "已保存到我的资产"
                        }
                        .into(),
                    );
                }
            }
        });
        if applied.is_err() {
            return;
        }
        if let Some(effects) = visual_effects {
            start_activation_visual_effects(&app, context, effects);
        }
    });
}

fn enqueue_toolbox_remote_delivery(
    app: &AppWindow,
    context: AppContext,
    persistence: PrivatePersistence,
    prepared: PreparedNamespaceDelivery,
    kind: ToolboxRemoteKind,
) -> Result<()> {
    prepared.ensure_current()?;
    anyhow::ensure!(
        persistence.lease() == prepared.lease(),
        "toolbox delivery binding changed"
    );
    let result_path = prepared.source_path().to_owned();
    let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
    start_image_delivery_commit(app, context, prepared, time, move |app, result| {
        let state = app.global::<AppState>();
        match (kind, result) {
            (ToolboxRemoteKind::Watermark, Ok((image, _, true))) => {
                let result_name = Path::new(&result_path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("去水印结果");
                state.set_watermark_result_path(result_path.clone().into());
                state.set_watermark_result_name(result_name.into());
                state.set_watermark_result_image(image);
                state.set_watermark_progress(100);
                state.set_watermark_processing(false);
                state.set_watermark_message("处理完成，已保存到“我的资产 / 其他”".into());
            }
            (ToolboxRemoteKind::Colorization, Ok((image, _, true))) => {
                let result_name = Path::new(&result_path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("上色结果");
                state.set_colorize_result_path(result_path.clone().into());
                state.set_colorize_result_name(result_name.into());
                state.set_colorize_result_image(image);
                state.set_colorize_progress(100);
                state.set_colorize_processing(false);
                state.set_colorize_message("上色完成，已保存到“我的资产 / 其他”".into());
            }
            (ToolboxRemoteKind::Watermark, Ok((_image, _, false))) => {
                state.set_watermark_processing(false);
                state.set_watermark_message("去水印结果已本地保存，服务端确认待恢复".into());
            }
            (ToolboxRemoteKind::Colorization, Ok((_image, _, false))) => {
                state.set_colorize_processing(false);
                state.set_colorize_message("上色结果已本地保存，服务端确认待恢复".into());
            }
            (ToolboxRemoteKind::Watermark, Err(error)) => {
                state.set_watermark_processing(false);
                state.set_watermark_message(
                    format!("去水印结果提交未确认，恢复记录已保留：{}", zh_error(&error.to_string())).into(),
                );
            }
            (ToolboxRemoteKind::Colorization, Err(error)) => {
                state.set_colorize_processing(false);
                state.set_colorize_message(
                    format!("上色结果提交未确认，恢复记录已保留：{}", zh_error(&error.to_string())).into(),
                );
            }
        }
    });
    Ok(())
}

#[derive(Clone, Copy)]
enum ManagedToolboxDirectory {
    CompressionInputs,
    CompressionResults,
    ConversionInputs,
    ConversionResults,
    CropInputs,
}

impl ManagedToolboxDirectory {
    fn name(self) -> &'static str {
        match self {
            Self::CompressionInputs => "compression-inputs",
            Self::CompressionResults => "compression-results",
            Self::ConversionInputs => "conversion-inputs",
            Self::ConversionResults => "conversion-results",
            Self::CropInputs => "crop-inputs",
        }
    }
}

const MANAGED_TOOLBOX_DIRECTORIES: [ManagedToolboxDirectory; 5] = [
    ManagedToolboxDirectory::CompressionInputs,
    ManagedToolboxDirectory::CompressionResults,
    ManagedToolboxDirectory::ConversionInputs,
    ManagedToolboxDirectory::ConversionResults,
    ManagedToolboxDirectory::CropInputs,
];

fn managed_toolbox_directory(data_directory: &Path, directory: ManagedToolboxDirectory) -> PathBuf {
    data_directory.join("toolbox").join(directory.name())
}

fn resolve_safe_managed_toolbox_directory(
    data_directory: &Path,
    directory: ManagedToolboxDirectory,
) -> Option<PathBuf> {
    let toolbox_directory = data_directory.join("toolbox");
    let managed_directory = managed_toolbox_directory(data_directory, directory);
    for candidate in [
        data_directory,
        toolbox_directory.as_path(),
        managed_directory.as_path(),
    ] {
        let metadata = fs::symlink_metadata(candidate).ok()?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return None;
        }
    }
    let canonical_data_directory = fs::canonicalize(data_directory).ok()?;
    let canonical_toolbox_directory = fs::canonicalize(&toolbox_directory).ok()?;
    let canonical_managed_directory = fs::canonicalize(&managed_directory).ok()?;
    if canonical_toolbox_directory.parent() != Some(canonical_data_directory.as_path())
        || canonical_managed_directory.parent() != Some(canonical_toolbox_directory.as_path())
    {
        return None;
    }
    Some(canonical_managed_directory)
}

/// Resolves only regular files that are direct children of an explicitly managed toolbox
/// directory. Every directory boundary and the file itself must be a real filesystem object,
/// never a symbolic link; canonical comparison additionally rejects `..` traversal.
fn resolve_managed_toolbox_file(
    data_directory: &Path,
    directory: ManagedToolboxDirectory,
    path: &Path,
) -> Option<PathBuf> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let managed_directory = resolve_safe_managed_toolbox_directory(data_directory, directory)?;
    let candidate = fs::canonicalize(path).ok()?;
    (candidate.parent() == Some(managed_directory.as_path())).then_some(candidate)
}

fn remove_managed_toolbox_file(
    data_directory: &Path,
    directory: ManagedToolboxDirectory,
    path: &Path,
) -> bool {
    resolve_managed_toolbox_file(data_directory, directory, path)
        .is_some_and(|path| fs::remove_file(path).is_ok())
}

fn remove_toolbox_item_files(
    data_directory: &Path,
    item: &CompressionImageItem,
    input_directory: ManagedToolboxDirectory,
    result_directory: ManagedToolboxDirectory,
) {
    if !item.source_path.trim().is_empty() {
        let _ = remove_managed_toolbox_file(
            data_directory,
            input_directory,
            Path::new(item.source_path.as_str()),
        );
    }
    if !item.result_path.trim().is_empty() {
        let _ = remove_managed_toolbox_file(
            data_directory,
            result_directory,
            Path::new(item.result_path.as_str()),
        );
    }
}

fn copy_and_release_managed_toolbox_result(
    source: &Path,
    destination: &Path,
    data_directory: &Path,
    result_directory: ManagedToolboxDirectory,
) -> std::io::Result<bool> {
    fs::copy(source, destination)?;
    Ok(remove_managed_toolbox_file(
        data_directory,
        result_directory,
        source,
    ))
}

fn clear_released_toolbox_result(
    images: &mut [CompressionImageItem],
    released_result_path: &Path,
) -> bool {
    let mut changed = false;
    for item in images {
        if Path::new(item.result_path.as_str()) == released_result_path {
            item.result_path = "".into();
            changed = true;
        }
    }
    changed
}

fn cleanup_stale_toolbox_files_in(
    data_directory: &Path,
    now: std::time::SystemTime,
    max_age: Duration,
) {
    for directory in MANAGED_TOOLBOX_DIRECTORIES {
        let Some(managed_directory) =
            resolve_safe_managed_toolbox_directory(data_directory, directory)
        else {
            continue;
        };
        let Ok(entries) = fs::read_dir(&managed_directory) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let stale = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > max_age);
            if stale {
                let _ = remove_managed_toolbox_file(data_directory, directory, &entry.path());
            }
        }
    }
}

pub(super) fn cleanup_stale_toolbox_files() {
    cleanup_stale_toolbox_files_in(
        &app_data_dir(),
        std::time::SystemTime::now(),
        TOOLBOX_TEMP_FILE_MAX_AGE,
    );
}

#[derive(Clone)]
struct CompressionInput {
    id: String,
    source_path: String,
}

enum CompressionOutcome {
    Started {
        id: String,
    },
    Completed {
        id: String,
        result_path: String,
        size_text: String,
    },
    Failed {
        id: String,
    },
    Finished {
        succeeded: usize,
        failed: usize,
    },
    Interrupted,
}

enum CompressionSaveOutcome {
    Saved {
        destination: PathBuf,
        released_result_path: Option<PathBuf>,
    },
    Failed,
}

#[derive(Clone)]
struct ConversionInput {
    id: String,
    source_path: String,
}

enum ConversionOutcome {
    Started {
        id: String,
    },
    Completed {
        id: String,
        result_path: String,
        size_text: String,
    },
    Failed {
        id: String,
    },
    Finished {
        succeeded: usize,
        failed: usize,
    },
    Interrupted,
}

enum ConversionSaveOutcome {
    Saved {
        destination: PathBuf,
        released_result_path: Option<PathBuf>,
    },
    Failed,
}

fn set_colorization_source_from_path(app: &AppWindow, path: &Path) -> Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "bmp" | "jpeg" | "jpg" | "png") {
        return Err(anyhow!("unsupported colorization image format"));
    }
    if fs::metadata(path)?.len() > COLORIZATION_MAX_INPUT_BYTES {
        return Err(anyhow!("colorization image exceeds 10 MB"));
    }
    let (width, height) = inspect_image_dimensions(path)?;
    if width >= COLORIZATION_MAX_EDGE_EXCLUSIVE
        || height >= COLORIZATION_MAX_EDGE_EXCLUSIVE
    {
        return Err(anyhow!("colorization image dimensions are unsupported"));
    }
    let image = load_preview_image(path, PreviewPurpose::Canvas)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string();
    let state = app.global::<AppState>();
    state.set_colorize_source_path(path.display().to_string().into());
    state.set_colorize_source_name(name.into());
    state.set_colorize_source_image(image);
    state.set_colorize_result_path("".into());
    state.set_colorize_result_name("".into());
    state.set_colorize_result_image(Image::default());
    state.set_colorize_estimated_credits("20".into());
    state.set_colorize_processing(false);
    state.set_colorize_progress(0);
    state.set_colorize_message("".into());
    Ok(())
}

fn set_colorization_source_for_authority(
    app: &AppWindow,
    persistence: &PrivatePersistence,
    authority: &NamespaceStorageAuthority,
    path: &Path,
) -> Result<()> {
    let extension = path.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase();
    anyhow::ensure!(matches!(extension.as_str(), "bmp" | "jpeg" | "jpg" | "png"), "unsupported colorization image format");
    let (image, width, height, size) =
        load_toolbox_preview_for_authority(authority, path, PreviewPurpose::Canvas)?;
    anyhow::ensure!(size <= COLORIZATION_MAX_INPUT_BYTES, "colorization image exceeds 10 MB");
    anyhow::ensure!(width < COLORIZATION_MAX_EDGE_EXCLUSIVE && height < COLORIZATION_MAX_EDGE_EXCLUSIVE,
        "colorization image dimensions are unsupported");
    let name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default().to_string();
    apply_toolbox_completion(persistence, || {
        let state = app.global::<AppState>();
        state.set_colorize_source_path(path.display().to_string().into());
        state.set_colorize_source_name(name.into());
        state.set_colorize_source_image(image);
        state.set_colorize_result_path("".into());
        state.set_colorize_result_name("".into());
        state.set_colorize_result_image(Image::default());
        state.set_colorize_estimated_credits("20".into());
        state.set_colorize_processing(false);
        state.set_colorize_progress(0);
        state.set_colorize_message("".into());
    })
}

fn set_colorization_source_error(app: &AppWindow, error: &anyhow::Error) {
    let state = app.global::<AppState>();
    let raw = error.to_string();
    let message = if raw.contains("10 MB") {
        if state.get_language().as_str() == "en" {
            "The image must not exceed 10 MB"
        } else {
            "图片大小不能超过 10 MB"
        }
    } else if raw.contains("dimensions") {
        if state.get_language().as_str() == "en" {
            "Both image dimensions must be less than 3000px"
        } else {
            "图片宽高均需小于 3000 像素"
        }
    } else if state.get_language().as_str() == "en" {
        "Choose a supported JPG, PNG or BMP image"
    } else {
        "请选择受支持的 JPG、PNG 或 BMP 图片"
    };
    state.set_colorize_message(message.into());
}

fn set_colorization_source_error_captured(
    app: &AppWindow,
    persistence: &PrivatePersistence,
    error: &anyhow::Error,
) {
    let _ = apply_toolbox_completion(persistence, || {
        set_colorization_source_error(app, error);
    });
}

#[cfg(test)]
thread_local! {
    static TOOLBOX_OPEN_PICKER_FIXTURE: RefCell<Vec<Vec<PathBuf>>> =
        const { RefCell::new(Vec::new()) };
    static TOOLBOX_PICKER_RETURN_HOOK: RefCell<Option<Box<dyn FnOnce()>>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
fn run_toolbox_picker_return_hook() {
    if let Some(hook) = TOOLBOX_PICKER_RETURN_HOOK.with(|slot| slot.borrow_mut().take()) {
        hook();
    }
}

#[cfg(not(test))]
fn choose_toolbox_image_paths() -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new()
        .add_filter("Images", crate::image_formats::picker_image_extensions())
        .pick_files()
}

#[cfg(test)]
fn choose_toolbox_image_paths() -> Option<Vec<PathBuf>> {
    run_toolbox_picker_return_hook();
    TOOLBOX_OPEN_PICKER_FIXTURE.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        (!fixture.is_empty()).then(|| fixture.remove(0))
    })
}

#[cfg(not(test))]
fn choose_toolbox_billing_image_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Images", &["jpg", "jpeg", "png", "bmp"])
        .pick_file()
}

#[cfg(test)]
fn choose_toolbox_billing_image_path() -> Option<PathBuf> {
    run_toolbox_picker_return_hook();
    TOOLBOX_OPEN_PICKER_FIXTURE.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        (!fixture.is_empty())
            .then(|| fixture.remove(0))
            .and_then(|paths| paths.into_iter().next())
    })
}

#[cfg(not(test))]
async fn choose_toolbox_crop_path() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("Images", crate::image_formats::picker_image_extensions())
        .pick_file()
        .await
        .map(|file| file.path().to_path_buf())
}

#[cfg(test)]
async fn choose_toolbox_crop_path() -> Option<PathBuf> {
    run_toolbox_picker_return_hook();
    TOOLBOX_OPEN_PICKER_FIXTURE.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        (!fixture.is_empty())
            .then(|| fixture.remove(0))
            .and_then(|paths| paths.into_iter().next())
    })
}

pub(super) fn wire_toolbox_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_choose_compression_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(entry_effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_compression_processing() || state.get_compression_saving() {
                return;
            }
            let original = entry_effect._persistence.clone();
            drop(entry_effect);
            let Some(paths) = choose_toolbox_image_paths() else {
                return;
            };
            let Some(effect) = recapture_toolbox_effect_for_binding(&action_store, &original) else { return; };
            add_compression_paths_captured(&app, paths, &effect._persistence);
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_add_compression_images_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return false; };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_compression_from_drag_data_captured(
                &app,
                TEXT_PLAIN_MIME,
                data.as_str(),
                &effect._persistence,
            )
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_paste_compression_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return false; };
            paste_compression_image(&app, &effect._persistence)
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_remove_compression_image(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_compression_processing() || state.get_compression_saving() {
                return;
            }
            let id = id.to_string();
            let (removed, images): (Vec<_>, Vec<_>) = state
                .get_compression_images()
                .iter()
                .partition(|item| item.id.as_str() == id);
            for item in &removed {
                let _ = unlink_registered_toolbox_temporary_output(
                    &effect._persistence,
                    Path::new(item.result_path.as_str()),
                );
            }
            let _ = apply_toolbox_completion(&effect._persistence, || {
                set_compression_images(&state, images);
                state.set_compression_message("".into());
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_clear_compression_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_compression_processing() || state.get_compression_saving() {
                return;
            }
            let removed = state.get_compression_images().iter().collect::<Vec<_>>();
            for item in &removed {
                let _ = unlink_registered_toolbox_temporary_output(
                    &effect._persistence,
                    Path::new(item.result_path.as_str()),
                );
            }
            let _ = apply_toolbox_completion(&effect._persistence, || {
                set_compression_images(&state, Vec::new());
                state.set_compression_message("".into());
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let export_context = context.clone();
        let action_store = context.store.clone();
        state.on_save_compression_result(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_compression_processing() || state.get_compression_saving() {
                return;
            }
            let result = state
                .get_compression_images()
                .iter()
                .find(|item| item.id == id)
                .map(|item| {
                    (
                        PathBuf::from(item.result_path.as_str()),
                        item.name.to_string(),
                    )
                });
            let Some((result_path, source_name)) = result else {
                return;
            };
            let available = effect._persistence.storage_authority().and_then(|authority| {
                authority.read_image_source(&result_path, 100 * 1024 * 1024).map(|_| ())
            });
            if available.is_err() {
                let _ = apply_toolbox_completion(&effect._persistence, || {
                    state.set_compression_message(
                        if state.get_language().as_str() == "en" {
                            "No compressed image is available to save"
                        } else {
                            "暂无可保存的压缩结果"
                        }
                        .into(),
                    );
                });
                return;
            }
            start_compression_result_save(
                &app,
                result_path,
                source_name,
                export_context.clone(),
                effect._persistence.clone(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_update_compression_target_preview(move |value| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_compression_processing() || state.get_compression_saving() {
                return;
            }
            let preview = value
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|kilobytes| format!("{:.2}", kilobytes / 1024.0))
                .unwrap_or_else(|| "--".to_string());
            let _ = apply_toolbox_completion(&effect._persistence, || {
                state.set_compression_target_mb(preview.into());
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_start_compression(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(_effect) = capture_toolbox_effect(&action_store) else { return; };
            start_local_compression(&app, action_store.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_choose_conversion_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(entry_effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_conversion_processing() || state.get_conversion_saving() {
                return;
            }
            let original = entry_effect._persistence.clone();
            drop(entry_effect);
            let Some(paths) = choose_toolbox_image_paths() else {
                return;
            };
            let Some(effect) = recapture_toolbox_effect_for_binding(&action_store, &original) else { return; };
            add_conversion_paths_captured(&app, paths, &effect._persistence);
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_add_conversion_images_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return false; };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_conversion_from_drag_data_captured(
                &app,
                TEXT_PLAIN_MIME,
                data.as_str(),
                &effect._persistence,
            )
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_paste_conversion_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return false; };
            paste_conversion_image(&app, &effect._persistence)
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_remove_conversion_image(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_conversion_processing() || state.get_conversion_saving() {
                return;
            }
            let id = id.to_string();
            let (removed, images): (Vec<_>, Vec<_>) = state
                .get_conversion_images()
                .iter()
                .partition(|item| item.id.as_str() == id);
            for item in &removed {
                let _ = unlink_registered_toolbox_temporary_output(
                    &effect._persistence,
                    Path::new(item.result_path.as_str()),
                );
            }
            let _ = apply_toolbox_completion(&effect._persistence, || {
                set_conversion_images(&state, images);
                state.set_conversion_message("".into());
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_clear_conversion_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_conversion_processing() || state.get_conversion_saving() {
                return;
            }
            let removed = state.get_conversion_images().iter().collect::<Vec<_>>();
            for item in &removed {
                let _ = unlink_registered_toolbox_temporary_output(
                    &effect._persistence,
                    Path::new(item.result_path.as_str()),
                );
            }
            let _ = apply_toolbox_completion(&effect._persistence, || {
                set_conversion_images(&state, Vec::new());
                state.set_conversion_message("".into());
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let export_context = context.clone();
        let action_store = context.store.clone();
        state.on_save_conversion_result(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_conversion_saving() {
                return;
            }
            let result = state
                .get_conversion_images()
                .iter()
                .find(|item| item.id == id)
                .map(|item| (item.result_path.to_string(), item.name.to_string()));
            let Some((result_path, source_name)) = result else {
                return;
            };
            let path = PathBuf::from(&result_path);
            let available = effect._persistence.storage_authority().and_then(|authority| {
                authority.read_image_source(&path, 100 * 1024 * 1024).map(|_| ())
            });
            if available.is_err() {
                let _ = apply_toolbox_completion(&effect._persistence, || {
                    state.set_conversion_message(
                        if state.get_language().as_str() == "en" {
                            "No converted image is available yet"
                        } else {
                            "暂无可保存的转换结果"
                        }
                        .into(),
                    );
                });
                return;
            }
            start_conversion_result_save(
                &app,
                path,
                source_name,
                export_context.clone(),
                effect._persistence.clone(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_start_conversion(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(_effect) = capture_toolbox_effect(&action_store) else { return; };
            start_local_conversion(&app, action_store.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_choose_crop_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(entry_effect) = capture_toolbox_effect(&action_store) else { return; };
            if app.global::<AppState>().get_crop_processing() {
                return;
            }
            let original = entry_effect._persistence.clone();
            let original_source = app.global::<AppState>().get_crop_source_path().to_string();
            let app_weak = app.as_weak();
            drop(app);
            drop(entry_effect);
            let action_store = action_store.clone();
            let _ = spawn_toolbox_dialog(async move {
                let Some(path) = choose_toolbox_crop_path().await else {
                    return;
                };
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let Some(effect) = recapture_toolbox_effect_for_binding(&action_store, &original) else { return; };
                if app.global::<AppState>().get_crop_processing()
                    || app.global::<AppState>().get_crop_source_path().as_str()
                        != original_source
                {
                    return;
                }
                add_crop_paths_captured(
                    &app,
                    vec![path],
                    &effect._persistence,
                );
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_add_crop_source_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return false; };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_crop_from_drag_data_captured(
                &app,
                TEXT_PLAIN_MIME,
                data.as_str(),
                &effect._persistence,
            )
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_paste_crop_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return false; };
            paste_crop_image(&app, &effect._persistence)
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_set_crop_ratio(move |ratio| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let _ = apply_toolbox_completion(&effect._persistence, || {
                set_crop_ratio(&app, ratio.as_str());
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_update_crop_rect(move |action, dx, dy, x, y, width, height| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let _ = apply_toolbox_completion(&effect._persistence, || {
                update_crop_rect(&app, action.as_str(), dx, dy, x, y, width, height);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_transform_crop_source(move |action| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            if let Err(error) = transform_crop_source_captured(&app, action.as_str(), &effect._persistence) {
                let _ = apply_toolbox_completion(&effect._persistence, || {
                    set_crop_error(&app, &error);
                });
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_reset_crop_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            if let Err(error) = reset_crop_source_captured(&app, &effect._persistence) {
                let _ = apply_toolbox_completion(&effect._persistence, || {
                    set_crop_error(&app, &error);
                });
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let crop_context = context.clone();
        let action_store = context.store.clone();
        state.on_save_crop_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(_effect) = capture_toolbox_effect(&action_store) else { return; };
            start_crop_save(&app, crop_context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_add_watermark_source_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return false; };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_watermark_from_drag_data_captured(
                &app,
                TEXT_PLAIN_MIME,
                data.as_str(),
                action_store.clone(),
                effect._persistence,
            )
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_choose_watermark_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(entry_effect) = capture_toolbox_effect(&action_store) else { return; };
            if app.global::<AppState>().get_watermark_processing() {
                return;
            }
            let original = entry_effect._persistence.clone();
            drop(entry_effect);
            let Some(path) = choose_toolbox_billing_image_path() else {
                return;
            };
            let Some(effect) = recapture_toolbox_effect_for_binding(&action_store, &original) else { return; };
            if app.global::<AppState>().get_watermark_processing() {
                return;
            }
            let accepted = effect
                ._persistence
                .storage_authority()
                .ok()
                .is_some_and(|authority| set_watermark_source_for_authority(&app, &effect._persistence, &authority, &path));
            if !accepted {
                set_watermark_unsupported_message_captured(&app, &effect._persistence);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let action_store = context.store.clone();
        state.on_start_watermark_removal(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_watermark_source_path().trim().is_empty() {
                let _ = apply_toolbox_completion(&effect._persistence, || {
                    state.set_watermark_message(
                        if state.get_language().as_str() == "en" {
                            "Upload an image first"
                        } else {
                            "请先上传图片"
                        }
                        .into(),
                    );
                });
                return;
            }
            start_watermark_removal(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_start_remove_black_tool(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(_effect) = capture_toolbox_effect(&action_store) else { return; };
            start_remove_black_tool(&app, action_store.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_reveal_watermark_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_watermark_result_path().to_string());
            let available = effect._persistence.storage_authority().and_then(|authority| {
                authority.read_image_source(&path, 100 * 1024 * 1024).map(|_| ())
            });
            if available.is_err() {
                let _ = apply_toolbox_completion(&effect._persistence, || {
                    state.set_watermark_message(
                        if state.get_language().as_str() == "en" {
                            "No processed image is available yet"
                        } else {
                            "暂无可查看的处理结果"
                        }
                        .into(),
                    );
                });
                return;
            }
            let persistence = effect._persistence.clone();
            drop(effect);
            let Some(reveal_effect) = capture_toolbox_effect(&action_store) else { return; };
            let result = reveal_path_in_file_manager(&path);
            drop(reveal_effect);
            let Some(_completion_effect) = capture_toolbox_effect(&action_store) else { return; };
            let _ = apply_toolbox_completion(&persistence, || match result {
                Ok(_) => state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "Opened the image folder"
                    } else {
                        "已打开图片所在文件夹"
                    }
                    .into(),
                ),
                Err(error) => state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        format!("Failed to open the image folder: {error}")
                    } else {
                        format!("打开图片所在文件夹失败：{error}")
                    }
                    .into(),
                ),
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_add_colorize_source_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return false; };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_colorization_from_drag_data_captured(
                &app,
                TEXT_PLAIN_MIME,
                data.as_str(),
                action_store.clone(),
                effect._persistence,
            )
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_choose_colorize_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(entry_effect) = capture_toolbox_effect(&action_store) else { return; };
            if app.global::<AppState>().get_colorize_processing() {
                return;
            }
            let original = entry_effect._persistence.clone();
            drop(entry_effect);
            let Some(path) = choose_toolbox_billing_image_path() else {
                return;
            };
            let Some(effect) = recapture_toolbox_effect_for_binding(&action_store, &original) else { return; };
            if app.global::<AppState>().get_colorize_processing() {
                return;
            }
            let result = effect
                ._persistence
                .storage_authority()
                .and_then(|authority| set_colorization_source_for_authority(&app, &effect._persistence, &authority, &path));
            if let Err(error) = result {
                set_colorization_source_error_captured(&app, &effect._persistence, &error);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let action_store = context.store.clone();
        state.on_start_colorize(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            if state.get_colorize_source_path().trim().is_empty() {
                let _ = apply_toolbox_completion(&effect._persistence, || {
                    state.set_colorize_message(
                        if state.get_language().as_str() == "en" {
                            "Upload an image first"
                        } else {
                            "请先上传图片"
                        }
                        .into(),
                    );
                });
                return;
            }
            start_image_colorization(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let action_store = context.store.clone();
        state.on_reveal_colorize_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(effect) = capture_toolbox_effect(&action_store) else { return; };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_colorize_result_path().to_string());
            let available = effect._persistence.storage_authority().and_then(|authority| {
                authority.read_image_source(&path, 100 * 1024 * 1024).map(|_| ())
            });
            if available.is_err() {
                let _ = apply_toolbox_completion(&effect._persistence, || {
                    state.set_colorize_message(
                        if state.get_language().as_str() == "en" {
                            "No colorized image is available yet"
                        } else {
                            "暂无可查看的上色结果"
                        }
                        .into(),
                    );
                });
                return;
            }
            let persistence = effect._persistence.clone();
            drop(effect);
            let Some(reveal_effect) = capture_toolbox_effect(&action_store) else { return; };
            let result = reveal_path_in_file_manager(&path);
            drop(reveal_effect);
            let Some(_completion_effect) = capture_toolbox_effect(&action_store) else { return; };
            let _ = apply_toolbox_completion(&persistence, || match result {
                Ok(_) => state.set_colorize_message(
                    if state.get_language().as_str() == "en" {
                        "Opened the image folder"
                    } else {
                        "已打开图片所在文件夹"
                    }
                    .into(),
                ),
                Err(error) => state.set_colorize_message(
                    if state.get_language().as_str() == "en" {
                        format!("Failed to open the image folder: {error}")
                    } else {
                        format!("打开图片所在文件夹失败：{error}")
                    }
                    .into(),
                ),
            });
        });
    }
}

pub(super) fn add_colorization_from_drag_data(
    app: &AppWindow,
    mime_type: &str,
    data: &str,
) -> bool {
    let state = app.global::<AppState>();
    if state.get_colorize_processing() {
        state.set_colorize_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while processing"
            } else {
                "处理中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    if let Some(url) = external_image_url(data) {
        start_external_colorization_import(app, url);
        return true;
    }
    if mime_type != URI_LIST_MIME
        && mime_type != TEXT_PLAIN_MIME
        && mime_type != IMAGE_DRAG_MIME
        && mime_type != "text/html"
    {
        return false;
    }
    let paths = drag_data_to_paths(data);
    if paths.is_empty() {
        return false;
    }
    add_colorization_paths(app, paths)
}

#[derive(Clone, Copy)]
enum CapturedExternalToolboxImport {
    Watermark,
    Colorization,
}

impl CapturedExternalToolboxImport {
    fn request_kind(self) -> &'static str {
        match self {
            Self::Watermark => "watermark",
            Self::Colorization => "colorization",
        }
    }
}

fn external_toolbox_import_key(
    lease: &NamespaceLease,
    kind: CapturedExternalToolboxImport,
) -> String {
    format!(
        "{}:{}:{}:{}",
        lease.namespace.user_public_id(),
        lease.auth_epoch,
        lease.namespace_epoch,
        kind.request_kind(),
    )
}

fn begin_external_toolbox_import(
    lease: &NamespaceLease,
    kind: CapturedExternalToolboxImport,
) -> u64 {
    static NEXT_IMPORT_REQUEST: AtomicU64 = AtomicU64::new(1);
    let request_id = NEXT_IMPORT_REQUEST.fetch_add(1, Ordering::Relaxed);
    toolbox_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .external_import_requests
        .insert(external_toolbox_import_key(lease, kind), request_id);
    request_id
}

fn finish_current_external_toolbox_import(
    lease: &NamespaceLease,
    kind: CapturedExternalToolboxImport,
    request_id: u64,
) -> bool {
    let key = external_toolbox_import_key(lease, kind);
    let mut registry = toolbox_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if registry.external_import_requests.get(&key).copied() != Some(request_id) {
        return false;
    }
    registry.external_import_requests.remove(&key);
    true
}

fn add_colorization_from_drag_data_captured(
    app: &AppWindow,
    mime_type: &str,
    data: &str,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
) -> bool {
    if app.global::<AppState>().get_colorize_processing() {
        let _ = apply_toolbox_completion(&persistence, || {
            app.global::<AppState>()
                .set_colorize_message("处理中暂时不能更换图片".into());
        });
        return true;
    }
    if let Some(url) = external_image_url(data) {
        start_captured_external_toolbox_import(
            app,
            store,
            persistence,
            url,
            CapturedExternalToolboxImport::Colorization,
        );
        return true;
    }
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME
        && mime_type != IMAGE_DRAG_MIME && mime_type != "text/html"
    {
        return false;
    }
    let paths = drag_data_to_paths(data);
    if paths.is_empty() { return false; }
    let Ok(authority) = persistence.storage_authority() else { return false; };
    for path in paths {
        if set_colorization_source_for_authority(app, &persistence, &authority, &path).is_ok() { return true; }
    }
    set_colorization_source_error_captured(
        app,
        &persistence,
        &anyhow!("unsupported captured colorization source"),
    );
    true
}

fn start_captured_external_toolbox_import(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
    url: String,
    kind: CapturedExternalToolboxImport,
) {
    let request_id = begin_external_toolbox_import(persistence.lease(), kind);
    let original_source = {
        let state = app.global::<AppState>();
        match kind {
            CapturedExternalToolboxImport::Watermark => {
                state.get_watermark_source_path().to_string()
            }
            CapturedExternalToolboxImport::Colorization => {
                state.get_colorize_source_path().to_string()
            }
        }
    };
    if apply_toolbox_completion(&persistence, || {
        let state = app.global::<AppState>();
        match kind {
            CapturedExternalToolboxImport::Watermark => {
                state.set_watermark_message("正在导入拖入的图片...".into())
            }
            CapturedExternalToolboxImport::Colorization => {
                state.set_colorize_message("正在导入拖入的图片...".into())
            }
        }
    })
    .is_err()
    {
        let _ = finish_current_external_toolbox_import(
            persistence.lease(),
            kind,
            request_id,
        );
        return;
    }
    let (sender, receiver) = mpsc::channel();
    let worker_persistence = persistence.clone();
    let worker = match spawn_toolbox_worker(persistence.lease().clone(), move |cancel| {
        let Ok(activity) = worker_persistence.begin_activity() else { return; };
        let result = (|| -> Result<PathBuf> {
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "toolbox import cancelled");
            let bytes = reference_callbacks::download_captured_reference_bytes(
                &url,
                &worker_persistence,
            )?;
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "toolbox import cancelled");
            let _effect = worker_persistence.begin_effect()?;
            let authority = worker_persistence.storage_authority()?;
            let (decoded, _) =
                decode_image_bytes(Path::new("captured-external-image"), &bytes)?;
            let decoded = match kind {
                CapturedExternalToolboxImport::Watermark => decoded,
                CapturedExternalToolboxImport::Colorization => {
                    flatten_colorization_image(&decoded)
                }
            };
            persist_reference_image_for_namespace(&authority, &decoded)
        })()
        .map_err(|_| "图片未能安全导入；原文件保持不变".to_string());
        drop(activity);
        if !cancel.load(Ordering::Acquire) {
            let _ = sender.send(result);
        }
    }) {
        Ok(worker) => worker,
        Err(_) => {
            let _ = finish_current_external_toolbox_import(
                persistence.lease(),
                kind,
                request_id,
            );
            return;
        }
    };
    poll_captured_external_toolbox_import(
        app.as_weak(),
        store,
        persistence,
        kind,
        request_id,
        original_source,
        Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
    );
}

fn poll_captured_external_toolbox_import(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
    kind: CapturedExternalToolboxImport,
    request_id: u64,
    original_source: String,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<std::result::Result<PathBuf, String>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(work) = slot.as_mut() else { return; };
            match work.poll_terminal_message() {
                Ok(Some(result)) => { slot.take(); Some(result) }
                Ok(None) => None,
                Err(_) => { slot.take(); Some(Err("图片导入任务已中断，请重试".into())) }
            }
        };
        let Some(result) = result else {
            poll_captured_external_toolbox_import(
                app_weak,
                store,
                persistence,
                kind,
                request_id,
                original_source,
                receiver,
            );
            return;
        };
        if !finish_current_external_toolbox_import(
            persistence.lease(),
            kind,
            request_id,
        ) {
            return;
        }
        let Some(_effect) = recapture_toolbox_effect_for_binding(&store, &persistence) else {
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        let current_source = {
            let state = app.global::<AppState>();
            match kind {
                CapturedExternalToolboxImport::Watermark => {
                    if state.get_watermark_processing() {
                        return;
                    }
                    state.get_watermark_source_path().to_string()
                }
                CapturedExternalToolboxImport::Colorization => {
                    if state.get_colorize_processing() {
                        return;
                    }
                    state.get_colorize_source_path().to_string()
                }
            }
        };
        if current_source != original_source {
            return;
        }
        let authority = persistence.storage_authority();
        match (kind, result, authority) {
            (CapturedExternalToolboxImport::Watermark, Ok(path), Ok(authority)) => {
                if !set_watermark_source_for_authority(&app, &persistence, &authority, &path) {
                    set_watermark_unsupported_message_captured(&app, &persistence);
                }
            }
            (CapturedExternalToolboxImport::Colorization, Ok(path), Ok(authority)) => {
                if let Err(error) = set_colorization_source_for_authority(&app, &persistence, &authority, &path) {
                    set_colorization_source_error_captured(&app, &persistence, &error);
                }
            }
            (CapturedExternalToolboxImport::Watermark, Err(error), _) => {
                let _ = apply_toolbox_completion(&persistence, || {
                    app.global::<AppState>().set_watermark_message(error.into());
                });
            }
            (CapturedExternalToolboxImport::Colorization, Err(error), _) => {
                let _ = apply_toolbox_completion(&persistence, || {
                    app.global::<AppState>().set_colorize_message(error.into());
                });
            }
            (CapturedExternalToolboxImport::Watermark, Ok(_), Err(_)) => {
                set_watermark_unsupported_message_captured(&app, &persistence);
            }
            (CapturedExternalToolboxImport::Colorization, Ok(_), Err(error)) => {
                set_colorization_source_error_captured(&app, &persistence, &error);
            }
        }
    });
}

pub(super) fn add_colorization_paths(app: &AppWindow, paths: Vec<PathBuf>) -> bool {
    let state = app.global::<AppState>();
    if state.get_colorize_processing() {
        state.set_colorize_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while processing"
            } else {
                "处理中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    let mut last_error = None;
    for path in paths {
        match set_colorization_source_from_path(app, &path) {
            Ok(()) => return true,
            Err(error) => last_error = Some(error),
        }
    }
    let error = last_error.unwrap_or_else(|| anyhow!("unsupported colorization image format"));
    set_colorization_source_error(app, &error);
    true
}

fn start_external_colorization_import(app: &AppWindow, url: String) {
    let state = app.global::<AppState>();
    state.set_colorize_message(
        if state.get_language().as_str() == "en" {
            "Importing the dropped image..."
        } else {
            "正在导入拖入的图片..."
        }
        .into(),
    );
    let (sender, receiver) = mpsc::channel::<std::result::Result<PathBuf, String>>();
    std::thread::spawn(move || {
        let _ = sender.send(reference_callbacks::download_external_reference(&url));
    });
    poll_external_colorization_import(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
}

fn poll_external_colorization_import(
    app_weak: Weak<AppWindow>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<PathBuf, String>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("图片导入任务已中断，请重试".to_string()))
                }
            }
        };
        let Some(result) = result else {
            poll_external_colorization_import(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match result {
            Ok(path) => {
                add_colorization_paths(&app, vec![path]);
            }
            Err(error) => app.global::<AppState>().set_colorize_message(error.into()),
        }
    });
}

pub(super) fn add_watermark_from_drag_data(app: &AppWindow, mime_type: &str, data: &str) -> bool {
    let state = app.global::<AppState>();
    if state.get_watermark_processing() {
        state.set_watermark_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while processing"
            } else {
                "处理中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    if let Some(url) = external_image_url(data) {
        start_external_watermark_import(app, url);
        return true;
    }
    if mime_type != URI_LIST_MIME
        && mime_type != TEXT_PLAIN_MIME
        && mime_type != IMAGE_DRAG_MIME
        && mime_type != "text/html"
    {
        return false;
    }
    let paths = drag_data_to_paths(data);
    if paths.is_empty() {
        return false;
    }
    add_watermark_paths(app, paths)
}

fn add_watermark_from_drag_data_captured(
    app: &AppWindow,
    mime_type: &str,
    data: &str,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
) -> bool {
    if app.global::<AppState>().get_watermark_processing() {
        let _ = apply_toolbox_completion(&persistence, || {
            app.global::<AppState>()
                .set_watermark_message("处理中暂时不能更换图片".into());
        });
        return true;
    }
    if let Some(url) = external_image_url(data) {
        start_captured_external_toolbox_import(
            app,
            store,
            persistence,
            url,
            CapturedExternalToolboxImport::Watermark,
        );
        return true;
    }
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME
        && mime_type != IMAGE_DRAG_MIME && mime_type != "text/html"
    {
        return false;
    }
    let paths = drag_data_to_paths(data);
    if paths.is_empty() { return false; }
    let Ok(authority) = persistence.storage_authority() else { return false; };
    for path in paths {
        if set_watermark_source_for_authority(app, &persistence, &authority, &path) { return true; }
    }
    set_watermark_unsupported_message_captured(app, &persistence);
    true
}

pub(super) fn add_watermark_paths(app: &AppWindow, paths: Vec<PathBuf>) -> bool {
    let state = app.global::<AppState>();
    if state.get_watermark_processing() {
        state.set_watermark_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while processing"
            } else {
                "处理中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    for path in paths {
        if set_watermark_source_from_path(app, &path) {
            return true;
        }
    }
    set_watermark_unsupported_message(app);
    true
}

fn set_watermark_source_from_path(app: &AppWindow, path: &Path) -> bool {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !canonical.is_file() {
        return false;
    }
    let Ok(image) = load_preview_image(&canonical, PreviewPurpose::Canvas) else {
        return false;
    };
    let name = canonical
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string();
    let state = app.global::<AppState>();
    state.set_watermark_source_path(canonical.display().to_string().into());
    state.set_watermark_source_name(name.into());
    state.set_watermark_source_image(image);
    state.set_watermark_result_path("".into());
    state.set_watermark_result_name("".into());
    state.set_watermark_result_image(Image::default());
    state.set_watermark_processing(false);
    state.set_watermark_progress(0);
    state.set_watermark_estimated_credits("20".into());
    state.set_watermark_message("".into());
    true
}

fn set_watermark_source_for_authority(
    app: &AppWindow,
    persistence: &PrivatePersistence,
    authority: &NamespaceStorageAuthority,
    path: &Path,
) -> bool {
    let Ok((image, _, _, _)) =
        load_toolbox_preview_for_authority(authority, path, PreviewPurpose::Canvas)
    else {
        return false;
    };
    let name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default().to_string();
    apply_toolbox_completion(persistence, || {
        let state = app.global::<AppState>();
        state.set_watermark_source_path(path.display().to_string().into());
        state.set_watermark_source_name(name.into());
        state.set_watermark_source_image(image);
        state.set_watermark_result_path("".into());
        state.set_watermark_result_name("".into());
        state.set_watermark_result_image(Image::default());
        state.set_watermark_processing(false);
        state.set_watermark_progress(0);
        state.set_watermark_estimated_credits("20".into());
        state.set_watermark_message("".into());
    }).is_ok()
}

enum RemoveBlackOutcome {
    Success {
        path: PathBuf,
        name: String,
        rgba: image::RgbaImage,
        width: u32,
        height: u32,
    },
    Failure(String),
}

fn start_remove_black_tool(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let Some(effect) = capture_toolbox_effect(&store) else { return; };
    let persistence = effect._persistence.clone();
    let state = app.global::<AppState>();
    let source = PathBuf::from(state.get_watermark_source_path().to_string());
    let Ok(authority) = persistence.storage_authority() else {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_watermark_message("请先上传图片".into());
        });
        return;
    };
    if apply_toolbox_completion(&persistence, || {
        state.set_watermark_processing(true);
        state.set_watermark_progress(12);
        state.set_watermark_message("正在去黑...".into());
    })
    .is_err()
    {
        return;
    }
    drop(effect);

    let (sender, receiver) = mpsc::channel();
    let worker_persistence = persistence.clone();
    let worker = match spawn_toolbox_worker(persistence.lease().clone(), move |cancel| {
        let outcome = (|| -> Result<RemoveBlackOutcome> {
            let _effect = worker_persistence.begin_effect()?;
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "remove-black cancelled");
            let source_bytes = authority.read_image_source(&source, 100 * 1024 * 1024)?;
            let (decoded, _) = decode_image_bytes(&source, &source_bytes)
                .context("无法读取图片")?;
            let mut rgba = decoded.to_rgba8();
            remove_black_pixels(rgba.as_mut());
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "remove-black cancelled");
            let bytes = encode_png_rgba(&rgba, rgba.width(), rgba.height())?;
            let stem = source
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("image");
            let leaf = format!(
                "toolbox-{}-{}-remove-black.png",
                Local::now().format("%Y%m%d%H%M%S%3f"),
                sanitize_filename(stem)
            );
            let path = write_toolbox_owned_output(&authority, &leaf, &bytes)?;
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string();
            let (width, height) = rgba.dimensions();
            Ok(RemoveBlackOutcome::Success {
                path,
                name,
                rgba,
                width,
                height,
            })
        })()
        .unwrap_or_else(|error| RemoveBlackOutcome::Failure(error.to_string()));
        if !cancel.load(Ordering::Acquire) {
            let _ = sender.send(outcome);
        }
    }) {
        Ok(worker) => worker,
        Err(_) => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_watermark_processing(false);
                state.set_watermark_progress(0);
                state.set_watermark_message("去黑工作线程无法启动".into());
            });
            return;
        }
    };
    poll_remove_black_tool(
        app.as_weak(),
        store,
        persistence,
        Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
    );
}

fn poll_remove_black_tool(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<RemoveBlackOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(work) = slot.as_mut() else { return; };
            match work.poll_terminal_message() {
                Ok(Some(outcome)) => {
                    slot.take();
                    Some(outcome)
                }
                Ok(None) => None,
                Err(error) => {
                    slot.take();
                    Some(RemoveBlackOutcome::Failure(error.to_string()))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_remove_black_tool(app_weak, store, persistence, receiver);
            return;
        };
        let Some(_effect) = capture_toolbox_effect(&store) else { return; };
        if !store
            .borrow()
            .private_persistence
            .as_ref()
            .is_some_and(|current| current.same_binding(&persistence))
        {
            return;
        }
        let Some(app) = app_weak.upgrade() else { return; };
        let _ = apply_toolbox_completion(&persistence, || {
            let state = app.global::<AppState>();
            match outcome {
                RemoveBlackOutcome::Success {
                    path,
                    name,
                    rgba,
                    width,
                    height,
                } => {
                    state.set_watermark_result_path(path.display().to_string().into());
                    state.set_watermark_result_name(name.into());
                    state.set_watermark_result_image(slint_image_from_rgba(&rgba, width, height));
                    state.set_watermark_progress(100);
                    state.set_watermark_message("去黑完成".into());
                }
                RemoveBlackOutcome::Failure(error) => {
                    state.set_watermark_progress(0);
                    state.set_watermark_message(format!("去黑失败：{error}").into());
                }
            }
            state.set_watermark_processing(false);
        });
    });
}

fn set_watermark_unsupported_message(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_watermark_message(
        if state.get_language().as_str() == "en" {
            "The dropped file is not a supported image"
        } else {
            "拖入的文件不是受支持的图片"
        }
        .into(),
    );
}

fn set_watermark_unsupported_message_captured(
    app: &AppWindow,
    persistence: &PrivatePersistence,
) {
    let _ = apply_toolbox_completion(persistence, || {
        set_watermark_unsupported_message(app);
    });
}

fn start_external_watermark_import(app: &AppWindow, url: String) {
    let state = app.global::<AppState>();
    state.set_watermark_message(
        if state.get_language().as_str() == "en" {
            "Importing the dropped image..."
        } else {
            "正在导入拖入的图片..."
        }
        .into(),
    );
    let (sender, receiver) = mpsc::channel::<std::result::Result<PathBuf, String>>();
    std::thread::spawn(move || {
        let _ = sender.send(reference_callbacks::download_external_reference(&url));
    });
    poll_external_watermark_import(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
}

fn poll_external_watermark_import(
    app_weak: Weak<AppWindow>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<PathBuf, String>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("图片导入任务已中断，请重试".to_string()))
                }
            }
        };
        let Some(result) = result else {
            poll_external_watermark_import(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match result {
            Ok(path) => {
                add_watermark_paths(&app, vec![path]);
            }
            Err(error) => app.global::<AppState>().set_watermark_message(error.into()),
        }
    });
}

pub(super) fn add_compression_from_drag_data(app: &AppWindow, mime_type: &str, data: &str) -> bool {
    let state = app.global::<AppState>();
    if state.get_compression_processing() || state.get_compression_saving() {
        return true;
    }
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    let paths = data
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(drag_data_to_path)
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return false;
    }
    add_compression_paths(app, paths);
    true
}

fn add_compression_from_drag_data_captured(
    app: &AppWindow,
    mime_type: &str,
    data: &str,
    persistence: &PrivatePersistence,
) -> bool {
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    let paths = data.lines().map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(drag_data_to_path).collect::<Vec<_>>();
    if paths.is_empty() { return false; }
    add_compression_paths_captured(app, paths, persistence);
    true
}

fn add_compression_paths_captured(
    app: &AppWindow,
    paths: Vec<PathBuf>,
    persistence: &PrivatePersistence,
) {
    let state = app.global::<AppState>();
    if state.get_compression_processing() || state.get_compression_saving() { return; }
    let Ok(authority) = persistence.storage_authority() else { return; };
    let mut images = state.get_compression_images().iter().collect::<Vec<_>>();
    let mut known_paths = images.iter().map(|item| item.source_path.to_string())
        .filter(|path| !path.is_empty()).collect::<BTreeSet<_>>();
    let available = MAX_COMPRESSION_IMAGES.saturating_sub(images.len());
    let mut added = 0usize;
    let mut skipped = paths.len().saturating_sub(available);
    for path in paths.into_iter().take(available) {
        let source_path = path.display().to_string();
        if !path.is_absolute() || !known_paths.insert(source_path.clone()) {
            skipped += 1; continue;
        }
        let Ok((preview, _, _, size)) = load_toolbox_preview_for_authority(&authority, &path, PreviewPurpose::Toolbox) else {
            skipped += 1; continue;
        };
        let name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default().to_string();
        images.push(CompressionImageItem {
            id: Uuid::new_v4().to_string().into(), name: name.into(), source_path: source_path.into(),
            size_text: format_file_size(size).into(), image: preview, status: "pending".into(), result_path: "".into(),
        });
        added += 1;
    }
    let _ = apply_toolbox_completion(persistence, || {
        set_compression_images(&state, images);
        state.set_compression_message(compression_add_message(
            state.get_language().as_str() == "en",
            added,
            skipped,
            state.get_compression_images().row_count(),
        ).into());
    });
}

pub(super) fn add_compression_paths(app: &AppWindow, paths: Vec<PathBuf>) {
    let state = app.global::<AppState>();
    if state.get_compression_processing() || state.get_compression_saving() {
        return;
    }
    let mut images = state.get_compression_images().iter().collect::<Vec<_>>();
    let mut known_paths = images
        .iter()
        .map(|item| item.source_path.to_string())
        .filter(|path| !path.is_empty())
        .collect::<BTreeSet<_>>();
    let available = MAX_COMPRESSION_IMAGES.saturating_sub(images.len());
    let mut added = 0usize;
    let mut skipped = paths.len().saturating_sub(available);

    for path in paths.into_iter().take(available) {
        let canonical = fs::canonicalize(&path).unwrap_or(path);
        let source_path = canonical.display().to_string();
        if !canonical.is_file() || !known_paths.insert(source_path.clone()) {
            skipped += 1;
            continue;
        }
        if compression_source_extension(&canonical).is_err() {
            skipped += 1;
            continue;
        }
        let Ok(preview) = load_preview_image(&canonical, PreviewPurpose::Toolbox) else {
            skipped += 1;
            continue;
        };
        let name = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        let size = fs::metadata(&canonical)
            .map(|metadata| format_file_size(metadata.len()))
            .unwrap_or_default();
        images.push(CompressionImageItem {
            id: Uuid::new_v4().to_string().into(),
            name: name.into(),
            source_path: source_path.into(),
            size_text: size.into(),
            image: preview,
            status: "pending".into(),
            result_path: "".into(),
        });
        added += 1;
    }

    set_compression_images(&state, images);
    state.set_compression_message(
        compression_add_message(
            state.get_language().as_str() == "en",
            added,
            skipped,
            state.get_compression_images().row_count(),
        )
        .into(),
    );
}

enum ToolboxClipboardContent {
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Text(String),
}

#[cfg(not(test))]
fn read_toolbox_clipboard() -> Result<ToolboxClipboardContent> {
    let mut clipboard = arboard::Clipboard::new()?;
    if let Ok(image) = clipboard.get_image() {
        return Ok(ToolboxClipboardContent::Image {
            width: image.width as u32,
            height: image.height as u32,
            rgba: image.bytes.into_owned(),
        });
    }
    Ok(ToolboxClipboardContent::Text(clipboard.get_text()?))
}

#[cfg(test)]
thread_local! {
    static TOOLBOX_CLIPBOARD_FIXTURE: RefCell<Option<ToolboxClipboardContent>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
fn read_toolbox_clipboard() -> Result<ToolboxClipboardContent> {
    TOOLBOX_CLIPBOARD_FIXTURE
        .with(|fixture| fixture.borrow_mut().take())
        .ok_or_else(|| anyhow!("toolbox clipboard fixture missing"))
}

fn paste_compression_image(app: &AppWindow, persistence: &PrivatePersistence) -> bool {
    let state = app.global::<AppState>();
    if state.get_compression_processing() || state.get_compression_saving() {
        return true;
    }
    if state.get_compression_images().row_count() >= MAX_COMPRESSION_IMAGES {
        let _ = apply_toolbox_completion(persistence, || {
            state.set_compression_message(
                compression_limit_message(state.get_language().as_str() == "en").into(),
            );
        });
        return true;
    }
    let Ok(content) = read_toolbox_clipboard() else { return false; };
    match content {
        ToolboxClipboardContent::Image { width, height, rgba } => {
            let Some(rgba) = image::RgbaImage::from_raw(width, height, rgba) else {
                return false;
            };
            let Ok(bytes) = encode_png_rgba(&rgba, rgba.width(), rgba.height()) else {
                return false;
            };
            let Ok(authority) = persistence.storage_authority() else { return false; };
            let leaf = format!("toolbox-compression-pasted-{}.png", Uuid::new_v4());
            let Ok(path) = write_toolbox_owned_output(&authority, &leaf, &bytes) else { return false; };
            add_compression_paths_captured(app, vec![path], persistence);
            true
        }
        ToolboxClipboardContent::Text(text) => {
            add_compression_from_drag_data_captured(app, TEXT_PLAIN_MIME, &text, persistence)
        }
    }
}

fn set_compression_images(state: &AppState, images: Vec<CompressionImageItem>) {
    let has_results = images
        .iter()
        .any(|item| item.status.as_str() == "completed" && !item.result_path.is_empty());
    state.set_compression_images(ModelRc::new(VecModel::from(images)));
    state.set_compression_has_results(has_results);
}

fn start_local_compression(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let Some(persistence) = store.borrow().private_persistence.clone() else { return; };
    let Ok(authority) = persistence.storage_authority() else { return; };
    let state = app.global::<AppState>();
    if state.get_compression_processing() || state.get_compression_saving() {
        return;
    }
    let mode = match state.get_compression_mode().as_str() {
        "quality" => {
            ImageCompressionMode::Quality(state.get_compression_quality().clamp(1, 100) as u8)
        }
        "size" => {
            let Some(target_bytes) = state
                .get_compression_target_kb()
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .and_then(|value| value.checked_mul(1024))
            else {
                let _ = apply_toolbox_completion(&persistence, || {
                    state.set_compression_message(
                        if state.get_language().as_str() == "en" {
                            "Enter a valid target size"
                        } else {
                            "请输入有效的目标文件大小"
                        }
                        .into(),
                    );
                });
                return;
            };
            ImageCompressionMode::TargetBytes(target_bytes)
        }
        _ => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_compression_message(
                    if state.get_language().as_str() == "en" {
                        "Choose a supported compression mode"
                    } else {
                        "请选择受支持的压缩方式"
                    }
                    .into(),
                );
            });
            return;
        }
    };

    let mut images = state.get_compression_images().iter().collect::<Vec<_>>();
    if images.is_empty() {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_compression_message(
                if state.get_language().as_str() == "en" {
                    "Add at least one image first"
                } else {
                    "请先添加需要压缩的图片"
                }
                .into(),
            );
        });
        return;
    }
    let inputs = images
        .iter()
        .map(|item| CompressionInput {
            id: item.id.to_string(),
            source_path: item.source_path.to_string(),
        })
        .collect::<Vec<_>>();
    let abandoned_results = images
        .iter()
        .filter_map(|item| {
            (!item.result_path.trim().is_empty())
                .then(|| PathBuf::from(item.result_path.as_str()))
        })
        .collect::<Vec<_>>();
    for item in &mut images {
        item.status = "pending".into();
        item.result_path = "".into();
    }
    if apply_toolbox_completion(&persistence, || {
        set_compression_images(&state, images);
        state.set_compression_processing(true);
        state.set_compression_message(
            if state.get_language().as_str() == "en" {
                "Compressing images locally..."
            } else {
                "正在压缩中..."
            }
            .into(),
        );
    })
    .is_err()
    {
        return;
    }
    if store
        .borrow()
        .private_persistence
        .as_ref()
        .is_some_and(|current| current.same_binding(&persistence))
    {
        for result_path in abandoned_results {
            let _ = unlink_registered_toolbox_temporary_output(&persistence, &result_path);
        }
    }

    let (sender, receiver) = mpsc::channel::<CompressionOutcome>();
    let worker_persistence = persistence.clone();
    let worker = match spawn_toolbox_worker(persistence.lease().clone(), move |cancel| {
        let Ok(_effect) = worker_persistence.begin_effect() else {
            let _ = sender.send(CompressionOutcome::Interrupted);
            return;
        };
        run_captured_local_compression_worker(authority, inputs, mode, cancel, sender);
    }) {
        Ok(worker) => worker,
        Err(_) => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_compression_processing(false);
                state.set_compression_message("本地压缩工作线程无法启动".into());
            });
            return;
        }
    };
    poll_local_compression(
        app.as_weak(),
        store,
        persistence,
        Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
    );
}

fn run_captured_local_compression_worker(
    authority: Arc<NamespaceStorageAuthority>,
    inputs: Vec<CompressionInput>,
    mode: ImageCompressionMode,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    sender: mpsc::Sender<CompressionOutcome>,
) {
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    for input in inputs {
        if cancel.load(Ordering::Acquire) { return; }
        if sender.send(CompressionOutcome::Started { id: input.id.clone() }).is_err() {
            return;
        }
        let result = (|| -> Result<(PathBuf, String)> {
            let source = Path::new(&input.source_path);
            let bytes = authority.read_image_source(source, 100 * 1024 * 1024)?;
            let compressed = compress_image_bytes(source, &bytes, mode.clone())?;
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "compression cancelled");
            let source_stem = source.file_stem().and_then(|value| value.to_str()).unwrap_or("image");
            let leaf = format!(
                "{}-{}.{}",
                sanitize_filename(source_stem),
                Uuid::new_v4(),
                compressed.extension
            );
            let destination = write_toolbox_owned_output(&authority, &leaf, &compressed.bytes)?;
            if cancel.load(Ordering::Acquire) {
                let _ = unlink_toolbox_owned_output_for_authority(&authority, &destination);
                anyhow::bail!("compression cancelled");
            }
            remember_toolbox_temporary_output(authority.lease(), &destination);
            Ok((destination, format_file_size(compressed.bytes.len() as u64)))
        })();
        match result {
            Ok((result_path, size_text)) => {
                succeeded += 1;
                if sender.send(CompressionOutcome::Completed {
                    id: input.id,
                    result_path: result_path.display().to_string(),
                    size_text,
                }).is_err() {
                    return;
                }
            }
            Err(_) => {
                failed += 1;
                if sender.send(CompressionOutcome::Failed { id: input.id }).is_err() {
                    return;
                }
            }
        }
    }
    let _ = sender.send(CompressionOutcome::Finished { succeeded, failed });
}

fn run_local_compression_worker(
    inputs: Vec<CompressionInput>,
    mode: ImageCompressionMode,
    output_dir: PathBuf,
    sender: mpsc::Sender<CompressionOutcome>,
) {
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    if fs::create_dir_all(&output_dir).is_err() {
        for input in inputs {
            failed += 1;
            let _ = sender.send(CompressionOutcome::Failed { id: input.id });
        }
        let _ = sender.send(CompressionOutcome::Finished { succeeded, failed });
        return;
    }

    for input in inputs {
        if sender
            .send(CompressionOutcome::Started {
                id: input.id.clone(),
            })
            .is_err()
        {
            return;
        }
        let result = (|| -> Result<(PathBuf, String)> {
            let compressed = compress_image_file(Path::new(&input.source_path), mode.clone())?;
            let source_stem = Path::new(&input.source_path)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("image");
            let destination = output_dir.join(format!(
                "{}-{}.{}",
                sanitize_filename(source_stem),
                Uuid::new_v4(),
                compressed.extension
            ));
            atomic_write_file(&destination, &compressed.bytes)?;
            Ok((destination, format_file_size(compressed.bytes.len() as u64)))
        })();
        match result {
            Ok((result_path, size_text)) => {
                succeeded += 1;
                if sender
                    .send(CompressionOutcome::Completed {
                        id: input.id,
                        result_path: result_path.display().to_string(),
                        size_text,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(_) => {
                failed += 1;
                if sender
                    .send(CompressionOutcome::Failed { id: input.id })
                    .is_err()
                {
                    return;
                }
            }
        }
    }
    let _ = sender.send(CompressionOutcome::Finished { succeeded, failed });
}

fn update_compression_item(
    state: &AppState,
    id: &str,
    status: &str,
    result_path: Option<&str>,
    size_text: Option<&str>,
) {
    let mut images = state.get_compression_images().iter().collect::<Vec<_>>();
    let Some(item) = images.iter_mut().find(|item| item.id.as_str() == id) else {
        return;
    };
    item.status = status.into();
    if let Some(result_path) = result_path {
        item.result_path = result_path.into();
    }
    if let Some(size_text) = size_text {
        item.size_text = size_text.into();
    }
    set_compression_images(state, images);
}

fn poll_local_compression(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<CompressionOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_mut() else {
                return;
            };
            match rx.poll_message() {
                Ok(Some(outcome @ CompressionOutcome::Finished { .. }))
                | Ok(Some(outcome @ CompressionOutcome::Interrupted)) => {
                    match rx.finish_message(outcome) {
                        Ok(Some(outcome)) => {
                            slot.take();
                            Some(outcome)
                        }
                        Ok(None) => None,
                        Err(_) => {
                            slot.take();
                            Some(CompressionOutcome::Interrupted)
                        }
                    }
                }
                Ok(Some(outcome)) => Some(outcome),
                Ok(None) => None,
                Err(_) => { slot.take(); Some(CompressionOutcome::Interrupted) }
            }
        };
        let Some(outcome) = outcome else {
            poll_local_compression(app_weak, store, persistence, receiver);
            return;
        };
        let Some(_effect) = recapture_toolbox_effect_for_binding(&store, &persistence) else {
            receiver.borrow_mut().take();
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        if apply_toolbox_completion(&persistence, || match outcome {
            CompressionOutcome::Started { id } => {
                update_compression_item(&state, &id, "processing", None, None);
            }
            CompressionOutcome::Completed {
                id,
                result_path,
                size_text,
            } => {
                update_compression_item(
                    &state,
                    &id,
                    "completed",
                    Some(&result_path),
                    Some(&size_text),
                );
            }
            CompressionOutcome::Failed { id } => {
                update_compression_item(&state, &id, "failed", Some(""), None);
            }
            CompressionOutcome::Finished { succeeded, failed } => {
                keep_polling = false;
                state.set_compression_processing(false);
                state.set_compression_message(
                    if state.get_language().as_str() == "en" {
                        if failed == 0 {
                            format!("Compressed {succeeded} image(s). Click Save to export.")
                        } else {
                            format!(
                                "Compressed {succeeded} image(s); {failed} failed. Save completed results."
                            )
                        }
                    } else if failed == 0 {
                        format!("已完成 {succeeded} 张图片压缩，点击“保存”导出到本地")
                    } else {
                        format!("已压缩 {succeeded} 张，失败 {failed} 张；可保存已完成的结果")
                    }
                    .into(),
                );
            }
            CompressionOutcome::Interrupted => {
                keep_polling = false;
                let mut images = state.get_compression_images().iter().collect::<Vec<_>>();
                for item in &mut images {
                    if matches!(item.status.as_str(), "pending" | "processing") {
                        item.status = "failed".into();
                    }
                }
                set_compression_images(&state, images);
                state.set_compression_processing(false);
                state.set_compression_message(
                    if state.get_language().as_str() == "en" {
                        "The local compression task was interrupted"
                    } else {
                        "本地压缩任务意外中断，请重试"
                    }
                    .into(),
                );
            }
        }).is_err() {
            receiver.borrow_mut().take();
            return;
        }
        if keep_polling {
            poll_local_compression(app_weak, store, persistence, receiver);
        }
    });
}

#[cfg(test)]
thread_local! {
    static TOOLBOX_EXPORT_FIXTURE: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

#[cfg(not(test))]
fn spawn_toolbox_dialog(future: impl std::future::Future<Output = ()> + 'static) -> std::result::Result<(), slint::EventLoopError> {
    slint::spawn_local(future).map(|_| ())
}

#[cfg(test)]
fn spawn_toolbox_dialog(future: impl std::future::Future<Output = ()> + 'static) -> std::result::Result<(), slint::EventLoopError> {
    // The per-thread test backend deliberately has no event-loop proxy. Its
    // final dialog seam is immediately ready; poll the actual callback future,
    // rather than silently accepting NoEventLoopProvider and testing no work.
    struct DialogWake;
    impl std::task::Wake for DialogWake { fn wake(self: Arc<Self>) {} }
    // Like spawn_local, enter only after the originating callback has released
    // its ordinary effect permits. The dialog may re-enter the event loop.
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let waker = std::task::Waker::from(Arc::new(DialogWake));
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = Box::pin(future);
        assert!(matches!(future.as_mut().poll(&mut context), std::task::Poll::Ready(())),
            "test dialog must resolve at the explicit final OS seam");
    });
    Ok(())
}

#[cfg(not(test))]
async fn choose_toolbox_export_path(
    title: &str,
    default_name: String,
    filter_name: String,
    extension: &str,
) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .set_file_name(default_name)
        .add_filter(filter_name, &[extension])
        .save_file()
        .await
        .map(|file| file.path().to_path_buf())
}

#[cfg(test)]
async fn choose_toolbox_export_path(
    _title: &str,
    _default_name: String,
    _filter_name: String,
    _extension: &str,
) -> Option<PathBuf> {
    run_toolbox_picker_return_hook();
    TOOLBOX_EXPORT_FIXTURE.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        (!fixture.is_empty()).then(|| fixture.remove(0))
    })
}

fn start_compression_result_save(
    app: &AppWindow,
    result_path: PathBuf,
    source_name: String,
    context: AppContext,
    persistence: PrivatePersistence,
) {
    let Some(root) = context.data_root_capability.clone() else { return; };
    let extension = result_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let source_stem = Path::new(&source_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let default_name = format!(
        "{}-compressed.{}",
        sanitize_filename(source_stem),
        extension
    );
    let filter_name = format!("{} Image", extension.to_ascii_uppercase());
    let app_weak = app.as_weak();
    let _ = spawn_toolbox_dialog(async move {
        let Some(selected_path) = choose_toolbox_export_path(
            "保存压缩结果",
            default_name,
            filter_name,
            &extension,
        )
        .await
        else {
            return;
        };
        let destination = normalize_compression_destination(&selected_path, &extension);
        let Some(parent) = destination.parent().map(Path::to_path_buf) else { return; };
        let Some(name) = destination.file_name().and_then(|value| value.to_str()).map(str::to_owned) else { return; };
        let Some(effect) = recapture_toolbox_effect_for_binding(&context.store, &persistence) else { return; };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if apply_toolbox_completion(&effect._persistence, || {
            state.set_compression_saving(true);
            state.set_compression_message(
            if state.get_language().as_str() == "en" {
                "Saving the compressed image..."
            } else {
                "正在保存压缩结果..."
            }
            .into(),
            );
        }).is_err() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let worker_persistence = persistence.clone();
        let worker_destination = destination.clone();
        let worker = match spawn_toolbox_worker(persistence.lease().clone(), move |cancel| {
            let outcome = (|| -> Result<CompressionSaveOutcome> {
                let _effect = worker_persistence.begin_effect()?;
                anyhow::ensure!(!cancel.load(Ordering::Acquire), "compression export cancelled");
                let authority = worker_persistence.storage_authority()?;
                let bytes = authority.read_image_source(&result_path, 100 * 1024 * 1024)?;
                anyhow::ensure!(!cancel.load(Ordering::Acquire), "compression export cancelled");
                let destination_capability = ExternalExportDestination::open(&root, &parent)?;
                destination_capability.write_new_file(&name, &mut Cursor::new(bytes))?;
                anyhow::ensure!(!cancel.load(Ordering::Acquire), "compression export cancelled");
                let released = unlink_registered_toolbox_temporary_output(&worker_persistence, &result_path)
                    .then_some(result_path);
                Ok(CompressionSaveOutcome::Saved {
                    destination: worker_destination,
                    released_result_path: released,
                })
            })()
            .unwrap_or(CompressionSaveOutcome::Failed);
            if !cancel.load(Ordering::Acquire) {
                let _ = sender.send(outcome);
            }
        }) {
            Ok(worker) => worker,
            Err(_) => {
                let _ = apply_toolbox_completion(&persistence, || {
                    state.set_compression_saving(false);
                    state.set_compression_message("压缩结果保存线程无法启动".into());
                });
                return;
            }
        };
        poll_compression_result_save(
            app.as_weak(),
            context.store.clone(),
            persistence,
            Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
        );
    });
}

fn normalize_compression_destination(path: &Path, extension: &str) -> PathBuf {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    {
        path.to_path_buf()
    } else {
        path.with_extension(extension)
    }
}

fn poll_compression_result_save(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<CompressionSaveOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_mut() else {
                return;
            };
            match rx.poll_terminal_message() {
                Ok(Some(outcome)) => { slot.take(); Some(outcome) }
                Ok(None) => None,
                Err(_) => { slot.take(); Some(CompressionSaveOutcome::Failed) }
            }
        };
        let Some(outcome) = outcome else {
            poll_compression_result_save(app_weak, store, persistence, receiver);
            return;
        };
        let Some(_effect) = recapture_toolbox_effect_for_binding(&store, &persistence) else {
            receiver.borrow_mut().take();
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_compression_saving(false);
            match outcome {
            CompressionSaveOutcome::Saved {
                destination,
                released_result_path,
            } => {
                if let Some(released_result_path) = released_result_path {
                    let mut images = state.get_compression_images().iter().collect::<Vec<_>>();
                    if clear_released_toolbox_result(&mut images, &released_result_path) {
                        set_compression_images(&state, images);
                    }
                }
                state.set_compression_message(
                    if state.get_language().as_str() == "en" {
                        format!("Saved to {}", destination.display())
                    } else {
                        format!("已保存到 {}", destination.display())
                    }
                    .into(),
                );
            }
            CompressionSaveOutcome::Failed => state.set_compression_message(
                if state.get_language().as_str() == "en" {
                    "The compressed image could not be saved"
                } else {
                    "压缩结果保存失败，请重试"
                }
                .into(),
            ),
            }
        });
    });
}

pub(super) fn add_conversion_from_drag_data(app: &AppWindow, mime_type: &str, data: &str) -> bool {
    let state = app.global::<AppState>();
    if state.get_conversion_processing() || state.get_conversion_saving() {
        return true;
    }
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    let paths = data
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(drag_data_to_path)
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return false;
    }
    add_conversion_paths(app, paths);
    true
}

fn add_conversion_from_drag_data_captured(
    app: &AppWindow,
    mime_type: &str,
    data: &str,
    persistence: &PrivatePersistence,
) -> bool {
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    let paths = data.lines().map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(drag_data_to_path).collect::<Vec<_>>();
    if paths.is_empty() { return false; }
    add_conversion_paths_captured(app, paths, persistence);
    true
}

fn add_conversion_paths_captured(
    app: &AppWindow,
    paths: Vec<PathBuf>,
    persistence: &PrivatePersistence,
) {
    let state = app.global::<AppState>();
    if state.get_conversion_processing() || state.get_conversion_saving() { return; }
    let Ok(authority) = persistence.storage_authority() else { return; };
    let mut images = state.get_conversion_images().iter().collect::<Vec<_>>();
    let mut known_paths = images.iter().map(|item| item.source_path.to_string())
        .filter(|path| !path.is_empty()).collect::<BTreeSet<_>>();
    let available = MAX_CONVERSION_IMAGES.saturating_sub(images.len());
    let mut added = 0usize;
    let mut skipped = paths.len().saturating_sub(available);
    for path in paths.into_iter().take(available) {
        let source_path = path.display().to_string();
        if !path.is_absolute() || !known_paths.insert(source_path.clone()) {
            skipped += 1; continue;
        }
        let Ok((preview, _, _, size)) = load_toolbox_preview_for_authority(&authority, &path, PreviewPurpose::Toolbox) else {
            skipped += 1; continue;
        };
        let name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default().to_string();
        images.push(CompressionImageItem {
            id: Uuid::new_v4().to_string().into(), name: name.into(), source_path: source_path.into(),
            size_text: format_file_size(size).into(), image: preview, status: "pending".into(), result_path: "".into(),
        });
        added += 1;
    }
    let _ = apply_toolbox_completion(persistence, || {
        set_conversion_images(&state, images);
        state.set_conversion_message(compression_add_message(
            state.get_language().as_str() == "en",
            added,
            skipped,
            state.get_conversion_images().row_count(),
        ).into());
    });
}

pub(super) fn add_conversion_paths(app: &AppWindow, paths: Vec<PathBuf>) {
    let state = app.global::<AppState>();
    if state.get_conversion_processing() || state.get_conversion_saving() {
        return;
    }
    let mut images = state.get_conversion_images().iter().collect::<Vec<_>>();
    let mut known_paths = images
        .iter()
        .map(|item| item.source_path.to_string())
        .filter(|path| !path.is_empty())
        .collect::<BTreeSet<_>>();
    let available = MAX_CONVERSION_IMAGES.saturating_sub(images.len());
    let mut added = 0usize;
    let mut skipped = paths.len().saturating_sub(available);

    for path in paths.into_iter().take(available) {
        let canonical = fs::canonicalize(&path).unwrap_or(path);
        let source_path = canonical.display().to_string();
        if !canonical.is_file() || !known_paths.insert(source_path.clone()) {
            skipped += 1;
            continue;
        }
        let Ok(preview) = load_preview_image(&canonical, PreviewPurpose::Toolbox) else {
            skipped += 1;
            continue;
        };
        let name = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        let size = fs::metadata(&canonical)
            .map(|metadata| format_file_size(metadata.len()))
            .unwrap_or_default();
        images.push(CompressionImageItem {
            id: Uuid::new_v4().to_string().into(),
            name: name.into(),
            source_path: source_path.into(),
            size_text: size.into(),
            image: preview,
            status: "pending".into(),
            result_path: "".into(),
        });
        added += 1;
    }

    set_conversion_images(&state, images);
    state.set_conversion_message(
        compression_add_message(
            state.get_language().as_str() == "en",
            added,
            skipped,
            state.get_conversion_images().row_count(),
        )
        .into(),
    );
}

fn paste_conversion_image(app: &AppWindow, persistence: &PrivatePersistence) -> bool {
    let state = app.global::<AppState>();
    if state.get_conversion_processing() || state.get_conversion_saving() {
        return true;
    }
    if state.get_conversion_images().row_count() >= MAX_CONVERSION_IMAGES {
        let _ = apply_toolbox_completion(persistence, || {
            state.set_conversion_message(
                compression_limit_message(state.get_language().as_str() == "en").into(),
            );
        });
        return true;
    }
    let Ok(content) = read_toolbox_clipboard() else { return false; };
    match content {
        ToolboxClipboardContent::Image { width, height, rgba } => {
            let Some(rgba) = image::RgbaImage::from_raw(width, height, rgba) else {
                return false;
            };
            let Ok(bytes) = encode_png_rgba(&rgba, rgba.width(), rgba.height()) else {
                return false;
            };
            let Ok(authority) = persistence.storage_authority() else { return false; };
            let leaf = format!("toolbox-conversion-pasted-{}.png", Uuid::new_v4());
            let Ok(path) = write_toolbox_owned_output(&authority, &leaf, &bytes) else { return false; };
            add_conversion_paths_captured(app, vec![path], persistence);
            true
        }
        ToolboxClipboardContent::Text(text) => {
            add_conversion_from_drag_data_captured(app, TEXT_PLAIN_MIME, &text, persistence)
        }
    }
}

fn set_conversion_images(state: &AppState, images: Vec<CompressionImageItem>) {
    let source_format = conversion_source_format(&images, state.get_language().as_str() == "en");
    let has_results = images
        .iter()
        .any(|item| item.status.as_str() == "completed" && !item.result_path.is_empty());
    state.set_conversion_images(ModelRc::new(VecModel::from(images)));
    state.set_conversion_source_format(source_format.into());
    state.set_conversion_has_results(has_results);
}

fn conversion_source_format(images: &[CompressionImageItem], english: bool) -> String {
    let formats = images
        .iter()
        .filter_map(|item| {
            Path::new(item.source_path.as_str())
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| match value.to_ascii_lowercase().as_str() {
                    "jpg" | "jpeg" => "JPEG".to_string(),
                    "png" => "PNG".to_string(),
                    "webp" => "WebP".to_string(),
                    "bmp" => "BMP".to_string(),
                    value => value.to_ascii_uppercase(),
                })
        })
        .collect::<BTreeSet<_>>();
    match formats.len() {
        0 => "--".to_string(),
        1 => formats.into_iter().next().unwrap_or_default(),
        _ if english => "Mixed formats".to_string(),
        _ => "混合格式".to_string(),
    }
}

fn start_local_conversion(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let Some(persistence) = store.borrow().private_persistence.clone() else { return; };
    let Ok(authority) = persistence.storage_authority() else { return; };
    let state = app.global::<AppState>();
    if state.get_conversion_processing() || state.get_conversion_saving() {
        return;
    }
    let target_format = state.get_conversion_target_format().to_string();
    if conversion_format_extension(&target_format).is_none() {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_conversion_message(
                if state.get_language().as_str() == "en" {
                    "Choose a supported output format"
                } else {
                    "请选择受支持的输出格式"
                }
                .into(),
            );
        });
        return;
    }

    let mut images = state.get_conversion_images().iter().collect::<Vec<_>>();
    if images.is_empty() {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_conversion_message(
                if state.get_language().as_str() == "en" {
                    "Add at least one image first"
                } else {
                    "请先添加需要转换的图片"
                }
                .into(),
            );
        });
        return;
    }
    let inputs = images
        .iter()
        .map(|item| ConversionInput {
            id: item.id.to_string(),
            source_path: item.source_path.to_string(),
        })
        .collect::<Vec<_>>();
    let abandoned_results = images
        .iter()
        .filter_map(|item| {
            (!item.result_path.trim().is_empty())
                .then(|| PathBuf::from(item.result_path.as_str()))
        })
        .collect::<Vec<_>>();
    for item in &mut images {
        item.status = "pending".into();
        item.result_path = "".into();
    }
    if apply_toolbox_completion(&persistence, || {
        set_conversion_images(&state, images);
        state.set_conversion_processing(true);
        state.set_conversion_message(
            if state.get_language().as_str() == "en" {
                "Converting images locally..."
            } else {
                "正在转换中..."
            }
            .into(),
        );
    })
    .is_err()
    {
        return;
    }
    if store
        .borrow()
        .private_persistence
        .as_ref()
        .is_some_and(|current| current.same_binding(&persistence))
    {
        for result_path in abandoned_results {
            let _ = unlink_registered_toolbox_temporary_output(&persistence, &result_path);
        }
    }

    let (sender, receiver) = mpsc::channel::<ConversionOutcome>();
    let worker_persistence = persistence.clone();
    let worker = match spawn_toolbox_worker(persistence.lease().clone(), move |cancel| {
        let Ok(_effect) = worker_persistence.begin_effect() else {
            let _ = sender.send(ConversionOutcome::Interrupted);
            return;
        };
        run_captured_local_conversion_worker(
            authority,
            inputs,
            target_format,
            cancel,
            sender,
        );
    }) {
        Ok(worker) => worker,
        Err(_) => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_conversion_processing(false);
                state.set_conversion_message("本地转换工作线程无法启动".into());
            });
            return;
        }
    };
    poll_local_conversion(
        app.as_weak(),
        store,
        persistence,
        Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
    );
}

fn run_captured_local_conversion_worker(
    authority: Arc<NamespaceStorageAuthority>,
    inputs: Vec<ConversionInput>,
    target_format: String,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    sender: mpsc::Sender<ConversionOutcome>,
) {
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    for input in inputs {
        if cancel.load(Ordering::Acquire) { return; }
        if sender.send(ConversionOutcome::Started { id: input.id.clone() }).is_err() {
            return;
        }
        let result = (|| -> Result<(PathBuf, String)> {
            let source = Path::new(&input.source_path);
            let bytes = authority.read_image_source(source, 100 * 1024 * 1024)?;
            let (bytes, extension) = convert_image_bytes(source, &bytes, &target_format)?;
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "conversion cancelled");
            let source_stem = source.file_stem().and_then(|value| value.to_str()).unwrap_or("image");
            let leaf = format!(
                "{}-{}.{}",
                sanitize_filename(source_stem),
                Uuid::new_v4(),
                extension
            );
            let destination = write_toolbox_owned_output(&authority, &leaf, &bytes)?;
            if cancel.load(Ordering::Acquire) {
                let _ = unlink_toolbox_owned_output_for_authority(&authority, &destination);
                anyhow::bail!("conversion cancelled");
            }
            remember_toolbox_temporary_output(authority.lease(), &destination);
            Ok((destination, format_file_size(bytes.len() as u64)))
        })();
        match result {
            Ok((result_path, size_text)) => {
                succeeded += 1;
                if sender.send(ConversionOutcome::Completed {
                    id: input.id,
                    result_path: result_path.display().to_string(),
                    size_text,
                }).is_err() {
                    return;
                }
            }
            Err(_) => {
                failed += 1;
                if sender.send(ConversionOutcome::Failed { id: input.id }).is_err() {
                    return;
                }
            }
        }
    }
    let _ = sender.send(ConversionOutcome::Finished { succeeded, failed });
}

fn run_local_conversion_worker(
    inputs: Vec<ConversionInput>,
    target_format: String,
    output_dir: PathBuf,
    sender: mpsc::Sender<ConversionOutcome>,
) {
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    if fs::create_dir_all(&output_dir).is_err() {
        for input in inputs {
            failed += 1;
            let _ = sender.send(ConversionOutcome::Failed { id: input.id });
        }
        let _ = sender.send(ConversionOutcome::Finished { succeeded, failed });
        return;
    }

    for input in inputs {
        if sender
            .send(ConversionOutcome::Started {
                id: input.id.clone(),
            })
            .is_err()
        {
            return;
        }
        let result = (|| -> Result<(PathBuf, String)> {
            let (bytes, extension) =
                convert_image_file(Path::new(&input.source_path), &target_format)?;
            let source_stem = Path::new(&input.source_path)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("image");
            let destination = output_dir.join(format!(
                "{}-{}.{}",
                sanitize_filename(source_stem),
                Uuid::new_v4(),
                extension
            ));
            atomic_write_file(&destination, &bytes)?;
            Ok((destination, format_file_size(bytes.len() as u64)))
        })();
        match result {
            Ok((result_path, size_text)) => {
                succeeded += 1;
                if sender
                    .send(ConversionOutcome::Completed {
                        id: input.id,
                        result_path: result_path.display().to_string(),
                        size_text,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(_) => {
                failed += 1;
                if sender
                    .send(ConversionOutcome::Failed { id: input.id })
                    .is_err()
                {
                    return;
                }
            }
        }
    }
    let _ = sender.send(ConversionOutcome::Finished { succeeded, failed });
}

fn update_conversion_item(
    state: &AppState,
    id: &str,
    status: &str,
    result_path: Option<&str>,
    size_text: Option<&str>,
) {
    let mut images = state.get_conversion_images().iter().collect::<Vec<_>>();
    let Some(item) = images.iter_mut().find(|item| item.id.as_str() == id) else {
        return;
    };
    item.status = status.into();
    if let Some(result_path) = result_path {
        item.result_path = result_path.into();
    }
    if let Some(size_text) = size_text {
        item.size_text = size_text.into();
    }
    set_conversion_images(state, images);
}

fn poll_local_conversion(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<ConversionOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_mut() else {
                return;
            };
            match rx.poll_message() {
                Ok(Some(outcome @ ConversionOutcome::Finished { .. }))
                | Ok(Some(outcome @ ConversionOutcome::Interrupted)) => {
                    match rx.finish_message(outcome) {
                        Ok(Some(outcome)) => {
                            slot.take();
                            Some(outcome)
                        }
                        Ok(None) => None,
                        Err(_) => {
                            slot.take();
                            Some(ConversionOutcome::Interrupted)
                        }
                    }
                }
                Ok(Some(outcome)) => Some(outcome),
                Ok(None) => None,
                Err(_) => { slot.take(); Some(ConversionOutcome::Interrupted) }
            }
        };
        let Some(outcome) = outcome else {
            poll_local_conversion(app_weak, store, persistence, receiver);
            return;
        };
        let Some(_effect) = recapture_toolbox_effect_for_binding(&store, &persistence) else {
            receiver.borrow_mut().take();
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        if apply_toolbox_completion(&persistence, || match outcome {
            ConversionOutcome::Started { id } => {
                update_conversion_item(&state, &id, "processing", None, None);
            }
            ConversionOutcome::Completed {
                id,
                result_path,
                size_text,
            } => {
                update_conversion_item(
                    &state,
                    &id,
                    "completed",
                    Some(&result_path),
                    Some(&size_text),
                );
            }
            ConversionOutcome::Failed { id } => {
                update_conversion_item(&state, &id, "failed", Some(""), None);
            }
            ConversionOutcome::Finished { succeeded, failed } => {
                keep_polling = false;
                state.set_conversion_processing(false);
                state.set_conversion_message(
                    if state.get_language().as_str() == "en" {
                        if failed == 0 {
                            format!("Converted {succeeded} image(s). Click Save to export.")
                        } else {
                            format!(
                                "Converted {succeeded} image(s); {failed} failed. Save completed results."
                            )
                        }
                    } else if failed == 0 {
                        format!("已完成 {succeeded} 张图片转换，点击“保存”导出到本地")
                    } else {
                        format!("已转换 {succeeded} 张，失败 {failed} 张；可保存已完成的结果")
                    }
                    .into(),
                );
            }
            ConversionOutcome::Interrupted => {
                keep_polling = false;
                let mut images = state.get_conversion_images().iter().collect::<Vec<_>>();
                for item in &mut images {
                    if matches!(item.status.as_str(), "pending" | "processing") {
                        item.status = "failed".into();
                    }
                }
                set_conversion_images(&state, images);
                state.set_conversion_processing(false);
                state.set_conversion_message(
                    if state.get_language().as_str() == "en" {
                        "The local conversion task was interrupted"
                    } else {
                        "本地转换任务意外中断，请重试"
                    }
                    .into(),
                );
            }
        }).is_err() {
            receiver.borrow_mut().take();
            return;
        }
        if keep_polling {
            poll_local_conversion(app_weak, store, persistence, receiver);
        }
    });
}

fn start_conversion_result_save(
    app: &AppWindow,
    result_path: PathBuf,
    source_name: String,
    context: AppContext,
    persistence: PrivatePersistence,
) {
    let Some(root) = context.data_root_capability.clone() else { return; };
    let extension = result_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let source_stem = Path::new(&source_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let default_name = format!("{}-converted.{}", sanitize_filename(source_stem), extension);
    let filter_name = format!("{} Image", extension.to_ascii_uppercase());
    let app_weak = app.as_weak();
    let _ = spawn_toolbox_dialog(async move {
        let Some(selected_path) = choose_toolbox_export_path(
            "保存转换结果",
            default_name,
            filter_name,
            &extension,
        )
        .await
        else {
            return;
        };
        let destination = normalize_conversion_destination(&selected_path, &extension);
        let Some(parent) = destination.parent().map(Path::to_path_buf) else { return; };
        let Some(name) = destination.file_name().and_then(|value| value.to_str()).map(str::to_owned) else { return; };
        let Some(effect) = recapture_toolbox_effect_for_binding(&context.store, &persistence) else { return; };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if apply_toolbox_completion(&effect._persistence, || {
            state.set_conversion_saving(true);
            state.set_conversion_message(
            if state.get_language().as_str() == "en" {
                "Saving the converted image..."
            } else {
                "正在保存转换结果..."
            }
            .into(),
            );
        }).is_err() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let worker_persistence = persistence.clone();
        let worker_destination = destination.clone();
        let worker = match spawn_toolbox_worker(persistence.lease().clone(), move |cancel| {
            let outcome = (|| -> Result<ConversionSaveOutcome> {
                let _effect = worker_persistence.begin_effect()?;
                anyhow::ensure!(!cancel.load(Ordering::Acquire), "conversion export cancelled");
                let authority = worker_persistence.storage_authority()?;
                let bytes = authority.read_image_source(&result_path, 100 * 1024 * 1024)?;
                anyhow::ensure!(!cancel.load(Ordering::Acquire), "conversion export cancelled");
                let destination_capability = ExternalExportDestination::open(&root, &parent)?;
                destination_capability.write_new_file(&name, &mut Cursor::new(bytes))?;
                anyhow::ensure!(!cancel.load(Ordering::Acquire), "conversion export cancelled");
                let released = unlink_registered_toolbox_temporary_output(&worker_persistence, &result_path)
                    .then_some(result_path);
                Ok(ConversionSaveOutcome::Saved {
                    destination: worker_destination,
                    released_result_path: released,
                })
            })()
            .unwrap_or(ConversionSaveOutcome::Failed);
            if !cancel.load(Ordering::Acquire) {
                let _ = sender.send(outcome);
            }
        }) {
            Ok(worker) => worker,
            Err(_) => {
                let _ = apply_toolbox_completion(&persistence, || {
                    state.set_conversion_saving(false);
                    state.set_conversion_message("转换结果保存线程无法启动".into());
                });
                return;
            }
        };
        poll_conversion_result_save(
            app.as_weak(),
            context.store.clone(),
            persistence,
            Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
        );
    });
}

fn normalize_conversion_destination(path: &Path, extension: &str) -> PathBuf {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    {
        path.to_path_buf()
    } else {
        path.with_extension(extension)
    }
}

fn poll_conversion_result_save(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<ConversionSaveOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_mut() else {
                return;
            };
            match rx.poll_terminal_message() {
                Ok(Some(outcome)) => { slot.take(); Some(outcome) }
                Ok(None) => None,
                Err(_) => { slot.take(); Some(ConversionSaveOutcome::Failed) }
            }
        };
        let Some(outcome) = outcome else {
            poll_conversion_result_save(app_weak, store, persistence, receiver);
            return;
        };
        let Some(_effect) = recapture_toolbox_effect_for_binding(&store, &persistence) else {
            receiver.borrow_mut().take();
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_conversion_saving(false);
            match outcome {
            ConversionSaveOutcome::Saved {
                destination,
                released_result_path,
            } => {
                if let Some(released_result_path) = released_result_path {
                    let mut images = state.get_conversion_images().iter().collect::<Vec<_>>();
                    if clear_released_toolbox_result(&mut images, &released_result_path) {
                        set_conversion_images(&state, images);
                    }
                }
                state.set_conversion_message(
                    if state.get_language().as_str() == "en" {
                        format!("Saved to {}", destination.display())
                    } else {
                        format!("已保存到 {}", destination.display())
                    }
                    .into(),
                );
            }
            ConversionSaveOutcome::Failed => state.set_conversion_message(
                if state.get_language().as_str() == "en" {
                    "The converted image could not be saved"
                } else {
                    "转换结果保存失败，请重试"
                }
                .into(),
            ),
            }
        });
    });
}

pub(super) fn add_crop_from_drag_data(app: &AppWindow, mime_type: &str, data: &str) -> bool {
    let state = app.global::<AppState>();
    if state.get_crop_processing() {
        state.set_crop_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while saving"
            } else {
                "保存过程中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    let paths = data
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(drag_data_to_path)
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return false;
    }
    add_crop_paths(app, paths)
}

fn add_crop_from_drag_data_captured(
    app: &AppWindow,
    mime_type: &str,
    data: &str,
    persistence: &PrivatePersistence,
) -> bool {
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    let paths = data.lines().map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(drag_data_to_path).collect::<Vec<_>>();
    if paths.is_empty() { return false; }
    add_crop_paths_captured(app, paths, persistence)
}

fn add_crop_paths_captured(
    app: &AppWindow,
    paths: Vec<PathBuf>,
    persistence: &PrivatePersistence,
) -> bool {
    let state = app.global::<AppState>();
    if state.get_crop_processing() { return true; }
    let Ok(authority) = persistence.storage_authority() else { return false; };
    for path in paths {
        if !path.is_absolute() { continue; }
        let Ok((preview, width, height, _)) = load_toolbox_preview_for_authority(&authority, &path, PreviewPurpose::Canvas) else {
            continue;
        };
        let name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default().to_string();
        let applied = apply_toolbox_completion(persistence, || {
            state.set_crop_source_path(path.display().to_string().into());
            state.set_crop_source_name(name.into());
            state.set_crop_source_image(preview);
            state.set_crop_source_width(width as i32);
            state.set_crop_source_height(height as i32);
            state.set_crop_transform_steps("".into());
            state.set_crop_ratio("original".into());
            state.set_crop_x(0.0);
            state.set_crop_y(0.0);
            state.set_crop_width(1.0);
            state.set_crop_height(1.0);
            state.set_crop_processing(false);
            state.set_crop_message("".into());
        });
        return applied.is_ok();
    }
    let _ = apply_toolbox_completion(persistence, || {
        state.set_crop_message(if state.get_language().as_str() == "en" {
            "Choose a supported image"
        } else { "请选择受支持的图片" }.into());
    });
    true
}

pub(super) fn add_compression_paths_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    paths: Vec<PathBuf>,
) {
    let Some(effect) = capture_toolbox_effect(store) else { return; };
    add_compression_paths_captured(app, paths, &effect._persistence);
}

pub(super) fn add_compression_drag_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    let Some(effect) = capture_toolbox_effect(store) else { return false; };
    add_compression_from_drag_data_captured(app, mime_type, data, &effect._persistence)
}

pub(super) fn add_conversion_paths_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    paths: Vec<PathBuf>,
) {
    let Some(effect) = capture_toolbox_effect(store) else { return; };
    add_conversion_paths_captured(app, paths, &effect._persistence);
}

pub(super) fn add_conversion_drag_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    let Some(effect) = capture_toolbox_effect(store) else { return false; };
    add_conversion_from_drag_data_captured(app, mime_type, data, &effect._persistence)
}

pub(super) fn add_crop_paths_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    paths: Vec<PathBuf>,
) -> bool {
    let Some(effect) = capture_toolbox_effect(store) else { return false; };
    add_crop_paths_captured(app, paths, &effect._persistence)
}

pub(super) fn add_crop_drag_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    let Some(effect) = capture_toolbox_effect(store) else { return false; };
    add_crop_from_drag_data_captured(app, mime_type, data, &effect._persistence)
}

pub(super) fn add_watermark_paths_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    paths: Vec<PathBuf>,
) -> bool {
    let Some(effect) = capture_toolbox_effect(store) else { return false; };
    if app.global::<AppState>().get_watermark_processing() {
        let _ = apply_toolbox_completion(&effect._persistence, || {
            app.global::<AppState>()
                .set_watermark_message("处理中暂时不能更换图片".into());
        });
        return true;
    }
    let Ok(authority) = effect._persistence.storage_authority() else { return false; };
    for path in paths {
        if set_watermark_source_for_authority(app, &effect._persistence, &authority, &path) { return true; }
    }
    set_watermark_unsupported_message_captured(app, &effect._persistence);
    true
}

pub(super) fn add_watermark_drag_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    let Some(effect) = capture_toolbox_effect(store) else { return false; };
    add_watermark_from_drag_data_captured(app, mime_type, data, store.clone(), effect._persistence)
}

pub(super) fn add_colorization_paths_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    paths: Vec<PathBuf>,
) -> bool {
    let Some(effect) = capture_toolbox_effect(store) else { return false; };
    if app.global::<AppState>().get_colorize_processing() {
        let _ = apply_toolbox_completion(&effect._persistence, || {
            app.global::<AppState>()
                .set_colorize_message("处理中暂时不能更换图片".into());
        });
        return true;
    }
    let Ok(authority) = effect._persistence.storage_authority() else { return false; };
    for path in paths {
        if set_colorization_source_for_authority(app, &effect._persistence, &authority, &path).is_ok() { return true; }
    }
    set_colorization_source_error_captured(
        app,
        &effect._persistence,
        &anyhow!("unsupported captured colorization source"),
    );
    true
}

pub(super) fn add_colorization_drag_for_store(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    let Some(effect) = capture_toolbox_effect(store) else { return false; };
    add_colorization_from_drag_data_captured(app, mime_type, data, store.clone(), effect._persistence)
}

pub(super) fn add_crop_paths(app: &AppWindow, paths: Vec<PathBuf>) -> bool {
    let state = app.global::<AppState>();
    if state.get_crop_processing() {
        state.set_crop_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while saving"
            } else {
                "保存过程中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    let previous_source = PathBuf::from(state.get_crop_source_path().to_string());
    for path in paths {
        let canonical = fs::canonicalize(&path).unwrap_or(path);
        if !canonical.is_file() {
            continue;
        }
        let Ok(preview) = load_preview_image(&canonical, PreviewPurpose::Canvas) else {
            continue;
        };
        let Ok((source_width, source_height)) = inspect_image_dimensions(&canonical) else {
            continue;
        };
        let name = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        state.set_crop_source_path(canonical.display().to_string().into());
        state.set_crop_source_name(name.into());
        state.set_crop_source_image(preview);
        state.set_crop_source_width(source_width as i32);
        state.set_crop_source_height(source_height as i32);
        state.set_crop_transform_steps("".into());
        state.set_crop_ratio("original".into());
        state.set_crop_x(0.0);
        state.set_crop_y(0.0);
        state.set_crop_width(1.0);
        state.set_crop_height(1.0);
        state.set_crop_processing(false);
        state.set_crop_message("".into());
        if !previous_source.as_os_str().is_empty() && previous_source != canonical {
            let _ = remove_managed_toolbox_file(
                &app_data_dir(),
                ManagedToolboxDirectory::CropInputs,
                &previous_source,
            );
        }
        return true;
    }
    state.set_crop_message(
        if state.get_language().as_str() == "en" {
            "Choose a supported image"
        } else {
            "请选择受支持的图片"
        }
        .into(),
    );
    true
}

fn paste_crop_image(app: &AppWindow, persistence: &PrivatePersistence) -> bool {
    let state = app.global::<AppState>();
    if state.get_crop_processing() {
        state.set_crop_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while saving"
            } else {
                "保存过程中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    let Ok(content) = read_toolbox_clipboard() else { return false; };
    match content {
        ToolboxClipboardContent::Image { width, height, rgba } => {
            let Some(rgba) = image::RgbaImage::from_raw(width, height, rgba) else {
                return false;
            };
            let Ok(bytes) = encode_png_rgba(&rgba, rgba.width(), rgba.height()) else {
                return false;
            };
            let Ok(authority) = persistence.storage_authority() else { return false; };
            let leaf = format!("toolbox-crop-pasted-{}.png", Uuid::new_v4());
            let Ok(path) = write_toolbox_owned_output(&authority, &leaf, &bytes) else { return false; };
            add_crop_paths_captured(app, vec![path], persistence);
            true
        }
        ToolboxClipboardContent::Text(text) => {
            add_crop_from_drag_data_captured(app, TEXT_PLAIN_MIME, &text, persistence)
        }
    }
}

fn set_crop_ratio(app: &AppWindow, ratio: &str) {
    let state = app.global::<AppState>();
    if state.get_crop_source_width() <= 0 || state.get_crop_source_height() <= 0 {
        return;
    }
    if ratio == "free" {
        state.set_crop_ratio(ratio.into());
        state.set_crop_message("".into());
        return;
    }
    let source_aspect =
        state.get_crop_source_width() as f32 / state.get_crop_source_height() as f32;
    let target_aspect = match ratio {
        "original" => source_aspect,
        "1:1" => 1.0,
        "4:3" => 4.0 / 3.0,
        "3:4" => 3.0 / 4.0,
        "16:9" => 16.0 / 9.0,
        "9:16" => 9.0 / 16.0,
        _ => return,
    };
    let (width, height) = if target_aspect >= source_aspect {
        (1.0, (source_aspect / target_aspect).clamp(0.0, 1.0))
    } else {
        ((target_aspect / source_aspect).clamp(0.0, 1.0), 1.0)
    };
    state.set_crop_ratio(ratio.into());
    state.set_crop_x((1.0 - width) / 2.0);
    state.set_crop_y((1.0 - height) / 2.0);
    state.set_crop_width(width);
    state.set_crop_height(height);
    state.set_crop_message("".into());
}

fn update_crop_rect(
    app: &AppWindow,
    action: &str,
    dx: f32,
    dy: f32,
    start_x: f32,
    start_y: f32,
    start_width: f32,
    start_height: f32,
) {
    let state = app.global::<AppState>();
    let source_width = state.get_crop_source_width().max(1) as f32;
    let source_height = state.get_crop_source_height().max(1) as f32;
    let minimum_width = (16.0 / source_width).max(0.02);
    let minimum_height = (16.0 / source_height).max(0.02);
    if action == "move" {
        state.set_crop_x((start_x + dx).clamp(0.0, 1.0 - start_width));
        state.set_crop_y((start_y + dy).clamp(0.0, 1.0 - start_height));
        return;
    }
    if !matches!(action, "nw" | "ne" | "sw" | "se") {
        return;
    }

    let west = action == "nw" || action == "sw";
    let north = action == "nw" || action == "ne";
    let fixed_x = if west { start_x + start_width } else { start_x };
    let fixed_y = if north {
        start_y + start_height
    } else {
        start_y
    };

    if state.get_crop_ratio().as_str() == "free" {
        let requested_width = if west {
            start_width - dx
        } else {
            start_width + dx
        };
        let requested_height = if north {
            start_height - dy
        } else {
            start_height + dy
        };
        let max_width = if west { fixed_x } else { 1.0 - fixed_x };
        let max_height = if north { fixed_y } else { 1.0 - fixed_y };
        let width = requested_width.clamp(minimum_width.min(max_width), max_width);
        let height = requested_height.clamp(minimum_height.min(max_height), max_height);
        state.set_crop_x(if west { fixed_x - width } else { fixed_x });
        state.set_crop_y(if north { fixed_y - height } else { fixed_y });
        state.set_crop_width(width);
        state.set_crop_height(height);
        return;
    }

    let source_aspect = source_width / source_height;
    let target_aspect = match state.get_crop_ratio().as_str() {
        "original" => source_aspect,
        "1:1" => 1.0,
        "4:3" => 4.0 / 3.0,
        "3:4" => 3.0 / 4.0,
        "16:9" => 16.0 / 9.0,
        "9:16" => 9.0 / 16.0,
        _ => source_aspect,
    };
    let horizontal_width = if west {
        start_width - dx
    } else {
        start_width + dx
    };
    let vertical_height = if north {
        start_height - dy
    } else {
        start_height + dy
    };
    let vertical_width = vertical_height * target_aspect / source_aspect;
    let requested_width = if dx.abs() >= dy.abs() {
        horizontal_width
    } else {
        vertical_width
    };
    let max_width_from_x = if west { fixed_x } else { 1.0 - fixed_x };
    let max_height = if north { fixed_y } else { 1.0 - fixed_y };
    let max_width_from_y = max_height * target_aspect / source_aspect;
    let max_width = max_width_from_x.min(max_width_from_y);
    let minimum_locked_width = minimum_width.max(minimum_height * target_aspect / source_aspect);
    let width = requested_width.clamp(minimum_locked_width.min(max_width), max_width);
    let height = width * source_aspect / target_aspect;
    state.set_crop_x(if west { fixed_x - width } else { fixed_x });
    state.set_crop_y(if north { fixed_y - height } else { fixed_y });
    state.set_crop_width(width);
    state.set_crop_height(height);
}

fn reset_crop_source(app: &AppWindow) -> Result<()> {
    let state = app.global::<AppState>();
    if state.get_crop_processing() {
        return Ok(());
    }
    state.set_crop_transform_steps("".into());
    refresh_crop_preview(app)
}

fn transform_crop_source(app: &AppWindow, action: &str) -> Result<()> {
    let state = app.global::<AppState>();
    if state.get_crop_processing() || state.get_crop_source_path().trim().is_empty() {
        return Ok(());
    }
    let step = match action {
        "rotate-left" => 'L',
        "rotate-right" => 'R',
        "flip-horizontal" => 'H',
        "flip-vertical" => 'V',
        _ => return Ok(()),
    };
    let mut steps = state.get_crop_transform_steps().to_string();
    steps.push(step);
    state.set_crop_transform_steps(steps.into());
    refresh_crop_preview(app)
}

fn reset_crop_source_captured(app: &AppWindow, persistence: &PrivatePersistence) -> Result<()> {
    let state = app.global::<AppState>();
    if state.get_crop_processing() { return Ok(()); }
    refresh_crop_preview_captured_with_steps(app, persistence, String::new())
}

fn transform_crop_source_captured(
    app: &AppWindow,
    action: &str,
    persistence: &PrivatePersistence,
) -> Result<()> {
    let state = app.global::<AppState>();
    if state.get_crop_processing() || state.get_crop_source_path().trim().is_empty() { return Ok(()); }
    let step = match action {
        "rotate-left" => 'L', "rotate-right" => 'R', "flip-horizontal" => 'H', "flip-vertical" => 'V',
        _ => return Ok(()),
    };
    let mut steps = state.get_crop_transform_steps().to_string();
    steps.push(step);
    refresh_crop_preview_captured_with_steps(app, persistence, steps)
}

fn refresh_crop_preview_captured(app: &AppWindow, persistence: &PrivatePersistence) -> Result<()> {
    let state = app.global::<AppState>();
    let steps = state.get_crop_transform_steps().to_string();
    refresh_crop_preview_captured_with_steps(app, persistence, steps)
}

fn refresh_crop_preview_captured_with_steps(
    app: &AppWindow,
    persistence: &PrivatePersistence,
    steps: String,
) -> Result<()> {
    let state = app.global::<AppState>();
    let path = PathBuf::from(state.get_crop_source_path().to_string());
    let authority = persistence.storage_authority()?;
    let bytes = authority.read_image_source(&path, 100 * 1024 * 1024)?;
    let (mut transformed, _) = decode_image_bytes(&path, &bytes)?;
    for step in steps.chars() {
        transformed = match step {
            'L' => transformed.rotate270(), 'R' => transformed.rotate90(),
            'H' => transformed.fliph(), 'V' => transformed.flipv(), _ => transformed,
        };
    }
    let rgba = transformed.to_rgba8();
    let (width, height) = rgba.dimensions();
    let preview = slint_image_from_rgba(&rgba, width, height);
    apply_toolbox_completion(persistence, || {
        state.set_crop_transform_steps(steps.into());
        state.set_crop_source_image(preview);
        state.set_crop_source_width(width as i32);
        state.set_crop_source_height(height as i32);
        state.set_crop_ratio("original".into());
        state.set_crop_x(0.0);
        state.set_crop_y(0.0);
        state.set_crop_width(1.0);
        state.set_crop_height(1.0);
        state.set_crop_message("".into());
    })
}

fn refresh_crop_preview(app: &AppWindow) -> Result<()> {
    let state = app.global::<AppState>();
    let path = PathBuf::from(state.get_crop_source_path().to_string());
    let steps = state.get_crop_transform_steps().to_string();
    let transformed = transformed_crop_image(&path, &steps)?;
    let rgba = transformed.to_rgba8();
    let (width, height) = rgba.dimensions();
    state.set_crop_source_image(slint_image_from_rgba(&rgba, width, height));
    state.set_crop_source_width(width as i32);
    state.set_crop_source_height(height as i32);
    state.set_crop_ratio("original".into());
    state.set_crop_x(0.0);
    state.set_crop_y(0.0);
    state.set_crop_width(1.0);
    state.set_crop_height(1.0);
    state.set_crop_message("".into());
    Ok(())
}

fn transformed_crop_image(path: &Path, steps: &str) -> Result<image::DynamicImage> {
    let (mut image, _) = decode_image_file(path)?;
    for step in steps.chars() {
        image = match step {
            'L' => image.rotate270(),
            'R' => image.rotate90(),
            'H' => image.fliph(),
            'V' => image.flipv(),
            _ => image,
        };
    }
    Ok(image)
}

fn set_crop_error(app: &AppWindow, _error: &anyhow::Error) {
    let state = app.global::<AppState>();
    state.set_crop_processing(false);
    state.set_crop_message(
        if state.get_language().as_str() == "en" {
            "The image could not be processed"
        } else {
            "图片处理失败，请更换图片后重试"
        }
        .into(),
    );
}

enum CropSaveOutcome {
    Prepared {
        item: AssetData,
        notification: NotificationData,
    },
    Failure,
}

fn start_crop_save(app: &AppWindow, context: AppContext) {
    let Some(persistence) = context.store.borrow().private_persistence.clone() else { return; };
    let Ok(authority) = persistence.storage_authority() else { return; };
    let state = app.global::<AppState>();
    if state.get_crop_processing() {
        return;
    }
    let source_path = state.get_crop_source_path().to_string();
    if source_path.trim().is_empty() {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_crop_message(
                if state.get_language().as_str() == "en" {
                    "Upload an image first"
                } else {
                    "请先上传图片"
                }
                .into(),
            );
        });
        return;
    }
    let steps = state.get_crop_transform_steps().to_string();
    let crop_rect = (
        state.get_crop_x(),
        state.get_crop_y(),
        state.get_crop_width(),
        state.get_crop_height(),
    );
    if apply_toolbox_completion(&persistence, || {
        state.set_crop_processing(true);
        state.set_crop_message(
            if state.get_language().as_str() == "en" {
                "Saving the cropped image..."
            } else {
                "正在保存裁剪结果..."
            }
            .into(),
        );
    })
    .is_err()
    {
        return;
    }

    let (sender, receiver) = mpsc::channel::<CropSaveOutcome>();
    let worker_persistence = persistence.clone();
    let worker = match spawn_toolbox_worker(persistence.lease().clone(), move |cancel| {
        let Ok(_effect) = worker_persistence.begin_effect() else {
            let _ = sender.send(CropSaveOutcome::Failure);
            return;
        };
        if cancel.load(Ordering::Acquire) { return; }
        let outcome = authority
            .read_image_source(Path::new(&source_path), 100 * 1024 * 1024)
            .and_then(|bytes| {
                process_crop_result_bytes(Path::new(&source_path), &bytes, &steps, crop_rect)
            })
            .and_then(|bytes| prepare_crop_asset(&worker_persistence, &source_path, &bytes))
            .map(|(_result_path, item, notification)| CropSaveOutcome::Prepared {
                item,
                notification,
            })
            .unwrap_or(CropSaveOutcome::Failure);
        if !cancel.load(Ordering::Acquire) {
            let _ = sender.send(outcome);
        }
    }) {
        Ok(worker) => worker,
        Err(_) => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_crop_processing(false);
                state.set_crop_message("裁剪工作线程无法启动".into());
            });
            return;
        }
    };
    poll_crop_save(
        app.as_weak(),
        context,
        persistence,
        Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
    );
}

pub(super) fn process_crop_result(
    source_path: &Path,
    steps: &str,
    crop_rect: (f32, f32, f32, f32),
) -> Result<Vec<u8>> {
    let transformed = transformed_crop_image(source_path, steps)?;
    let rgba = transformed.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let left =
        ((crop_rect.0.clamp(0.0, 1.0) * width as f32).floor() as u32).min(width.saturating_sub(1));
    let top = ((crop_rect.1.clamp(0.0, 1.0) * height as f32).floor() as u32)
        .min(height.saturating_sub(1));
    let right = (((crop_rect.0 + crop_rect.2).clamp(0.0, 1.0) * width as f32).ceil() as u32)
        .clamp(left + 1, width);
    let bottom = (((crop_rect.1 + crop_rect.3).clamp(0.0, 1.0) * height as f32).ceil() as u32)
        .clamp(top + 1, height);
    let cropped =
        image::imageops::crop_imm(&rgba, left, top, right - left, bottom - top).to_image();
    encode_png_rgba(&cropped, cropped.width(), cropped.height())
}

fn process_crop_result_bytes(
    source_name: &Path,
    bytes: &[u8],
    steps: &str,
    crop_rect: (f32, f32, f32, f32),
) -> Result<Vec<u8>> {
    let (mut transformed, _) = decode_image_bytes(source_name, bytes)?;
    for step in steps.chars() {
        transformed = match step {
            'L' => transformed.rotate270(),
            'R' => transformed.rotate90(),
            'H' => transformed.fliph(),
            'V' => transformed.flipv(),
            _ => transformed,
        };
    }
    let rgba = transformed.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let left =
        ((crop_rect.0.clamp(0.0, 1.0) * width as f32).floor() as u32).min(width.saturating_sub(1));
    let top = ((crop_rect.1.clamp(0.0, 1.0) * height as f32).floor() as u32)
        .min(height.saturating_sub(1));
    let right = (((crop_rect.0 + crop_rect.2).clamp(0.0, 1.0) * width as f32).ceil() as u32)
        .clamp(left + 1, width);
    let bottom = (((crop_rect.1 + crop_rect.3).clamp(0.0, 1.0) * height as f32).ceil() as u32)
        .clamp(top + 1, height);
    let cropped =
        image::imageops::crop_imm(&rgba, left, top, right - left, bottom - top).to_image();
    encode_png_rgba(&cropped, cropped.width(), cropped.height())
}

fn poll_crop_save(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    persistence: PrivatePersistence,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<CropSaveOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_mut() else {
                return;
            };
            match rx.poll_terminal_message() {
                Ok(Some(outcome)) => { slot.take(); Some(outcome) }
                Ok(None) => None,
                Err(_) => { slot.take(); Some(CropSaveOutcome::Failure) }
            }
        };
        let Some(outcome) = outcome else {
            poll_crop_save(app_weak, context, persistence, receiver);
            return;
        };
        let Some(_effect) = recapture_toolbox_effect_for_binding(&context.store, &persistence) else {
            receiver.borrow_mut().take();
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        match outcome {
            CropSaveOutcome::Prepared { item, notification } => {
                match enqueue_toolbox_asset_projection(
                    &app,
                    &context,
                    &persistence,
                    item,
                    notification,
                ) {
                    Ok(receiver) => {
                        poll_toolbox_asset_ack(
                            app.as_weak(),
                            context.clone(),
                            persistence.clone(),
                            receiver,
                            PendingToolboxAssetCommit {
                                completion: ToolboxAssetCompletion::Crop,
                            },
                        );
                    }
                    Err(_) => {
                        let _ = apply_toolbox_completion(&persistence, || {
                            state.set_crop_processing(false);
                            state.set_crop_message(
                                if state.get_language().as_str() == "en" {
                                    "The cropped image could not be saved"
                                } else {
                                    "裁剪结果保存失败，请重试"
                                }
                                .into(),
                            );
                        });
                    }
                }
            }
            CropSaveOutcome::Failure => {
                let _ = apply_toolbox_completion(&persistence, || {
                    state.set_crop_processing(false);
                    state.set_crop_message(
                        if state.get_language().as_str() == "en" {
                            "The image could not be processed"
                        } else {
                            "图片处理失败，请更换图片后重试"
                        }
                        .into(),
                    )
                });
            }
        }
    });
}

fn prepare_crop_asset(
    persistence: &PrivatePersistence,
    source_path: &str,
    bytes: &[u8],
) -> Result<(String, AssetData, NotificationData)> {
    let (width, height) = generated_image_dimensions(bytes)?;
    let source_title = Path::new(source_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("图片");
    let title = format!("{} 裁剪", short_text(source_title, 18));
    let authority = persistence.storage_authority()?;
    let result = persist_toolbox_result_bytes(&authority, &title, bytes)?;
    let leaf = result
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("crop output identity missing"))?;
    let key = ManagedFileKey::new(ManagedUserArea::Output, leaf)?;
    let file = authority.open_existing_regular(&key)?;
    let registration = NamespacedManagedFileRegistration::new(
        &authority,
        file,
        "image",
        "user",
    )
    .map_err(anyhow::Error::from)?;
    authority
        .delivery_index()?
        .register_file_for_namespace(&authority, &registration)
        .map_err(anyhow::Error::from)?;
    let result_path = result.display().to_string();
    let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: String::new(),
        title: title.clone(),
        category: "other".to_string(),
        kind: "game".to_string(),
        time: now.clone(),
        prompt: "图片裁剪".to_string(),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality_from_actual_dimensions(width, height),
        model: "图片裁剪".to_string(),
        origin: "image_crop".to_string(),
        width,
        height,
        source_path: result_path.clone(),
        reference_paths: vec![source_path.to_string()],
        cutout_done: false,
        remove_black_done: false,
        upscale_done: false,
        is_new: false,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let notification = NotificationData {
            id: Uuid::new_v4().to_string(),
            title: format!("图片裁剪完成：{title}"),
            model: "图片裁剪".to_string(),
            time: now,
            reason: String::new(),
            success: true,
            read: false,
        };
    Ok((result_path, item, notification))
}

fn format_file_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", (bytes.max(1) as f64 / 1024.0).ceil())
    }
}

fn compression_limit_message(english: bool) -> &'static str {
    if english {
        "A batch can contain up to 50 images"
    } else {
        "每批最多添加 50 张图片"
    }
}

fn compression_add_message(english: bool, added: usize, skipped: usize, total: usize) -> String {
    if added == 0 && total >= MAX_COMPRESSION_IMAGES {
        return compression_limit_message(english).to_string();
    }
    if english {
        if skipped == 0 {
            format!("Added {added} image(s), {total}/50 in this batch")
        } else {
            format!("Added {added} image(s), skipped {skipped}, {total}/50 in this batch")
        }
    } else if skipped == 0 {
        format!("已添加 {added} 张，本批次共 {total}/50 张")
    } else {
        format!("已添加 {added} 张，跳过 {skipped} 个文件，本批次共 {total}/50 张")
    }
}

fn start_watermark_removal(app: &AppWindow, context: AppContext) {
    let original = context.store.borrow().private_persistence.clone();
    let (scope, authority, _activity) = match context.capture_billing_action(KnownCapability::Bill) {
        Ok(captured) => captured,
        Err(error) => {
            let message = error.user_message();
            drop(error);
            if let Some(persistence) = original {
                if let Some(_effect) =
                    recapture_toolbox_effect_for_binding(&context.store, &persistence)
                {
                    let _ = apply_toolbox_completion(&persistence, || {
                        app.global::<AppState>().set_watermark_message(message.into());
                    });
                }
            }
            return;
        }
    };
    start_watermark_removal_with_billing_scope(app, context, authority, &scope);
}

pub(super) fn start_watermark_removal_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
) {
    let Some(persistence) = context.store.borrow().private_persistence.clone() else { return; };
    if persistence.lease() != authority.lease() { return; }
    let billing_scope = match capture_billing_scope_for_submission(
        context.backend.as_deref(),
        &authority,
        billing_scope,
    ) {
        Ok(scope) => scope,
        Err(error) => {
            let _ = apply_toolbox_completion(&persistence, || {
                app.global::<AppState>()
                    .set_watermark_message(error.user_message().into());
            });
            return;
        }
    };
    let session_scope = billing_scope.request.session.clone();

    let state = app.global::<AppState>();
    if state.get_session_state().as_str() != "online" {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_auth_open(true);
            state.set_watermark_message(
                if state.get_language().as_str() == "en" {
                    "Sign in and connect to the service before removing a watermark"
                } else {
                    "请先登录并连接服务后再去水印"
                }
                .into(),
            );
        });
        return;
    }
    if context.backend.is_none() || state.get_watermark_processing() {
        return;
    }

    let source = PathBuf::from(state.get_watermark_source_path().to_string());
    let persisted_source = match authority
        .read_image_source(&source, 100 * 1024 * 1024)
        .and_then(|bytes| decode_image_bytes(&source, &bytes).map(|(image, _)| image))
        .and_then(|image| persist_reference_image_for_namespace(&authority, &image))
    {
        Ok(path) => path,
        Err(_) => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "The selected image could not be prepared"
                    } else {
                        "无法处理所选图片，请更换图片后重试"
                    }
                    .into(),
                );
            });
            return;
        }
    };
    if apply_toolbox_completion(&persistence, || {
        state.set_watermark_source_path(persisted_source.display().to_string().into());
    })
    .is_err()
    {
        return;
    }
    let client_request_id = Uuid::new_v4().simple().to_string();
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints_for_namespace(&authority, std::slice::from_ref(&persisted_source)) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                let _ = apply_toolbox_completion(&persistence, || {
                    state.set_watermark_message(format!("原图校验失败：{error}").into());
                });
                return;
            }
        };
    let record = PendingGenerationRecord {
        video_request: None,
        source_asset_id: String::new(),
        schema_version: 2,
            cancel_requested: false,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id,
        owner_user_id: session_scope.owner_user_id.clone(),
        billing_account_group_id: billing_scope.request.account_group_id.clone(),
        auth_epoch: session_scope.auth_epoch,
        local_task_id: Uuid::new_v4().to_string(),
        server_task_id: String::new(),
        raw_prompt: "去除图片水印".to_string(),
        generation_prompt: String::new(),
        task_type: "image_watermark_removal".to_string(),
        category: "other".to_string(),
        mode: "game".to_string(),
        ratio: String::new(),
        quality: "1K".to_string(),
        model_code: "openai_image".to_string(),
        conversation_id: String::new(),
        count: 1,
        target_width: 0,
        target_height: 0,
        create_conversation: false,
        reference_paths: vec![persisted_source.display().to_string()],
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: vec![persisted_source.display().to_string()],
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: String::new(),
        canvas_ui_extraction: false,
    };
    if upsert_pending_generation_for_namespace(&authority, &billing_scope, record.clone()).is_err()
    {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_watermark_message(
                if state.get_language().as_str() == "en" {
                    "The task could not be saved locally"
                } else {
                    "任务准备失败，请重试"
                }
                .into(),
            );
        });
        return;
    }
    launch_watermark_removal_with_billing_scope(
        app,
        context,
        authority,
        Some(billing_scope),
        record,
        false,
    );
}

// TEMP(team-accounts): remove in Task 10 after namespace admission is wired.
pub(super) fn resume_pending_watermark_removal(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
) {
    let Ok(plan) = toolbox_replay_plan(&record, "image_watermark_removal") else {
        return;
    };
    let Some(backend) = context.backend.as_ref() else {
        return;
    };
    let Ok(lease) = context.namespace_for(&plan.session) else {
        return;
    };
    let Ok(authority) = context.storage_authority_for(&lease).map(Arc::new) else {
        return;
    };
    if backend.api.begin_user_work(&plan.session).is_err()
        || authority.user_public_id() != plan.session.owner_user_id
        || authority.lease().auth_epoch != plan.session.auth_epoch
    {
        return;
    }
    launch_watermark_removal_with_billing_scope(
        app,
        context,
        authority,
        None,
        record,
        true,
    );
}

fn launch_watermark_removal_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: Option<BillingScope>,
    record: PendingGenerationRecord,
    recovering: bool,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !generation_scope_matches_context(&context, &session_scope) {
        return;
    }
    let Some(persistence) = context.store.borrow().private_persistence.clone() else { return; };
    if persistence.lease() != authority.lease() { return; }
    let state = app.global::<AppState>();
    if apply_toolbox_completion(&persistence, || {
        state.set_watermark_processing(true);
        state.set_watermark_progress(if recovering { 5 } else { 1 });
        state.set_watermark_estimated_credits("20".into());
        state.set_watermark_result_path("".into());
        state.set_watermark_result_name("".into());
        state.set_watermark_result_image(Image::default());
        state.set_watermark_message(
            if state.get_language().as_str() == "en" {
                if recovering {
                    "Recovering the watermark-removal task..."
                } else {
                    "Uploading the image..."
                }
            } else if recovering {
                "正在恢复未完成的去水印任务..."
            } else {
                "正在上传图片..."
            }
            .into(),
        );
    })
    .is_err()
    {
        return;
    }

    let (sender, receiver) = mpsc::channel::<ToolboxRemoteOutcome>();
    let worker_scope = session_scope.clone();
    let worker = match spawn_toolbox_worker(authority.lease().clone(), move |cancel| {
        if cancel.load(Ordering::Acquire) { return; }
        run_watermark_worker(
            backend,
            authority,
            billing_scope,
            worker_scope,
            record,
            cancel,
            sender,
        )
    }) {
        Ok(worker) => worker,
        Err(_) => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_watermark_processing(false);
                state.set_watermark_message("去水印工作线程无法启动".into());
            });
            return;
        }
    };
    poll_watermark_outcomes(
        app.as_weak(),
        context,
        session_scope,
        persistence,
        Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
    );
}

fn run_watermark_worker(
    backend: Arc<BackendRuntime>,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: Option<BillingScope>,
    session_scope: SessionScope,
    mut record: PendingGenerationRecord,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    sender: mpsc::Sender<ToolboxRemoteOutcome>,
) {
    if cancel.load(Ordering::Acquire) {
        return;
    }
    let Ok(_activity) = backend.api.begin_user_work(&session_scope) else { return; };
    if billing_scope.as_ref().is_some_and(|scope| {
        capture_billing_scope_for_submission(Some(&backend), &authority, scope).is_err()
            || record.billing_account_group_id != scope.request.account_group_id
    })
        || record.owner_user_id != session_scope.owner_user_id
        || record.auth_epoch != session_scope.auth_epoch
        || !backend_generation_scope_active(&backend, &session_scope)
    {
        return;
    }
    if cancel.load(Ordering::Acquire) {
        return;
    }
    let api = GenerationApi::new(backend.api.clone())
        .with_saved_group(&record.billing_account_group_id);
    if record.terminal {
        let prepared = authority
            .delivery_index()
            .map_err(|error| anyhow!(error.to_string()))
            .and_then(|index| {
                prepare_namespace_delivery(
                    &api,
                    authority.clone(),
                    index,
                    &record.identity(),
                    0,
                )
                .map_err(|error| anyhow!(error.to_string()))
            });
        let outcome = match prepared {
            Ok(prepared) => ToolboxRemoteOutcome::Prepared(Box::new(prepared)),
            Err(error) => ToolboxRemoteOutcome::Failure {
                reason: error.to_string(),
            },
        };
        if !cancel.load(Ordering::Acquire) {
            let _ = sender.send(outcome);
        }
        return;
    }

    let mut uploaded = record.uploaded_file_ids.clone();
    if uploaded.is_empty() && record.server_task_id.is_empty() {
        if record.reference_paths.len() != 1
            || record.reference_sha256.len() != 1
            || record.reference_size_bytes.len() != 1
        {
            let _ = sender.send(ToolboxRemoteOutcome::Failure {
                reason: "原图内容已变化，恢复任务已暂停，请重新发起".to_string(),
            });
            return;
        }
        let Some(path) = record.reference_paths.first() else {
            let _ = sender.send(ToolboxRemoteOutcome::Failure {
                reason: "找不到待处理的原图，请重新上传".to_string(),
            });
            return;
        };
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match api.upload_reference_for_namespace_checked(
            Path::new(path),
            &authority,
            &session_scope,
            false,
            &record.reference_sha256[0],
            record.reference_size_bytes[0],
        ) {
            Ok(file_id) => {
                if cancel.load(Ordering::Acquire) {
                    let _ = api.delete_reference_scoped(&file_id, &session_scope);
                    return;
                }
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                if !matches!(
                    apply_generation_patch_for_namespace(
                        &authority,
                        &record.identity(),
                        GenerationRecoveryPatch::UploadedFileIds(snapshot)
                    ),
                    Ok(true)
                ) {
                    if let Some(file_id) = uploaded.last() {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    return;
                }
            }
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    let _ = remove_pending_generation_for_namespace(&authority, &record.identity());
                }
                let _ = sender.send(ToolboxRemoteOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    }

    let mut detail = if record.server_task_id.is_empty() {
        let request = CreateWatermarkRemoval {
            client_request_id: record.client_request_id.clone(),
            reference_file_id: uploaded[0].clone(),
        };
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let created: std::result::Result<GenerationTaskDetail, ApiError> =
            if let Some(billing_scope) = billing_scope.as_ref() {
                api.create_watermark_removal_billing(&request, billing_scope)
            } else {
                SavedReplayRequest::generation(
                    authority.clone(),
                    &session_scope,
                    &record.client_request_id,
                )
                .map_err(transition_error)
                .and_then(|replay| {
                    backend
                        .api
                        .replay_saved::<GenerationTaskDetail>(&replay)
                        .map(|response| response.data)
                })
            };
        match created {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if error.is_billing_rejection() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &record.identity());
                    let _ = sender.send(ToolboxRemoteOutcome::CreditInsufficient {
                        message: error,
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &record.identity());
                }
                let _ = sender.send(ToolboxRemoteOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    } else {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ToolboxRemoteOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    };

    if validate_toolbox_task_detail(&record, &detail).is_err() {
        let _ = sender.send(ToolboxRemoteOutcome::Failure {
            reason: "服务端返回了不匹配的任务身份或原付款账户，恢复记录已保留".to_string(),
        });
        return;
    }

    record.server_task_id = detail.id.clone();
    let server_task_id = detail.id.clone();
    let server_id_snapshot = server_task_id.clone();
    let uploaded_snapshot = uploaded.clone();
    if !matches!(
        apply_generation_patch_for_namespace(
            &authority,
            &record.identity(),
            GenerationRecoveryPatch::Accepted {
                server_task_id: server_id_snapshot,
                uploaded_file_ids: uploaded_snapshot,
                clear_reference_inputs: false
            }
        ),
        Ok(true)
    ) {
        return;
    }
    let _ = sender.send(ToolboxRemoteOutcome::Accepted {
        task_id: server_task_id.clone(),
    });

    loop {
        if cancel.load(Ordering::Acquire)
            || !backend_generation_scope_active(&backend, &session_scope)
        {
            return;
        }
        let _ = sender.send(ToolboxRemoteOutcome::Progress {
            percent: detail.progress_percent,
        });
        if let Some(item) = detail.items.iter().find(|item| item.status == "succeeded") {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let prepared = authority
                .delivery_index()
                .map_err(|error| anyhow!(error.to_string()))
                .and_then(|index| {
                    prepare_namespace_delivery(
                        &api,
                        authority.clone(),
                        index,
                        &record.identity(),
                        item.index,
                    )
                    .map_err(|error| anyhow!(error.to_string()))
                });
            let outcome = match prepared {
                Ok(prepared) => ToolboxRemoteOutcome::Prepared(Box::new(prepared)),
                Err(error) => ToolboxRemoteOutcome::Failure {
                    reason: error.to_string(),
                },
            };
            if !cancel.load(Ordering::Acquire) {
                let _ = sender.send(outcome);
            }
            return;
        }
        if detail.terminal() {
            let reason = detail
                .failure
                .as_ref()
                .map(|failure| failure.message.clone())
                .or_else(|| {
                    detail.items.iter().find_map(|item| {
                        item.failure.as_ref().map(|failure| failure.message.clone())
                    })
                })
                .unwrap_or_else(|| "服务端未能完成去水印".to_string());
            if !matches!(
                apply_generation_patch_for_namespace(
                    &authority,
                    &record.identity(),
                    GenerationRecoveryPatch::Terminal {
                        expected_success_count: 0
                    }
                ),
                Ok(true)
            ) {
                return;
            }
            let _ = sender.send(ToolboxRemoteOutcome::Failure { reason });
            return;
        }
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let next_detail = match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ToolboxRemoteOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        };
        if validate_toolbox_task_detail(&record, &next_detail).is_err() {
            let _ = sender.send(ToolboxRemoteOutcome::Failure {
                reason: "服务端轮询返回了不匹配的任务身份或原付款账户，恢复记录已保留"
                    .to_string(),
            });
            return;
        }
        detail = next_detail;
    }
}

fn poll_watermark_outcomes(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    persistence: PrivatePersistence,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<ToolboxRemoteOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_mut() else {
                return;
            };
            match rx.poll_message() {
                Ok(Some(outcome)) if outcome.terminal() => match rx.finish_message(outcome) {
                    Ok(Some(outcome)) => {
                        slot.take();
                        Some(outcome)
                    }
                    Ok(None) => None,
                    Err(_) => {
                        slot.take();
                        Some(ToolboxRemoteOutcome::Failure {
                            reason: "去水印任务已中断，请重试".to_string(),
                        })
                    }
                },
                Ok(Some(outcome)) => Some(outcome),
                Ok(None) => None,
                Err(_) => { slot.take(); Some(ToolboxRemoteOutcome::Failure {
                    reason: "去水印任务已中断，请重试".to_string(),
                }) }
            }
        };
        let Some(outcome) = outcome else {
            poll_watermark_outcomes(app_weak, context, session_scope, persistence, receiver);
            return;
        };
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if !toolbox_binding_is_current(&context.store, &persistence) {
            receiver.borrow_mut().take();
            return;
        }
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        match outcome {
            ToolboxRemoteOutcome::Accepted { task_id } => {
                if apply_toolbox_completion(&persistence, || {
                    state.set_watermark_progress(state.get_watermark_progress().max(8));
                    state.set_watermark_message(
                        if state.get_language().as_str() == "en" {
                            format!("Task {task_id} is queued")
                        } else {
                            "任务已提交，正在排队处理...".to_string()
                        }
                        .into(),
                    );
                })
                .is_err()
                {
                    receiver.borrow_mut().take();
                    return;
                }
            }
            ToolboxRemoteOutcome::Progress { percent } => {
                if apply_toolbox_completion(&persistence, || {
                    state.set_watermark_progress(percent.clamp(1, 99));
                    state.set_watermark_message(
                        if state.get_language().as_str() == "en" {
                            "Removing the watermark..."
                        } else {
                            "正在智能修复水印区域..."
                        }
                        .into(),
                    );
                })
                .is_err()
                {
                    receiver.borrow_mut().take();
                    return;
                }
            }
            ToolboxRemoteOutcome::Prepared(prepared) => {
                keep_polling = false;
                match enqueue_toolbox_remote_delivery(
                    &app,
                    context.clone(),
                    persistence.clone(),
                    *prepared,
                    ToolboxRemoteKind::Watermark,
                ) {
                    Ok(()) => {}
                    Err(error) => {
                        let _ = apply_toolbox_completion(&persistence, || {
                            state.set_watermark_processing(false);
                            state.set_watermark_message(
                                format!("处理结果保存失败：{}", zh_error(&error.to_string())).into(),
                            );
                        });
                    }
                }
            }
            ToolboxRemoteOutcome::CreditInsufficient { message } => {
                keep_polling = false;
                let applied = apply_toolbox_completion(&persistence, || {
                    state.set_watermark_processing(false);
                    state.set_watermark_progress(0);
                    if let Some(text) = show_credit_rejection(&state, &message) {
                        state.set_watermark_message(text.into());
                    }
                });
                if applied.is_ok() && context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            ToolboxRemoteOutcome::Failure { reason } => {
                keep_polling = false;
                let applied = apply_toolbox_completion(&persistence, || {
                    state.set_watermark_processing(false);
                    state.set_watermark_progress(0);
                    state.set_watermark_message(reason.into());
                });
                if applied.is_ok() && context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
        }
        if keep_polling {
            poll_watermark_outcomes(app_weak, context, session_scope, persistence, receiver);
        }
    });
}

fn start_image_colorization(app: &AppWindow, context: AppContext) {
    let original = context.store.borrow().private_persistence.clone();
    let (scope, authority, _activity) = match context.capture_billing_action(KnownCapability::Bill) {
        Ok(captured) => captured,
        Err(error) => {
            let message = error.user_message();
            drop(error);
            if let Some(persistence) = original {
                if let Some(_effect) =
                    recapture_toolbox_effect_for_binding(&context.store, &persistence)
                {
                    let _ = apply_toolbox_completion(&persistence, || {
                        app.global::<AppState>().set_colorize_message(message.into());
                    });
                }
            }
            return;
        }
    };
    start_image_colorization_with_billing_scope(app, context, authority, &scope);
}

pub(super) fn start_image_colorization_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
) {
    let Some(persistence) = context.store.borrow().private_persistence.clone() else { return; };
    if persistence.lease() != authority.lease() { return; }
    let billing_scope = match capture_billing_scope_for_submission(
        context.backend.as_deref(),
        &authority,
        billing_scope,
    ) {
        Ok(scope) => scope,
        Err(error) => {
            let _ = apply_toolbox_completion(&persistence, || {
                app.global::<AppState>()
                    .set_colorize_message(error.user_message().into());
            });
            return;
        }
    };
    let session_scope = billing_scope.request.session.clone();

    let state = app.global::<AppState>();
    if state.get_session_state().as_str() != "online" {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_auth_open(true);
            state.set_colorize_message(
                if state.get_language().as_str() == "en" {
                    "Sign in and connect to the service before colorizing a photo"
                } else {
                    "请先登录并连接服务后再进行老照片上色"
                }
                .into(),
            );
        });
        return;
    }
    if context.backend.is_none() || state.get_colorize_processing() {
        return;
    }

    let source = PathBuf::from(state.get_colorize_source_path().to_string());
    if let Err(error) = set_colorization_source_for_authority(app, &persistence, &authority, &source) {
        set_colorization_source_error_captured(app, &persistence, &error);
        return;
    }
    let persisted_source = match authority
        .read_image_source(&source, 100 * 1024 * 1024)
        .and_then(|bytes| decode_image_bytes(&source, &bytes).map(|(image, _)| image))
        .map(|image| flatten_colorization_image(&image))
        .and_then(|image| persist_reference_image_for_namespace(&authority, &image))
    {
        Ok(path) => path,
        Err(_) => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_colorize_message(
                    if state.get_language().as_str() == "en" {
                        "The selected image could not be prepared"
                    } else {
                        "无法处理所选图片，请更换图片后重试"
                    }
                    .into(),
                );
            });
            return;
        }
    };
    if apply_toolbox_completion(&persistence, || {
        state.set_colorize_source_path(persisted_source.display().to_string().into());
    })
    .is_err()
    {
        return;
    }
    let client_request_id = Uuid::new_v4().simple().to_string();
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints_for_namespace(&authority, std::slice::from_ref(&persisted_source)) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                let _ = apply_toolbox_completion(&persistence, || {
                    state.set_colorize_message(format!("原图校验失败：{error}").into());
                });
                return;
            }
        };
    let record = PendingGenerationRecord {
        video_request: None,
        source_asset_id: String::new(),
        schema_version: 2,
            cancel_requested: false,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id,
        owner_user_id: session_scope.owner_user_id.clone(),
        billing_account_group_id: billing_scope.request.account_group_id.clone(),
        auth_epoch: session_scope.auth_epoch,
        local_task_id: Uuid::new_v4().to_string(),
        server_task_id: String::new(),
        raw_prompt: "老照片上色".to_string(),
        generation_prompt: String::new(),
        task_type: "image_colorization".to_string(),
        category: "other".to_string(),
        mode: "game".to_string(),
        ratio: String::new(),
        quality: "standard".to_string(),
        model_code: "aliyun_image_colorization".to_string(),
        conversation_id: String::new(),
        count: 1,
        target_width: 0,
        target_height: 0,
        create_conversation: false,
        reference_paths: vec![persisted_source.display().to_string()],
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: vec![persisted_source.display().to_string()],
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: String::new(),
        canvas_ui_extraction: false,
    };
    if upsert_pending_generation_for_namespace(&authority, &billing_scope, record.clone()).is_err()
    {
        let _ = apply_toolbox_completion(&persistence, || {
            state.set_colorize_message(
                if state.get_language().as_str() == "en" {
                    "The task could not be saved locally"
                } else {
                    "任务准备失败，请重试"
                }
                .into(),
            );
        });
        return;
    }
    launch_image_colorization_with_billing_scope(
        app,
        context,
        authority,
        Some(billing_scope),
        record,
        false,
    );
}

// TEMP(team-accounts): remove in Task 10 after namespace admission is wired.
pub(super) fn resume_pending_image_colorization(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
) {
    let Ok(plan) = toolbox_replay_plan(&record, "image_colorization") else {
        return;
    };
    let Some(backend) = context.backend.as_ref() else {
        return;
    };
    let Ok(lease) = context.namespace_for(&plan.session) else {
        return;
    };
    let Ok(authority) = context.storage_authority_for(&lease).map(Arc::new) else {
        return;
    };
    if backend.api.begin_user_work(&plan.session).is_err()
        || authority.user_public_id() != plan.session.owner_user_id
        || authority.lease().auth_epoch != plan.session.auth_epoch
    {
        return;
    }
    launch_image_colorization_with_billing_scope(
        app,
        context,
        authority,
        None,
        record,
        true,
    );
}

fn launch_image_colorization_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: Option<BillingScope>,
    record: PendingGenerationRecord,
    recovering: bool,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !generation_scope_matches_context(&context, &session_scope) {
        return;
    }
    let Some(persistence) = context.store.borrow().private_persistence.clone() else { return; };
    if persistence.lease() != authority.lease() { return; }
    let state = app.global::<AppState>();
    if apply_toolbox_completion(&persistence, || {
        state.set_colorize_processing(true);
        state.set_colorize_progress(if recovering { 5 } else { 1 });
        state.set_colorize_estimated_credits("20".into());
        state.set_colorize_result_path("".into());
        state.set_colorize_result_name("".into());
        state.set_colorize_result_image(Image::default());
        state.set_colorize_message(
            if state.get_language().as_str() == "en" {
                if recovering {
                    "Recovering the photo-colorization task..."
                } else {
                    "Uploading the image..."
                }
            } else if recovering {
                "正在恢复未完成的老照片上色任务..."
            } else {
                "正在上传图片..."
            }
            .into(),
        );
    })
    .is_err()
    {
        return;
    }

    let (sender, receiver) = mpsc::channel::<ToolboxRemoteOutcome>();
    let worker_scope = session_scope.clone();
    let worker = match spawn_toolbox_worker(authority.lease().clone(), move |cancel| {
        if cancel.load(Ordering::Acquire) { return; }
        run_image_colorization_worker(
            backend,
            authority,
            billing_scope,
            worker_scope,
            record,
            cancel,
            sender,
        )
    }) {
        Ok(worker) => worker,
        Err(_) => {
            let _ = apply_toolbox_completion(&persistence, || {
                state.set_colorize_processing(false);
                state.set_colorize_message("上色工作线程无法启动".into());
            });
            return;
        }
    };
    poll_image_colorization_outcomes(
        app.as_weak(),
        context,
        session_scope,
        persistence,
        Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
    );
}

fn run_image_colorization_worker(
    backend: Arc<BackendRuntime>,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: Option<BillingScope>,
    session_scope: SessionScope,
    mut record: PendingGenerationRecord,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    sender: mpsc::Sender<ToolboxRemoteOutcome>,
) {
    if cancel.load(Ordering::Acquire) {
        return;
    }
    let Ok(_activity) = backend.api.begin_user_work(&session_scope) else { return; };
    if billing_scope.as_ref().is_some_and(|scope| {
        capture_billing_scope_for_submission(Some(&backend), &authority, scope).is_err()
            || record.billing_account_group_id != scope.request.account_group_id
    })
        || record.owner_user_id != session_scope.owner_user_id
        || record.auth_epoch != session_scope.auth_epoch
        || !backend_generation_scope_active(&backend, &session_scope)
    {
        return;
    }
    if cancel.load(Ordering::Acquire) {
        return;
    }
    let api = GenerationApi::new(backend.api.clone())
        .with_saved_group(&record.billing_account_group_id);
    if record.terminal {
        let prepared = authority
            .delivery_index()
            .map_err(|error| anyhow!(error.to_string()))
            .and_then(|index| {
                prepare_namespace_delivery(
                    &api,
                    authority.clone(),
                    index,
                    &record.identity(),
                    0,
                )
                .map_err(|error| anyhow!(error.to_string()))
            });
        let outcome = match prepared {
            Ok(prepared) => ToolboxRemoteOutcome::Prepared(Box::new(prepared)),
            Err(error) => ToolboxRemoteOutcome::Failure {
                reason: error.to_string(),
            },
        };
        if !cancel.load(Ordering::Acquire) {
            let _ = sender.send(outcome);
        }
        return;
    }

    let mut uploaded = record.uploaded_file_ids.clone();
    if uploaded.is_empty() && record.server_task_id.is_empty() {
        if record.reference_paths.len() != 1
            || record.reference_sha256.len() != 1
            || record.reference_size_bytes.len() != 1
        {
            let _ = sender.send(ToolboxRemoteOutcome::Failure {
                reason: "原图内容已变化，恢复任务已暂停，请重新发起".to_string(),
            });
            return;
        }
        let Some(path) = record.reference_paths.first() else {
            let _ = sender.send(ToolboxRemoteOutcome::Failure {
                reason: "找不到待上色的原图，请重新上传".to_string(),
            });
            return;
        };
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match api.upload_reference_for_namespace_checked(
            Path::new(path),
            &authority,
            &session_scope,
            false,
            &record.reference_sha256[0],
            record.reference_size_bytes[0],
        ) {
            Ok(file_id) => {
                if cancel.load(Ordering::Acquire) {
                    let _ = api.delete_reference_scoped(&file_id, &session_scope);
                    return;
                }
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                if !matches!(
                    apply_generation_patch_for_namespace(
                        &authority,
                        &record.identity(),
                        GenerationRecoveryPatch::UploadedFileIds(snapshot)
                    ),
                    Ok(true)
                ) {
                    if let Some(file_id) = uploaded.last() {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    return;
                }
            }
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    let _ = remove_pending_generation_for_namespace(&authority, &record.identity());
                }
                let _ = sender.send(ToolboxRemoteOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    }

    let mut detail = if record.server_task_id.is_empty() {
        let request = CreateImageColorization {
            client_request_id: record.client_request_id.clone(),
            reference_file_id: uploaded[0].clone(),
        };
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let created: std::result::Result<GenerationTaskDetail, ApiError> =
            if let Some(billing_scope) = billing_scope.as_ref() {
                api.create_image_colorization_billing(&request, billing_scope)
            } else {
                SavedReplayRequest::generation(
                    authority.clone(),
                    &session_scope,
                    &record.client_request_id,
                )
                .map_err(transition_error)
                .and_then(|replay| {
                    backend
                        .api
                        .replay_saved::<GenerationTaskDetail>(&replay)
                        .map(|response| response.data)
                })
            };
        match created {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if error.is_billing_rejection() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &record.identity());
                    let _ = sender.send(ToolboxRemoteOutcome::CreditInsufficient {
                        message: error,
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &record.identity());
                }
                let _ = sender.send(ToolboxRemoteOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    } else {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ToolboxRemoteOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    };

    if validate_toolbox_task_detail(&record, &detail).is_err() {
        let _ = sender.send(ToolboxRemoteOutcome::Failure {
            reason: "服务端返回了不匹配的任务身份或原付款账户，恢复记录已保留".to_string(),
        });
        return;
    }

    record.server_task_id = detail.id.clone();
    let server_task_id = detail.id.clone();
    let server_id_snapshot = server_task_id.clone();
    let uploaded_snapshot = uploaded.clone();
    if !matches!(
        apply_generation_patch_for_namespace(
            &authority,
            &record.identity(),
            GenerationRecoveryPatch::Accepted {
                server_task_id: server_id_snapshot,
                uploaded_file_ids: uploaded_snapshot,
                clear_reference_inputs: false
            }
        ),
        Ok(true)
    ) {
        return;
    }
    let _ = sender.send(ToolboxRemoteOutcome::Accepted {
        task_id: server_task_id.clone(),
    });

    loop {
        if cancel.load(Ordering::Acquire)
            || !backend_generation_scope_active(&backend, &session_scope)
        {
            return;
        }
        let _ = sender.send(ToolboxRemoteOutcome::Progress {
            percent: detail.progress_percent,
        });
        if let Some(item) = detail.items.iter().find(|item| item.status == "succeeded") {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let prepared = authority
                .delivery_index()
                .map_err(|error| anyhow!(error.to_string()))
                .and_then(|index| {
                    prepare_namespace_delivery(
                        &api,
                        authority.clone(),
                        index,
                        &record.identity(),
                        item.index,
                    )
                    .map_err(|error| anyhow!(error.to_string()))
                });
            let outcome = match prepared {
                Ok(prepared) => ToolboxRemoteOutcome::Prepared(Box::new(prepared)),
                Err(error) => ToolboxRemoteOutcome::Failure {
                    reason: error.to_string(),
                },
            };
            if !cancel.load(Ordering::Acquire) {
                let _ = sender.send(outcome);
            }
            return;
        }
        if detail.terminal() {
            let reason = detail
                .failure
                .as_ref()
                .map(|failure| failure.message.clone())
                .or_else(|| {
                    detail.items.iter().find_map(|item| {
                        item.failure.as_ref().map(|failure| failure.message.clone())
                    })
                })
                .unwrap_or_else(|| "服务端未能完成老照片上色".to_string());
            if !matches!(
                apply_generation_patch_for_namespace(
                    &authority,
                    &record.identity(),
                    GenerationRecoveryPatch::Terminal {
                        expected_success_count: 0
                    }
                ),
                Ok(true)
            ) {
                return;
            }
            let _ = sender.send(ToolboxRemoteOutcome::Failure { reason });
            return;
        }
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let next_detail = match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ToolboxRemoteOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        };
        if validate_toolbox_task_detail(&record, &next_detail).is_err() {
            let _ = sender.send(ToolboxRemoteOutcome::Failure {
                reason: "服务端轮询返回了不匹配的任务身份或原付款账户，恢复记录已保留"
                    .to_string(),
            });
            return;
        }
        detail = next_detail;
    }
}

fn poll_image_colorization_outcomes(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    persistence: PrivatePersistence,
    receiver: Rc<RefCell<Option<CapturedToolboxWork<ToolboxRemoteOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_mut() else {
                return;
            };
            match rx.poll_message() {
                Ok(Some(outcome)) if outcome.terminal() => match rx.finish_message(outcome) {
                    Ok(Some(outcome)) => {
                        slot.take();
                        Some(outcome)
                    }
                    Ok(None) => None,
                    Err(_) => {
                        slot.take();
                        Some(ToolboxRemoteOutcome::Failure {
                            reason: "老照片上色任务已中断，请重试".to_string(),
                        })
                    }
                },
                Ok(Some(outcome)) => Some(outcome),
                Ok(None) => None,
                Err(_) => { slot.take(); Some(ToolboxRemoteOutcome::Failure {
                    reason: "老照片上色任务已中断，请重试".to_string(),
                }) }
            }
        };
        let Some(outcome) = outcome else {
            poll_image_colorization_outcomes(
                app_weak,
                context,
                session_scope,
                persistence,
                receiver,
            );
            return;
        };
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if !toolbox_binding_is_current(&context.store, &persistence) {
            receiver.borrow_mut().take();
            return;
        }
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        match outcome {
            ToolboxRemoteOutcome::Accepted { task_id } => {
                if apply_toolbox_completion(&persistence, || {
                    state.set_colorize_progress(state.get_colorize_progress().max(8));
                    state.set_colorize_message(
                        if state.get_language().as_str() == "en" {
                            format!("Task {task_id} is queued")
                        } else {
                            "任务已提交，正在排队处理...".to_string()
                        }
                        .into(),
                    );
                })
                .is_err()
                {
                    receiver.borrow_mut().take();
                    return;
                }
            }
            ToolboxRemoteOutcome::Progress { percent } => {
                if apply_toolbox_completion(&persistence, || {
                    state.set_colorize_progress(percent.clamp(1, 99));
                    state.set_colorize_message(
                        if state.get_language().as_str() == "en" {
                            "Colorizing the photo..."
                        } else {
                            "正在为老照片还原自然色彩..."
                        }
                        .into(),
                    );
                })
                .is_err()
                {
                    receiver.borrow_mut().take();
                    return;
                }
            }
            ToolboxRemoteOutcome::Prepared(prepared) => {
                keep_polling = false;
                match enqueue_toolbox_remote_delivery(
                    &app,
                    context.clone(),
                    persistence.clone(),
                    *prepared,
                    ToolboxRemoteKind::Colorization,
                ) {
                    Ok(()) => {}
                    Err(error) => {
                        let _ = apply_toolbox_completion(&persistence, || {
                            state.set_colorize_processing(false);
                            state.set_colorize_message(
                                format!("上色结果保存失败：{}", zh_error(&error.to_string())).into(),
                            );
                        });
                    }
                }
            }
            ToolboxRemoteOutcome::CreditInsufficient { message } => {
                keep_polling = false;
                let applied = apply_toolbox_completion(&persistence, || {
                    state.set_colorize_processing(false);
                    state.set_colorize_progress(0);
                    if let Some(text) = show_credit_rejection(&state, &message) {
                        state.set_colorize_message(text.into());
                    }
                });
                if applied.is_ok() && context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            ToolboxRemoteOutcome::Failure { reason } => {
                keep_polling = false;
                let applied = apply_toolbox_completion(&persistence, || {
                    state.set_colorize_processing(false);
                    state.set_colorize_progress(0);
                    state.set_colorize_message(reason.into());
                });
                if applied.is_ok() && context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
        }
        if keep_polling {
            poll_image_colorization_outcomes(
                app_weak,
                context,
                session_scope,
                persistence,
                receiver,
            );
        }
    });
}

#[cfg(test)]
mod local_image_tests {
    use super::*;
    use crate::runtime::video_image_callbacks::tests as video_images_tests;

    struct ToolboxFixtureWorkerDrain {
        lease: NamespaceLease,
        activity: Option<UserActivityGate>,
    }

    impl ToolboxFixtureWorkerDrain {
        fn for_fixture(fixture: &video_images_tests::scoped_inputs::Fixture) -> Self {
            Self {
                lease: fixture.persistence.lease().clone(),
                activity: Some(fixture.context.user_activity.clone()),
            }
        }
    }

    impl Drop for ToolboxFixtureWorkerDrain {
        fn drop(&mut self) {
            let joined = join_toolbox_fixture_workers(&self.lease);
            if let Some(activity) = self.activity.take() {
                if let Ok(quiesced) = activity.begin_quiesce(&self.lease) {
                    quiesced.retire();
                }
            }
            if let Err(error) = joined {
                // Preserve an already-unwinding assertion as the primary failure.
                // On every normal exit, a lifecycle failure fails this fixture.
                if !std::thread::panicking() {
                    panic!("toolbox fixture worker drain failed: {error:#}");
                }
            }
        }
    }

    struct ReleaseWorkerOnDrop(Arc<std::sync::atomic::AtomicBool>);

    impl ReleaseWorkerOnDrop {
        fn new(release: &Arc<std::sync::atomic::AtomicBool>) -> Self {
            Self(Arc::clone(release))
        }
    }

    impl Drop for ReleaseWorkerOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    fn join_toolbox_fixture_workers(lease: &NamespaceLease) -> Result<()> {
        cancel_delivery_commit_workers(lease);
        let mut failures = Vec::new();
        if let Err(error) = drain_toolbox_workers_for_lease_for_test(lease) {
            failures.push(format!("toolbox: {error:#}"));
        }
        if let Err(error) = drain_delivery_commit_workers_for_lease_for_test(lease) {
            failures.push(format!("delivery: {error:#}"));
        }
        if let Err(error) = drain_activation_preview_workers_for_lease_for_test(lease) {
            failures.push(format!("activation preview: {error:#}"));
        }
        anyhow::ensure!(failures.is_empty(), failures.join("; "));
        Ok(())
    }

    fn drain_toolbox_fixture(fixture: &video_images_tests::scoped_inputs::Fixture) {
        join_toolbox_fixture_workers(fixture.persistence.lease()).unwrap();
        fixture.drain();
    }

    fn toolbox_test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("artforge-toolbox-{label}-{}", Uuid::new_v4()))
    }

    fn toolbox_test_item(source_path: &Path, result_path: &Path) -> CompressionImageItem {
        CompressionImageItem {
            id: Uuid::new_v4().to_string().into(),
            name: "test.png".into(),
            source_path: source_path.display().to_string().into(),
            size_text: "1 KB".into(),
            image: Image::default(),
            status: "completed".into(),
            result_path: result_path.display().to_string().into(),
        }
    }

    #[test]
    fn managed_toolbox_removal_never_crosses_the_exact_directory_boundary() {
        let test_root = toolbox_test_root("path-safety");
        let managed_directory =
            managed_toolbox_directory(&test_root, ManagedToolboxDirectory::CompressionInputs);
        let other_managed_directory =
            managed_toolbox_directory(&test_root, ManagedToolboxDirectory::ConversionInputs);
        fs::create_dir_all(&managed_directory).expect("create managed directory");
        fs::create_dir_all(&other_managed_directory).expect("create other managed directory");

        let managed_file = managed_directory.join("pasted-managed.png");
        let outside_file = test_root.join("outside.png");
        let wrong_kind_file = other_managed_directory.join("pasted-other.png");
        let traversal_target = test_root.join("toolbox").join("escaped.png");
        let traversal_path = managed_directory.join("..").join("escaped.png");
        fs::write(&managed_file, b"managed").expect("write managed file");
        fs::write(&outside_file, b"outside").expect("write outside file");
        fs::write(&wrong_kind_file, b"other").expect("write other managed file");
        fs::write(&traversal_target, b"escaped").expect("write traversal target");

        assert!(!remove_managed_toolbox_file(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
            &outside_file,
        ));
        assert!(!remove_managed_toolbox_file(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
            &wrong_kind_file,
        ));
        assert!(!remove_managed_toolbox_file(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
            &traversal_path,
        ));
        assert!(outside_file.is_file());
        assert!(wrong_kind_file.is_file());
        assert!(traversal_target.is_file());
        assert!(remove_managed_toolbox_file(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
            &managed_file,
        ));
        assert!(!managed_file.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let linked_file = managed_directory.join("linked-outside.png");
            symlink(&outside_file, &linked_file).expect("create file symlink");
            assert!(!remove_managed_toolbox_file(
                &test_root,
                ManagedToolboxDirectory::CompressionInputs,
                &linked_file,
            ));
            assert!(linked_file.exists());
            assert!(outside_file.is_file());
        }

        let _ = fs::remove_dir_all(test_root);
    }

    #[cfg(unix)]
    #[test]
    fn managed_toolbox_directory_symlink_never_deletes_its_target() {
        use std::os::unix::fs::symlink;

        let test_root = toolbox_test_root("directory-symlink");
        let toolbox_directory = test_root.join("toolbox");
        let works_directory = test_root.join("out");
        fs::create_dir_all(&toolbox_directory).expect("create toolbox root");
        fs::create_dir_all(&works_directory).expect("create works directory");
        let work = works_directory.join("generated-work.png");
        fs::write(&work, b"user work").expect("write user work");
        let linked_directory =
            managed_toolbox_directory(&test_root, ManagedToolboxDirectory::CompressionInputs);
        symlink(&works_directory, &linked_directory).expect("create directory symlink");

        cleanup_stale_toolbox_files_in(
            &test_root,
            std::time::SystemTime::now() + Duration::from_secs(2),
            Duration::ZERO,
        );
        assert!(work.is_file());
        assert!(!remove_managed_toolbox_file(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
            &linked_directory.join("generated-work.png"),
        ));
        assert!(work.is_file());

        fs::remove_file(&linked_directory).expect("remove directory symlink");
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn removing_an_item_cleans_only_its_managed_input_and_result() {
        let test_root = toolbox_test_root("item-cleanup");
        let input_directory =
            managed_toolbox_directory(&test_root, ManagedToolboxDirectory::CompressionInputs);
        let result_directory =
            managed_toolbox_directory(&test_root, ManagedToolboxDirectory::CompressionResults);
        fs::create_dir_all(&input_directory).expect("create input directory");
        fs::create_dir_all(&result_directory).expect("create result directory");
        let input = input_directory.join("pasted-input.png");
        let result = result_directory.join("compressed.png");
        fs::write(&input, b"input").expect("write input");
        fs::write(&result, b"result").expect("write result");

        remove_toolbox_item_files(
            &test_root,
            &toolbox_test_item(&input, &result),
            ManagedToolboxDirectory::CompressionInputs,
            ManagedToolboxDirectory::CompressionResults,
        );
        assert!(!input.exists());
        assert!(!result.exists());

        let external_input = test_root.join("external-input.png");
        let external_result = test_root.join("external-result.png");
        fs::write(&external_input, b"external input").expect("write external input");
        fs::write(&external_result, b"external result").expect("write external result");
        remove_toolbox_item_files(
            &test_root,
            &toolbox_test_item(&external_input, &external_result),
            ManagedToolboxDirectory::CompressionInputs,
            ManagedToolboxDirectory::CompressionResults,
        );
        assert!(external_input.is_file());
        assert!(external_result.is_file());

        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn successful_export_releases_only_a_managed_temporary_result() {
        let test_root = toolbox_test_root("export-cleanup");
        let result_directory =
            managed_toolbox_directory(&test_root, ManagedToolboxDirectory::ConversionResults);
        let export_directory = test_root.join("exports");
        fs::create_dir_all(&result_directory).expect("create result directory");
        fs::create_dir_all(&export_directory).expect("create export directory");

        let managed_result = result_directory.join("converted.png");
        let exported = export_directory.join("converted.png");
        fs::write(&managed_result, b"converted").expect("write result");
        assert!(
            copy_and_release_managed_toolbox_result(
                &managed_result,
                &exported,
                &test_root,
                ManagedToolboxDirectory::ConversionResults,
            )
            .expect("export managed result")
        );
        assert!(!managed_result.exists());
        assert_eq!(fs::read(&exported).expect("read export"), b"converted");

        let external_result = test_root.join("external-result.png");
        let external_export = export_directory.join("external.png");
        fs::write(&external_result, b"external").expect("write external result");
        assert!(
            !copy_and_release_managed_toolbox_result(
                &external_result,
                &external_export,
                &test_root,
                ManagedToolboxDirectory::ConversionResults,
            )
            .expect("export external result")
        );
        assert!(external_result.is_file());
        assert_eq!(
            fs::read(&external_export).expect("read external export"),
            b"external"
        );

        let mut items = vec![
            toolbox_test_item(Path::new("source-a"), &managed_result),
            toolbox_test_item(Path::new("source-b"), &external_result),
        ];
        assert!(clear_released_toolbox_result(&mut items, &managed_result));
        assert!(items[0].result_path.is_empty());
        assert_eq!(
            Path::new(items[1].result_path.as_str()),
            external_result.as_path()
        );

        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn stale_cleanup_scans_only_known_direct_toolbox_files() {
        let test_root = toolbox_test_root("stale-cleanup");
        let mut stale_files = Vec::new();
        for (index, directory) in MANAGED_TOOLBOX_DIRECTORIES.into_iter().enumerate() {
            let path = managed_toolbox_directory(&test_root, directory);
            fs::create_dir_all(&path).expect("create managed directory");
            let file = path.join(format!("stale-{index}.tmp"));
            fs::write(&file, b"stale").expect("write stale file");
            stale_files.push(file);
        }
        let compression_inputs =
            managed_toolbox_directory(&test_root, ManagedToolboxDirectory::CompressionInputs);
        let nested_directory = compression_inputs.join("nested");
        fs::create_dir_all(&nested_directory).expect("create nested directory");
        let nested_file = nested_directory.join("nested.tmp");
        fs::write(&nested_file, b"nested").expect("write nested file");
        let unknown_directory = test_root.join("toolbox").join("unknown");
        fs::create_dir_all(&unknown_directory).expect("create unknown directory");
        let unknown_file = unknown_directory.join("unknown.tmp");
        fs::write(&unknown_file, b"unknown").expect("write unknown file");

        cleanup_stale_toolbox_files_in(
            &test_root,
            std::time::SystemTime::now() + Duration::from_secs(2),
            Duration::ZERO,
        );
        assert!(stale_files.iter().all(|path| !path.exists()));
        assert!(nested_file.is_file());
        assert!(unknown_file.is_file());

        let fresh_file = compression_inputs.join("fresh.tmp");
        fs::write(&fresh_file, b"fresh").expect("write fresh file");
        cleanup_stale_toolbox_files_in(
            &test_root,
            std::time::SystemTime::now(),
            TOOLBOX_TEMP_FILE_MAX_AGE,
        );
        assert!(fresh_file.is_file());

        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn local_compression_worker_preserves_detected_format_and_reports_batch_results() {
        let test_root =
            std::env::temp_dir().join(format!("artforge-compression-test-{}", Uuid::new_v4()));
        let source = test_root.join("source.jpg");
        let missing = test_root.join("missing.png");
        let output_dir = test_root.join("results");
        fs::create_dir_all(&test_root).expect("create compression test directory");
        let rgba = image::RgbaImage::from_fn(48, 32, |x, y| {
            image::Rgba([
                ((x * 13 + y * 7) % 256) as u8,
                ((x * 5 + y * 17) % 256) as u8,
                ((x * 19 + y * 3) % 256) as u8,
                255,
            ])
        });
        let source_bytes =
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode source png");
        fs::write(&source, source_bytes).expect("write png with jpeg extension");

        let (sender, receiver) = mpsc::channel();
        run_local_compression_worker(
            vec![
                CompressionInput {
                    id: "valid".to_string(),
                    source_path: source.display().to_string(),
                },
                CompressionInput {
                    id: "missing".to_string(),
                    source_path: missing.display().to_string(),
                },
            ],
            ImageCompressionMode::Quality(75),
            output_dir,
            sender,
        );
        let outcomes = receiver.into_iter().collect::<Vec<_>>();

        assert!(matches!(
            outcomes.first(),
            Some(CompressionOutcome::Started { id }) if id == "valid"
        ));
        let result_path = match outcomes.get(1) {
            Some(CompressionOutcome::Completed {
                id,
                result_path,
                size_text,
            }) if id == "valid" && !size_text.is_empty() => PathBuf::from(result_path),
            _ => panic!("expected a completed compression result"),
        };
        assert!(matches!(
            outcomes.get(2),
            Some(CompressionOutcome::Started { id }) if id == "missing"
        ));
        assert!(matches!(
            outcomes.get(3),
            Some(CompressionOutcome::Failed { id }) if id == "missing"
        ));
        assert!(matches!(
            outcomes.get(4),
            Some(CompressionOutcome::Finished {
                succeeded: 1,
                failed: 1
            })
        ));
        assert_eq!(
            result_path.extension().and_then(|value| value.to_str()),
            Some("png")
        );
        assert_eq!(
            image::guess_format(&fs::read(&result_path).expect("read compression result"))
                .expect("detect compression result"),
            image::ImageFormat::Png
        );
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn compression_save_destination_uses_the_result_extension() {
        let directory = Path::new("chosen-folder");
        assert_eq!(
            normalize_compression_destination(&directory.join("image"), "png"),
            directory.join("image.png")
        );
        assert_eq!(
            normalize_compression_destination(&directory.join("image.jpg"), "webp"),
            directory.join("image.webp")
        );
        assert_eq!(
            normalize_compression_destination(&directory.join("image.WEBP"), "webp"),
            directory.join("image.WEBP")
        );
    }

    fn pending_toolbox_record(task_type: &str) -> PendingGenerationRecord {
        PendingGenerationRecord {
            source_asset_id: String::new(),
            schema_version: 2,
            cancel_requested: false,
            created_at_epoch_ms: 1,
            client_request_id: "saved-request-key".to_string(),
            owner_user_id: "11111111-1111-4111-8111-111111111111".to_string(),
            billing_account_group_id: "22222222-2222-4222-8222-222222222222".to_string(),
            auth_epoch: 17,
            local_task_id: "local-task".to_string(),
            server_task_id: "server-task".to_string(),
            raw_prompt: "saved prompt".to_string(),
            generation_prompt: String::new(),
            task_type: task_type.to_string(),
            category: "other".to_string(),
            mode: "game".to_string(),
            ratio: String::new(),
            quality: "standard".to_string(),
            model_code: "saved-model".to_string(),
            conversation_id: String::new(),
            count: 1,
            target_width: 0,
            target_height: 0,
            create_conversation: false,
            reference_paths: vec!["/captured/source.png".to_string()],
            reference_sha256: vec!["saved-sha".to_string()],
            reference_size_bytes: vec![91],
            lineage_reference_paths: vec!["/captured/source.png".to_string()],
            uploaded_file_ids: vec!["saved-upload".to_string()],
            video_request: None,
            deliveries: vec![],
            terminal: false,
            expected_success_count: 0,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }

    #[test]
    fn saved_toolbox_replay_plan_preserves_exact_owner_payer_key_type_and_sources() {
        let record = pending_toolbox_record("image_watermark_removal");
        let plan = toolbox_replay_plan(&record, "image_watermark_removal")
            .expect("exact retained record");
        assert_eq!(plan.session.owner_user_id, record.owner_user_id);
        assert_eq!(plan.session.auth_epoch, record.auth_epoch);
        assert_eq!(plan.billing_account_group_id, record.billing_account_group_id);
        assert_eq!(plan.client_request_id, record.client_request_id);
        assert_eq!(plan.task_type, record.task_type);
        assert_eq!(plan.reference_paths, record.reference_paths);
        assert_eq!(plan.uploaded_file_ids, record.uploaded_file_ids);
        assert_eq!(plan.server_task_id, record.server_task_id);

        let mut ambiguous_create = record;
        ambiguous_create.server_task_id.clear();
        let ambiguous = toolbox_replay_plan(&ambiguous_create, "image_watermark_removal")
            .expect("ambiguous saved POST retains the original replay authority");
        assert!(ambiguous.server_task_id.is_empty());
        assert_eq!(ambiguous.client_request_id, "saved-request-key");
        assert_eq!(
            ambiguous.billing_account_group_id,
            "22222222-2222-4222-8222-222222222222"
        );
    }

    #[test]
    fn toolbox_detail_validation_retains_exact_payer_and_bound_task_identity() {
        let record = pending_toolbox_record("image_watermark_removal");
        let task_type = record.task_type.clone();
        let detail = |id: &str, payer: &str| GenerationTaskDetail {
            id: id.to_string(),
            billing_account_group_id: payer.to_string(),
            status: "processing".to_string(),
            progress_percent: 20,
            success_count: 0,
            failure_count: 0,
            failure: None,
            prompt: None,
            result_prompt: None,
            request: serde_json::Value::Null,
            model: None,
            quality: String::new(),
            requested_count: 1,
            task_type: task_type.clone(),
            items: Vec::new(),
        };
        assert!(validate_toolbox_task_detail(
            &record,
            &detail(&record.server_task_id, &record.billing_account_group_id),
        )
        .is_ok());
        assert!(validate_toolbox_task_detail(
            &record,
            &detail("different-task", &record.billing_account_group_id),
        )
        .is_err());
        assert!(validate_toolbox_task_detail(
            &record,
            &detail(&record.server_task_id, "different-payer"),
        )
        .is_err());
        assert!(validate_toolbox_task_detail(
            &record,
            &detail("", &record.billing_account_group_id),
        )
        .is_err());

        let mut unbound = record;
        unbound.server_task_id.clear();
        assert!(validate_toolbox_task_detail(
            &unbound,
            &detail("first-server-task", &unbound.billing_account_group_id),
        )
        .is_ok());
    }

    #[test]
    fn saved_toolbox_replay_never_substitutes_a_different_operation() {
        let record = pending_toolbox_record("image_colorization");
        assert!(toolbox_replay_plan(&record, "image_watermark_removal").is_err());
        let mut missing_payer = record.clone();
        missing_payer.billing_account_group_id.clear();
        assert!(toolbox_replay_plan(&missing_payer, "image_colorization").is_err());
        let mut missing_key = record;
        missing_key.client_request_id.clear();
        assert!(toolbox_replay_plan(&missing_key, "image_colorization").is_err());
    }

    #[test]
    fn actual_local_toolbox_callbacks_fail_closed_without_captured_authority() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_compression_target_mb("unchanged".into());
        state.set_conversion_message("unchanged".into());
        wire_toolbox_callbacks(&app, AppContext::default());
        state.invoke_update_compression_target_preview("1024".into());
        state.invoke_start_conversion();
        assert_eq!(state.get_compression_target_mb().as_str(), "unchanged");
        assert_eq!(state.get_conversion_message().as_str(), "unchanged");
    }

    #[test]
    fn native_picker_callbacks_never_apply_an_old_dialog_to_a_new_binding() {
        i_slint_backend_testing::init_no_event_loop();
        let original = video_images_tests::scoped_inputs::Fixture::new();
        let replacement = video_images_tests::scoped_inputs::Fixture::new();
        let _original_workers = ToolboxFixtureWorkerDrain::for_fixture(&original);
        let _replacement_workers = ToolboxFixtureWorkerDrain::for_fixture(&replacement);
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("picker-source.png");
        let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([80, 120, 200, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_crop_source_path("crop-sentinel".into());
        state.set_watermark_source_path("watermark-sentinel".into());
        state.set_colorize_source_path("colorize-sentinel".into());
        wire_toolbox_callbacks(&app, original.context.clone());

        let invoke = |paths: Vec<PathBuf>, action: &dyn Fn()| {
            original.context.store.borrow_mut().private_persistence =
                Some(original.persistence.clone());
            TOOLBOX_OPEN_PICKER_FIXTURE.with(|fixture| {
                *fixture.borrow_mut() = vec![paths];
            });
            let store = original.context.store.clone();
            let replacement_persistence = replacement.persistence.clone();
            TOOLBOX_PICKER_RETURN_HOOK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move || {
                    store.borrow_mut().private_persistence = Some(replacement_persistence);
                }));
            });
            action();
        };

        invoke(vec![source.clone()], &|| state.invoke_choose_compression_images());
        assert_eq!(state.get_compression_images().row_count(), 0);
        invoke(vec![source.clone()], &|| state.invoke_choose_conversion_images());
        assert_eq!(state.get_conversion_images().row_count(), 0);
        invoke(vec![source.clone()], &|| state.invoke_choose_watermark_source());
        assert_eq!(state.get_watermark_source_path().as_str(), "watermark-sentinel");
        invoke(vec![source.clone()], &|| state.invoke_choose_colorize_source());
        assert_eq!(state.get_colorize_source_path().as_str(), "colorize-sentinel");
        invoke(vec![source], &|| state.invoke_choose_crop_source());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        slint::platform::update_timers_and_animations();
        assert_eq!(state.get_crop_source_path().as_str(), "crop-sentinel");

        drain_toolbox_fixture(&original);
        drain_toolbox_fixture(&replacement);
    }

    #[test]
    fn native_picker_callbacks_discard_every_selection_returned_after_426() {
        i_slint_backend_testing::init_no_event_loop();
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("picker-after-426.png");
        let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([80, 120, 200, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        for kind in ["compression", "conversion", "crop", "watermark", "colorization"] {
            let fixture = video_images_tests::scoped_inputs::Fixture::new();
            let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
            let app = AppWindow::new().unwrap();
            let state = app.global::<AppState>();
            wire_toolbox_callbacks(&app, fixture.context.clone());
            TOOLBOX_OPEN_PICKER_FIXTURE.with(|slot| {
                *slot.borrow_mut() = vec![vec![source.clone()]];
            });
            let latch = fixture.persistence.upgrade_latch().clone();
            TOOLBOX_PICKER_RETURN_HOOK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move || {
                    latch.trip(RequiredUpgrade { minimum_version: None });
                }));
            });
            match kind {
                "compression" => state.invoke_choose_compression_images(),
                "conversion" => state.invoke_choose_conversion_images(),
                "crop" => state.invoke_choose_crop_source(),
                "watermark" => state.invoke_choose_watermark_source(),
                "colorization" => state.invoke_choose_colorize_source(),
                _ => unreachable!(),
            }
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
            slint::platform::update_timers_and_animations();
            assert_eq!(state.get_compression_images().row_count(), 0);
            assert_eq!(state.get_conversion_images().row_count(), 0);
            assert!(state.get_crop_source_path().is_empty());
            assert!(state.get_watermark_source_path().is_empty());
            assert!(state.get_colorize_source_path().is_empty());
            drain_toolbox_fixture(&fixture);
        }
    }

    #[test]
    fn captured_billed_source_callbacks_preserve_the_running_request_source() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let external = tempfile::tempdir().unwrap();
        let candidate = external.path().join("replacement.png");
        let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([70, 130, 220, 255]));
        fs::write(
            &candidate,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_watermark_source_path("running-watermark-source".into());
        state.set_colorize_source_path("running-colorize-source".into());
        state.set_watermark_processing(true);
        state.set_colorize_processing(true);

        assert!(add_watermark_paths_for_store(
            &app,
            &fixture.context.store,
            vec![candidate.clone()],
        ));
        assert!(add_colorization_paths_for_store(
            &app,
            &fixture.context.store,
            vec![candidate.clone()],
        ));
        assert!(add_watermark_drag_for_store(
            &app,
            &fixture.context.store,
            TEXT_PLAIN_MIME,
            &candidate.display().to_string(),
        ));
        assert!(add_colorization_drag_for_store(
            &app,
            &fixture.context.store,
            TEXT_PLAIN_MIME,
            &candidate.display().to_string(),
        ));
        assert_eq!(
            state.get_watermark_source_path().as_str(),
            "running-watermark-source"
        );
        assert_eq!(
            state.get_colorize_source_path().as_str(),
            "running-colorize-source"
        );
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_crop_callback_fails_closed_without_captured_authority() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_crop_source_width(400);
        state.set_crop_source_height(200);
        state.set_crop_ratio("sentinel".into());
        wire_toolbox_callbacks(&app, AppContext::default());
        state.invoke_set_crop_ratio("1:1".into());
        assert_eq!(state.get_crop_ratio().as_str(), "sentinel");
    }

    #[test]
    fn actual_billed_callbacks_fail_closed_without_captured_authority() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_watermark_message("watermark-sentinel".into());
        state.set_colorize_message("colorize-sentinel".into());
        wire_toolbox_callbacks(&app, AppContext::default());
        state.invoke_start_watermark_removal();
        state.invoke_start_colorize();
        assert_eq!(state.get_watermark_message().as_str(), "watermark-sentinel");
        assert_eq!(state.get_colorize_message().as_str(), "colorize-sentinel");
    }

    #[test]
    fn actual_reveal_callbacks_fail_closed_before_os_effects() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_watermark_message("watermark-sentinel".into());
        state.set_colorize_message("colorize-sentinel".into());
        wire_toolbox_callbacks(&app, AppContext::default());
        state.invoke_reveal_watermark_result();
        state.invoke_reveal_colorize_result();
        assert_eq!(state.get_watermark_message().as_str(), "watermark-sentinel");
        assert_eq!(state.get_colorize_message().as_str(), "colorize-sentinel");
    }

    #[test]
    fn actual_clipboard_callbacks_use_isolated_bytes_and_publish_only_owned_inputs() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        wire_toolbox_callbacks(&app, fixture.context.clone());
        let clipboard_image = || ToolboxClipboardContent::Image {
            width: 3,
            height: 2,
            rgba: vec![40, 90, 180, 255, 40, 90, 180, 255, 40, 90, 180, 255,
                       40, 90, 180, 255, 40, 90, 180, 255, 40, 90, 180, 255],
        };

        TOOLBOX_CLIPBOARD_FIXTURE.with(|slot| {
            *slot.borrow_mut() = Some(clipboard_image());
        });
        assert!(state.invoke_paste_compression_images());
        let compression = state.get_compression_images().row_data(0).unwrap();
        assert!(Path::new(compression.source_path.as_str())
            .starts_with(fixture.authority.lease().namespace.output_dir()));

        TOOLBOX_CLIPBOARD_FIXTURE.with(|slot| {
            *slot.borrow_mut() = Some(clipboard_image());
        });
        assert!(state.invoke_paste_conversion_images());
        let conversion = state.get_conversion_images().row_data(0).unwrap();
        assert!(Path::new(conversion.source_path.as_str())
            .starts_with(fixture.authority.lease().namespace.output_dir()));

        TOOLBOX_CLIPBOARD_FIXTURE.with(|slot| {
            *slot.borrow_mut() = Some(clipboard_image());
        });
        assert!(state.invoke_paste_crop_source());
        assert!(Path::new(state.get_crop_source_path().as_str())
            .starts_with(fixture.authority.lease().namespace.output_dir()));
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn store_aware_drag_bridges_read_explicit_sources_through_captured_authority() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("drag-source.png");
        let rgba = image::RgbaImage::from_pixel(5, 4, image::Rgba([80, 140, 200, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let data = source.display().to_string();
        assert!(add_compression_drag_for_store(
            &app,
            &fixture.context.store,
            TEXT_PLAIN_MIME,
            &data,
        ));
        assert!(add_conversion_drag_for_store(
            &app,
            &fixture.context.store,
            TEXT_PLAIN_MIME,
            &data,
        ));
        assert!(add_crop_drag_for_store(
            &app,
            &fixture.context.store,
            TEXT_PLAIN_MIME,
            &data,
        ));
        assert!(add_watermark_drag_for_store(
            &app,
            &fixture.context.store,
            TEXT_PLAIN_MIME,
            &data,
        ));
        assert!(add_colorization_drag_for_store(
            &app,
            &fixture.context.store,
            TEXT_PLAIN_MIME,
            &data,
        ));
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_local_callback_accepts_current_namespace_then_rejects_late_426_work() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        wire_toolbox_callbacks(&app, fixture.context.clone());
        state.invoke_update_compression_target_preview("1024".into());
        assert_eq!(state.get_compression_target_mb().as_str(), "1.00");
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade {
            minimum_version: Some("99.0.0".into()),
        });
        state.invoke_update_compression_target_preview("2048".into());
        assert_eq!(state.get_compression_target_mb().as_str(), "1.00");
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_compression_callback_reads_captured_source_and_publishes_owned_result() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("actual-compression.png");
        let rgba = image::RgbaImage::from_pixel(8, 6, image::Rgba([20, 80, 180, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_compression_mode("quality".into());
        state.set_compression_quality(90);
        set_compression_images(
            &state,
            vec![CompressionImageItem {
                id: "actual-compression".into(),
                name: "actual-compression.png".into(),
                source_path: source.display().to_string().into(),
                size_text: "1 KB".into(),
                image: Image::default(),
                status: "pending".into(),
                result_path: "".into(),
            }],
        );
        wire_toolbox_callbacks(&app, fixture.context.clone());
        state.invoke_start_compression();
        video_images_tests::scoped_inputs::pump(|| {
            !app.global::<AppState>().get_compression_processing()
        });
        let item = state.get_compression_images().row_data(0).unwrap();
        assert_eq!(item.status.as_str(), "completed");
        let result = PathBuf::from(item.result_path.to_string());
        assert!(result.starts_with(fixture.authority.lease().namespace.output_dir()));
        assert!(fixture
            .authority
            .read_image_source(&result, 100 * 1024 * 1024)
            .is_ok());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_conversion_callback_reads_captured_source_and_publishes_owned_result() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("actual-conversion.png");
        let rgba = image::RgbaImage::from_pixel(8, 6, image::Rgba([120, 30, 210, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_conversion_target_format("webp".into());
        set_conversion_images(
            &state,
            vec![CompressionImageItem {
                id: "actual-conversion".into(),
                name: "actual-conversion.png".into(),
                source_path: source.display().to_string().into(),
                size_text: "1 KB".into(),
                image: Image::default(),
                status: "pending".into(),
                result_path: "".into(),
            }],
        );
        wire_toolbox_callbacks(&app, fixture.context.clone());
        state.invoke_start_conversion();
        video_images_tests::scoped_inputs::pump(|| {
            !app.global::<AppState>().get_conversion_processing()
        });
        let item = state.get_conversion_images().row_data(0).unwrap();
        assert_eq!(item.status.as_str(), "completed");
        let result = PathBuf::from(item.result_path.to_string());
        assert!(result.starts_with(fixture.authority.lease().namespace.output_dir()));
        let bytes = fixture
            .authority
            .read_image_source(&result, 100 * 1024 * 1024)
            .unwrap();
        assert_eq!(image::guess_format(&bytes).unwrap(), image::ImageFormat::WebP);
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_remove_black_callback_runs_in_registered_worker_and_publishes_owned_result() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("actual-remove-black.png");
        let mut rgba = image::RgbaImage::from_pixel(6, 4, image::Rgba([220, 30, 20, 255]));
        rgba.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_watermark_source_path(source.display().to_string().into());
        wire_toolbox_callbacks(&app, fixture.context.clone());
        state.invoke_start_remove_black_tool();
        video_images_tests::scoped_inputs::pump(|| {
            !app.global::<AppState>().get_watermark_processing()
                && !app
                    .global::<AppState>()
                    .get_watermark_result_path()
                    .is_empty()
        });
        let result = PathBuf::from(state.get_watermark_result_path().to_string());
        assert!(result.starts_with(fixture.authority.lease().namespace.output_dir()));
        assert!(fixture
            .authority
            .read_image_source(&result, 100 * 1024 * 1024)
            .is_ok());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_export_callbacks_use_held_external_destination_without_overwrite() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        let rgba = image::RgbaImage::from_pixel(5, 4, image::Rgba([12, 90, 160, 255]));
        let bytes = encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap();
        let compression_result =
            write_toolbox_owned_output(&fixture.authority, "compression-export-source.png", &bytes)
                .unwrap();
        let conversion_result =
            write_toolbox_owned_output(&fixture.authority, "conversion-export-source.png", &bytes)
                .unwrap();
        remember_toolbox_temporary_output(fixture.persistence.lease(), &compression_result);
        remember_toolbox_temporary_output(fixture.persistence.lease(), &conversion_result);
        set_compression_images(&state, vec![CompressionImageItem {
            id: "compression-export".into(),
            name: "source.png".into(),
            source_path: compression_result.display().to_string().into(),
            size_text: "1 KB".into(),
            image: Image::default(),
            status: "completed".into(),
            result_path: compression_result.display().to_string().into(),
        }]);
        set_conversion_images(&state, vec![CompressionImageItem {
            id: "conversion-export".into(),
            name: "source.png".into(),
            source_path: conversion_result.display().to_string().into(),
            size_text: "1 KB".into(),
            image: Image::default(),
            status: "completed".into(),
            result_path: conversion_result.display().to_string().into(),
        }]);
        wire_toolbox_callbacks(&app, fixture.context.clone());
        let external = tempfile::tempdir().unwrap();
        let compression_destination = external.path().join("compressed.png");
        let conversion_destination = external.path().join("converted.png");
        TOOLBOX_EXPORT_FIXTURE.with(|slot| {
            *slot.borrow_mut() = vec![
                compression_destination.clone(),
                conversion_destination.clone(),
            ];
        });

        state.invoke_save_compression_result("compression-export".into());
        video_images_tests::scoped_inputs::pump(|| {
            compression_destination.is_file() && !state.get_compression_saving()
        });
        assert!(fs::read(&compression_destination).is_ok_and(|saved| saved == bytes));

        state.invoke_save_conversion_result("conversion-export".into());
        video_images_tests::scoped_inputs::pump(|| {
            conversion_destination.is_file() && !state.get_conversion_saving()
        });
        assert!(fs::read(&conversion_destination).is_ok_and(|saved| saved == bytes));
        assert!(compression_result.exists());
        assert!(conversion_result.exists());

        let overwrite_source =
            write_toolbox_owned_output(&fixture.authority, "overwrite-source.png", &bytes).unwrap();
        let existing_destination = external.path().join("existing.png");
        fs::write(&existing_destination, b"sentinel").unwrap();
        set_compression_images(&state, vec![CompressionImageItem {
            id: "overwrite-export".into(),
            name: "source.png".into(),
            source_path: overwrite_source.display().to_string().into(),
            size_text: "1 KB".into(),
            image: Image::default(),
            status: "completed".into(),
            result_path: overwrite_source.display().to_string().into(),
        }]);
        TOOLBOX_EXPORT_FIXTURE.with(|slot| {
            *slot.borrow_mut() = vec![existing_destination.clone()];
        });
        state.invoke_save_compression_result("overwrite-export".into());
        video_images_tests::scoped_inputs::pump(|| {
            state.get_compression_message().contains("失败")
                || state.get_compression_message().contains("could not")
        });
        assert_eq!(fs::read(&existing_destination).unwrap(), b"sentinel");
        assert!(overwrite_source.exists());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn export_dialogs_reject_binding_replacement_and_426_before_external_write() {
        i_slint_backend_testing::init_no_event_loop();
        for (kind, trip) in [
            ("compression", false),
            ("conversion", false),
            ("compression", true),
            ("conversion", true),
        ] {
            let fixture = video_images_tests::scoped_inputs::Fixture::new();
            let replacement = video_images_tests::scoped_inputs::Fixture::new();
            let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
            let _replacement_workers = ToolboxFixtureWorkerDrain::for_fixture(&replacement);
            let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 100, 190, 255]));
            let bytes = encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap();
            let leaf = format!("{kind}-dialog-source.png");
            let result = write_toolbox_owned_output(&fixture.authority, &leaf, &bytes).unwrap();
            remember_toolbox_temporary_output(fixture.persistence.lease(), &result);
            let app = AppWindow::new().unwrap();
            let state = app.global::<AppState>();
            let item = CompressionImageItem {
                id: "dialog-export".into(),
                name: "source.png".into(),
                source_path: result.display().to_string().into(),
                size_text: "1 KB".into(),
                image: Image::default(),
                status: "completed".into(),
                result_path: result.display().to_string().into(),
            };
            if kind == "compression" {
                set_compression_images(&state, vec![item]);
            } else {
                set_conversion_images(&state, vec![item]);
            }
            wire_toolbox_callbacks(&app, fixture.context.clone());
            let external = tempfile::tempdir().unwrap();
            let destination = external.path().join(format!("{kind}-must-not-write.png"));
            TOOLBOX_EXPORT_FIXTURE.with(|slot| {
                *slot.borrow_mut() = vec![destination.clone()];
            });
            if trip {
                let latch = fixture.persistence.upgrade_latch().clone();
                TOOLBOX_PICKER_RETURN_HOOK.with(|slot| {
                    *slot.borrow_mut() = Some(Box::new(move || {
                        latch.trip(RequiredUpgrade { minimum_version: None });
                    }));
                });
            } else {
                let store = fixture.context.store.clone();
                let replacement_persistence = replacement.persistence.clone();
                TOOLBOX_PICKER_RETURN_HOOK.with(|slot| {
                    *slot.borrow_mut() = Some(Box::new(move || {
                        store.borrow_mut().private_persistence = Some(replacement_persistence);
                    }));
                });
            }
            if kind == "compression" {
                state.invoke_save_compression_result("dialog-export".into());
            } else {
                state.invoke_save_conversion_result("dialog-export".into());
            }
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
            slint::platform::update_timers_and_animations();
            assert!(!destination.exists());
            assert!(result.exists());
            assert!(!state.get_compression_saving());
            assert!(!state.get_conversion_saving());
            drain_toolbox_fixture(&fixture);
            drain_toolbox_fixture(&replacement);
        }
    }

    #[test]
    fn actual_remove_callback_retains_registered_output_without_consumption_proof() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("external-source.png");
        fs::write(&source, b"external-sentinel").unwrap();
        let result = write_toolbox_owned_output(
            &fixture.authority,
            "owned-result.png",
            b"owned-sentinel",
        )
        .unwrap();
        remember_toolbox_temporary_output(fixture.persistence.lease(), &result);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        set_compression_images(&state, vec![CompressionImageItem {
            id: "remove-owned".into(),
            name: "source.png".into(),
            source_path: source.display().to_string().into(),
            size_text: "1 KB".into(),
            image: Image::default(),
            status: "completed".into(),
            result_path: result.display().to_string().into(),
        }]);
        wire_toolbox_callbacks(&app, fixture.context.clone());
        state.invoke_remove_compression_image("remove-owned".into());
        assert!(source.exists());
        assert!(result.exists());
        assert_eq!(state.get_compression_images().row_count(), 0);
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn generated_result_reused_as_input_and_same_path_replacement_are_never_deleted() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 150, 90, 255]));
        let bytes = encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap();
        let result = write_toolbox_owned_output(
            &fixture.authority,
            "shared-tool-result.png",
            &bytes,
        )
        .unwrap();
        remember_toolbox_temporary_output(fixture.persistence.lease(), &result);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        add_conversion_paths_captured(&app, vec![result.clone()], &fixture.persistence);
        assert_eq!(state.get_conversion_images().row_count(), 1);
        set_compression_images(&state, vec![CompressionImageItem {
            id: "shared-result".into(),
            name: "shared-tool-result.png".into(),
            source_path: result.display().to_string().into(),
            size_text: "1 KB".into(),
            image: Image::default(),
            status: "completed".into(),
            result_path: result.display().to_string().into(),
        }]);
        wire_toolbox_callbacks(&app, fixture.context.clone());
        state.invoke_remove_compression_image("shared-result".into());
        assert!(fixture
            .authority
            .read_image_source(&result, 100 * 1024 * 1024)
            .is_ok());
        assert_eq!(
            state
                .get_conversion_images()
                .row_data(0)
                .unwrap()
                .source_path
                .as_str(),
            result.display().to_string(),
        );

        fs::remove_file(&result).unwrap();
        fs::write(&result, b"same path, new file identity").unwrap();
        remember_toolbox_temporary_output(fixture.persistence.lease(), &result);
        set_compression_images(&state, vec![CompressionImageItem {
            id: "replacement".into(),
            name: "shared-tool-result.png".into(),
            source_path: result.display().to_string().into(),
            size_text: "1 KB".into(),
            image: Image::default(),
            status: "completed".into(),
            result_path: result.display().to_string().into(),
        }]);
        state.invoke_clear_compression_images();
        assert_eq!(fs::read(&result).unwrap(), b"same path, new file identity");
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn remove_and_clear_callbacks_preserve_saved_output_asset_and_file_index() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let rgba = image::RgbaImage::from_pixel(5, 4, image::Rgba([30, 160, 90, 255]));
        let bytes = encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap();
        let path = write_toolbox_owned_output(&fixture.authority, "saved-asset.png", &bytes)
            .unwrap();
        let key = ManagedFileKey::new(ManagedUserArea::Output, "saved-asset.png").unwrap();
        let file = fixture.authority.open_existing_regular(&key).unwrap();
        let registration = NamespacedManagedFileRegistration::new(
            &fixture.authority,
            file,
            "image",
            "user",
        )
        .unwrap();
        let index = fixture.authority.delivery_index().unwrap();
        index
            .register_file_for_namespace(&fixture.authority, &registration)
            .unwrap();
        fixture.context.store.borrow_mut().assets.insert(0, AssetData {
            id: "saved-output-asset".into(),
            conversation_id: String::new(),
            title: "saved output".into(),
            category: "other".into(),
            kind: "game".into(),
            time: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            model: String::new(),
            origin: "image_generation".into(),
            width: 5,
            height: 4,
            source_path: path.display().to_string(),
            reference_paths: vec![],
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable: false,
            delivery_downloading: false,
        });
        let app = AppWindow::new().unwrap();
        add_compression_paths_for_store(&app, &fixture.context.store, vec![path.clone()]);
        add_conversion_paths_for_store(&app, &fixture.context.store, vec![path.clone()]);
        assert!(add_crop_paths_for_store(
            &app,
            &fixture.context.store,
            vec![path.clone()],
        ));
        let replacement_root = tempfile::tempdir().unwrap();
        let crop_replacement = replacement_root.path().join("crop-replacement.png");
        fs::write(&crop_replacement, &bytes).unwrap();
        assert!(add_crop_paths_for_store(
            &app,
            &fixture.context.store,
            vec![crop_replacement],
        ));
        wire_toolbox_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        let compression_id = state.get_compression_images().row_data(0).unwrap().id;
        state.invoke_remove_compression_image(compression_id);
        state.invoke_clear_conversion_images();

        assert!(fixture
            .authority
            .read_image_source(&path, 100 * 1024 * 1024)
            .is_ok());
        assert!(fixture
            .context
            .store
            .borrow()
            .assets
            .iter()
            .any(|asset| asset.id == "saved-output-asset"));
        assert!(index
            .find_file_by_path_for_namespace(
                &fixture.authority,
                ManagedUserArea::Output,
                "saved-asset.png",
            )
            .unwrap()
            .is_some());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_crop_callback_waits_for_owned_output_and_ordered_store_ack() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("actual-crop.png");
        let rgba = image::RgbaImage::from_pixel(8, 6, image::Rgba([200, 60, 40, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_crop_source_path(source.display().to_string().into());
        state.set_crop_source_name("actual-crop.png".into());
        state.set_crop_transform_steps("".into());
        state.set_crop_x(0.0);
        state.set_crop_y(0.0);
        state.set_crop_width(1.0);
        state.set_crop_height(1.0);
        wire_toolbox_callbacks(&app, fixture.context.clone());
        state.invoke_save_crop_result();
        video_images_tests::scoped_inputs::pump(|| {
            fixture
                .context
                .store
                .borrow()
                .assets
                .iter()
                .any(|item| item.origin == "image_crop")
                && !app.global::<AppState>().get_crop_processing()
        });
        let result = fixture
            .context
            .store
            .borrow()
            .assets
            .iter()
            .find(|item| item.origin == "image_crop")
            .map(|item| PathBuf::from(&item.source_path))
            .expect("ordered crop asset");
        let durable = fixture
            .writer
            .load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap()
            .expect("real ordered SQLite acknowledgement");
        let result_text = result.display().to_string();
        assert!(durable
            .assets
            .iter()
            .any(|item| item.origin == "image_crop" && item.source_path == result_text));
        assert!(durable
            .notifications
            .iter()
            .any(|item| item.success && item.model == "图片裁剪"));
        assert!(result.starts_with(fixture.authority.lease().namespace.output_dir()));
        assert!(fixture
            .authority
            .read_image_source(&result, 100 * 1024 * 1024)
            .is_ok());
        let leaf = result.file_name().and_then(|value| value.to_str()).unwrap();
        assert!(fixture
            .authority
            .delivery_index()
            .unwrap()
            .find_file_by_path_for_namespace(
                &fixture.authority,
                ManagedUserArea::Output,
                leaf,
            )
            .unwrap()
            .is_some());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_crop_sql_rejection_preserves_indexed_staging_without_success_ack() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        fixture.reject_notification_inserts_for_test();
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("rejected-crop.png");
        let rgba = image::RgbaImage::from_pixel(8, 6, image::Rgba([180, 70, 50, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_crop_source_path(source.display().to_string().into());
        state.set_crop_source_name("rejected-crop.png".into());
        state.set_crop_transform_steps("".into());
        state.set_crop_x(0.0);
        state.set_crop_y(0.0);
        state.set_crop_width(1.0);
        state.set_crop_height(1.0);
        wire_toolbox_callbacks(&app, fixture.context.clone());
        state.invoke_save_crop_result();
        video_images_tests::scoped_inputs::pump(|| {
            state.get_crop_message().contains("未确认")
        });
        let staged = fixture
            .context
            .store
            .borrow()
            .assets
            .iter()
            .find(|item| item.origin == "image_crop")
            .map(|item| PathBuf::from(&item.source_path))
            .expect("failed ordered write retains staged crop metadata");
        assert!(fixture
            .authority
            .read_image_source(&staged, 100 * 1024 * 1024)
            .is_ok());
        let leaf = staged.file_name().and_then(|value| value.to_str()).unwrap();
        assert!(fixture
            .authority
            .delivery_index()
            .unwrap()
            .find_file_by_path_for_namespace(
                &fixture.authority,
                ManagedUserArea::Output,
                leaf,
            )
            .unwrap()
            .is_some());
        assert!(state.get_crop_message().contains("未确认"));
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_local_callback_rejects_retired_captured_namespace() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_compression_target_mb("old-user-sentinel".into());
        wire_toolbox_callbacks(&app, fixture.context.clone());
        drain_toolbox_fixture(&fixture);
        state.invoke_update_compression_target_preview("1024".into());
        assert_eq!(state.get_compression_target_mb().as_str(), "old-user-sentinel");
    }

    #[test]
    fn writer_failure_preserves_staged_toolbox_asset_for_ordered_retry() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        let asset_id = "staged-toolbox-asset".to_string();
        let notification_id = "staged-toolbox-notification".to_string();
        fixture.context.store.borrow_mut().assets.insert(0, AssetData {
            id: asset_id.clone(),
            conversation_id: String::new(),
            title: "staged".into(),
            category: "other".into(),
            kind: "game".into(),
            time: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            model: String::new(),
            origin: "watermark_removal".into(),
            width: 1,
            height: 1,
            source_path: "never-published".into(),
            reference_paths: vec![],
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable: false,
            delivery_downloading: false,
        });
        fixture.context.store.borrow_mut().notifications.insert(0, NotificationData {
            id: notification_id.clone(),
            title: "staged".into(),
            model: String::new(),
            time: String::new(),
            reason: String::new(),
            success: true,
            read: false,
        });
        let (sender, receiver) = mpsc::channel::<std::result::Result<(), &'static str>>();
        sender.send(Err("controlled writer failure")).unwrap();
        poll_toolbox_asset_ack(
            app.as_weak(),
            fixture.context.clone(),
            fixture.persistence.clone(),
            receiver,
            PendingToolboxAssetCommit {
                completion: ToolboxAssetCompletion::Crop,
            },
        );
        video_images_tests::scoped_inputs::pump(|| {
            app.global::<AppState>().get_crop_message().contains("未确认")
        });
        assert!(fixture.context.store.borrow().assets.iter().any(|item| item.id == asset_id));
        assert!(fixture.context.store.borrow().notifications.iter().any(|item| item.id == notification_id));
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn captured_local_worker_drop_is_nonblocking_and_reaps_after_finish() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        use std::sync::atomic::{AtomicBool, Ordering};
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let (_sender, receiver) = mpsc::channel::<()>();
        let worker = spawn_toolbox_worker(fixture.persistence.lease().clone(), move |_| {
            worker_finished.store(true, Ordering::SeqCst)
        })
        .unwrap();
        let worker_id = worker.id;
        drop(CapturedToolboxWork::new(receiver, worker));
        video_images_tests::scoped_inputs::pump(|| {
            finished.load(Ordering::SeqCst)
                && finish_toolbox_worker_if_ready(worker_id).unwrap_or(false)
        });
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn local_terminal_message_waits_for_worker_exit_before_ui_completion() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_compression_processing(true);
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _release_on_panic = ReleaseWorkerOnDrop::new(&release);
        let worker_release = Arc::clone(&release);
        let (sender, receiver) = mpsc::channel();
        let worker = spawn_toolbox_worker(fixture.persistence.lease().clone(), move |_| {
            sender
                .send(CompressionOutcome::Finished {
                    succeeded: 1,
                    failed: 0,
                })
                .unwrap();
            while !worker_release.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        })
        .unwrap();
        poll_local_compression(
            app.as_weak(),
            fixture.context.store.clone(),
            fixture.persistence.clone(),
            Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
        );
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(80));
        slint::platform::update_timers_and_animations();
        assert!(state.get_compression_processing());
        release.store(true, Ordering::Release);
        video_images_tests::scoped_inputs::pump(|| !state.get_compression_processing());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn remote_terminal_message_waits_for_worker_exit_and_original_binding() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_watermark_processing(true);
        state.set_watermark_message("still-running".into());
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _release_on_panic = ReleaseWorkerOnDrop::new(&release);
        let worker_release = Arc::clone(&release);
        let (sender, receiver) = mpsc::channel();
        let worker = spawn_toolbox_worker(fixture.persistence.lease().clone(), move |_| {
            sender
                .send(ToolboxRemoteOutcome::Failure {
                    reason: "joined-terminal".to_string(),
                })
                .unwrap();
            while !worker_release.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        })
        .unwrap();
        let session_scope = SessionScope {
            owner_user_id: fixture.persistence.lease().namespace.user_public_id().to_string(),
            auth_epoch: fixture.persistence.lease().auth_epoch,
        };
        poll_watermark_outcomes(
            app.as_weak(),
            fixture.context.clone(),
            session_scope,
            fixture.persistence.clone(),
            Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
        );
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        slint::platform::update_timers_and_animations();
        assert_eq!(state.get_watermark_message().as_str(), "still-running");
        release.store(true, Ordering::Release);
        video_images_tests::scoped_inputs::pump(|| {
            state.get_watermark_message().as_str() == "joined-terminal"
        });
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn remote_terminal_after_binding_change_or_426_never_overwrites_visible_state() {
        i_slint_backend_testing::init_no_event_loop();
        for trip in [false, true] {
            let fixture = video_images_tests::scoped_inputs::Fixture::new();
            let replacement = video_images_tests::scoped_inputs::Fixture::new();
            let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
            let _replacement_workers = ToolboxFixtureWorkerDrain::for_fixture(&replacement);
            let app = AppWindow::new().unwrap();
            let state = app.global::<AppState>();
            state.set_colorize_processing(true);
            state.set_colorize_message("new-binding-sentinel".into());
            let (sender, receiver) = mpsc::channel();
            let worker = spawn_toolbox_worker(fixture.persistence.lease().clone(), move |_| {
                sender
                    .send(ToolboxRemoteOutcome::Failure {
                        reason: "stale-terminal".to_string(),
                    })
                    .unwrap();
            })
            .unwrap();
            let session_scope = SessionScope {
                owner_user_id: fixture
                    .persistence
                    .lease()
                    .namespace
                    .user_public_id()
                    .to_string(),
                auth_epoch: fixture.persistence.lease().auth_epoch,
            };
            poll_image_colorization_outcomes(
                app.as_weak(),
                fixture.context.clone(),
                session_scope,
                fixture.persistence.clone(),
                Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
            );
            if trip {
                fixture
                    .persistence
                    .upgrade_latch()
                    .trip(RequiredUpgrade { minimum_version: None });
            } else {
                fixture.context.store.borrow_mut().private_persistence =
                    Some(replacement.persistence.clone());
            }
            video_images_tests::scoped_inputs::pump(|| {
                drain_toolbox_workers_for_lease_for_test(fixture.persistence.lease()).is_ok()
            });
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
            slint::platform::update_timers_and_animations();
            assert_eq!(state.get_colorize_message().as_str(), "new-binding-sentinel");
            assert!(state.get_colorize_processing());
            drain_toolbox_workers_for_lease_for_test(fixture.persistence.lease()).unwrap();
            if !trip {
                drain_toolbox_fixture(&fixture);
            }
            drain_toolbox_fixture(&replacement);
        }
    }

    #[test]
    fn captured_external_import_result_cannot_replace_a_newer_source() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("late-import.png");
        let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([20, 150, 80, 255]));
        fs::write(
            &source,
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap(),
        )
        .unwrap();
        let imported = persist_reference_image_for_namespace(
            &fixture.authority,
            &decode_reference_bytes(&fs::read(&source).unwrap()).unwrap(),
        )
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_watermark_source_path("source-before-url-import".into());
        let (sender, receiver) = mpsc::channel();
        let worker = spawn_toolbox_worker(fixture.persistence.lease().clone(), move |_| {
            sender.send(Ok(imported)).unwrap();
        })
        .unwrap();
        let request_id = begin_external_toolbox_import(
            fixture.persistence.lease(),
            CapturedExternalToolboxImport::Watermark,
        );
        poll_captured_external_toolbox_import(
            app.as_weak(),
            fixture.context.store.clone(),
            fixture.persistence.clone(),
            CapturedExternalToolboxImport::Watermark,
            request_id,
            "source-before-url-import".to_string(),
            Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
        );
        state.set_watermark_source_path("newer-selected-source".into());
        video_images_tests::scoped_inputs::pump(|| {
            drain_toolbox_workers_for_lease_for_test(fixture.persistence.lease()).is_ok()
        });
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(120));
        slint::platform::update_timers_and_animations();
        assert_eq!(state.get_watermark_source_path().as_str(), "newer-selected-source");
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn two_captured_url_imports_released_oldest_first_publish_only_the_latest_request() {
        use std::io::{Read as _, Write as _};
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let delayed_server = |color: [u8; 4]| {
            let image = image::RgbaImage::from_pixel(4, 4, image::Rgba(color));
            let bytes = encode_png_rgba(&image, image.width(), image.height()).unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/captured.png", listener.local_addr().unwrap());
            let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let server_release = Arc::clone(&release);
            let handle = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request).unwrap();
                while !server_release.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len(),
                )
                .unwrap();
                stream.write_all(&bytes).unwrap();
            });
            (url, release, handle)
        };
        let (first_url, first_release, first_server) = delayed_server([220, 20, 30, 255]);
        let (second_url, second_release, second_server) = delayed_server([20, 60, 230, 255]);
        let _first_release_on_panic = ReleaseWorkerOnDrop::new(&first_release);
        let _second_release_on_panic = ReleaseWorkerOnDrop::new(&second_release);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_watermark_source_path("same-original-source".into());
        start_captured_external_toolbox_import(
            &app,
            fixture.context.store.clone(),
            fixture.persistence.clone(),
            first_url,
            CapturedExternalToolboxImport::Watermark,
        );
        start_captured_external_toolbox_import(
            &app,
            fixture.context.store.clone(),
            fixture.persistence.clone(),
            second_url,
            CapturedExternalToolboxImport::Watermark,
        );

        first_release.store(true, Ordering::Release);
        first_server.join().unwrap();
        video_images_tests::scoped_inputs::pump(|| {
            toolbox_workers()
                .lock()
                .unwrap()
                .workers
                .iter()
                .filter(|worker| &worker.lease == fixture.persistence.lease())
                .count()
                == 1
        });
        assert_eq!(state.get_watermark_source_path().as_str(), "same-original-source");

        second_release.store(true, Ordering::Release);
        second_server.join().unwrap();
        video_images_tests::scoped_inputs::pump(|| {
            state.get_watermark_source_path().as_str() != "same-original-source"
        });
        let published = PathBuf::from(state.get_watermark_source_path().to_string());
        let bytes = fixture
            .authority
            .read_image_source(&published, 100 * 1024 * 1024)
            .unwrap();
        let pixel = decode_reference_bytes(&bytes).unwrap().to_rgba8().get_pixel(0, 0).0;
        assert_eq!(pixel, [20, 60, 230, 255]);
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_captured_url_import_publishes_only_a_namespaced_indexed_reference() {
        use std::io::{Read as _, Write as _};
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 140, 100, 255]));
        let bytes = encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/captured.png", listener.local_addr().unwrap());
        let transport = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let app = AppWindow::new().unwrap();
        start_captured_external_toolbox_import(
            &app,
            fixture.context.store.clone(),
            fixture.persistence.clone(),
            url,
            CapturedExternalToolboxImport::Watermark,
        );
        video_images_tests::scoped_inputs::pump(|| {
            !app.global::<AppState>().get_watermark_source_path().is_empty()
        });
        transport.join().unwrap();
        let path = PathBuf::from(
            app.global::<AppState>()
                .get_watermark_source_path()
                .to_string(),
        );
        assert!(path.starts_with(
            fixture
                .authority
                .lease()
                .namespace
                .path(ManagedUserArea::ReferencesLibrary)
        ));
        let leaf = path.file_name().and_then(|value| value.to_str()).unwrap();
        assert!(fixture
            .authority
            .delivery_index()
            .unwrap()
            .find_file_by_path_for_namespace(
                &fixture.authority,
                ManagedUserArea::ReferencesLibrary,
                leaf,
            )
            .unwrap()
            .is_some());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn actual_captured_url_426_trips_without_transfer_activity_self_deadlock() {
        use std::io::{Read as _, Write as _};
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/upgrade", listener.local_addr().unwrap());
        let transport = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            let body = r#"{"request_id":"upgrade","data":null,"error":{"code":"client_upgrade_required","message":"upgrade","details":{"minimum_version":"99.0.0"}},"meta":null}"#;
            write!(
                stream,
                "HTTP/1.1 426 Upgrade Required\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let app = AppWindow::new().unwrap();
        start_captured_external_toolbox_import(
            &app,
            fixture.context.store.clone(),
            fixture.persistence.clone(),
            url,
            CapturedExternalToolboxImport::Colorization,
        );
        video_images_tests::scoped_inputs::pump(|| {
            fixture.persistence.upgrade_latch().is_tripped()
        });
        transport.join().unwrap();
        assert!(app.global::<AppState>().get_colorize_source_path().is_empty());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn namespace_retirement_cancels_registered_toolbox_worker_without_ui_join() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let ticket = spawn_toolbox_worker(fixture.persistence.lease().clone(), move |cancel| {
            while !cancel.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_finished.store(true, Ordering::Release);
        })
        .unwrap();
        let worker_id = ticket.id;
        cancel_toolbox_workers_for_lease(fixture.persistence.lease());
        video_images_tests::scoped_inputs::pump(|| {
            finished.load(Ordering::Acquire)
                && finish_toolbox_worker_if_ready(worker_id).unwrap_or(false)
        });
        assert!(finished.load(Ordering::Acquire));
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn captured_local_workers_read_through_authority_and_publish_owned_outputs() {
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("source.png");
        let rgba = image::RgbaImage::from_pixel(6, 4, image::Rgba([30, 100, 220, 255]));
        fs::write(&source, encode_png_rgba(&rgba, rgba.width(), rgba.height()).unwrap()).unwrap();

        let (compression_sender, compression_receiver) = mpsc::channel();
        run_captured_local_compression_worker(
            fixture.authority.clone(),
            vec![CompressionInput { id: "compression".into(), source_path: source.display().to_string() }],
            ImageCompressionMode::Quality(90),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            compression_sender,
        );
        let compressed = compression_receiver.into_iter().find_map(|outcome| match outcome {
            CompressionOutcome::Completed { result_path, .. } => Some(PathBuf::from(result_path)),
            _ => None,
        }).expect("captured compression result");
        assert!(compressed.starts_with(fixture.authority.lease().namespace.output_dir()));
        assert!(fixture.authority.read_image_source(&compressed, 100 * 1024 * 1024).is_ok());

        let (conversion_sender, conversion_receiver) = mpsc::channel();
        run_captured_local_conversion_worker(
            fixture.authority.clone(),
            vec![ConversionInput { id: "conversion".into(), source_path: source.display().to_string() }],
            "webp".into(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            conversion_sender,
        );
        let converted = conversion_receiver.into_iter().find_map(|outcome| match outcome {
            ConversionOutcome::Completed { result_path, .. } => Some(PathBuf::from(result_path)),
            _ => None,
        }).expect("captured conversion result");
        assert!(converted.starts_with(fixture.authority.lease().namespace.output_dir()));
        let converted_bytes = fixture.authority.read_image_source(&converted, 100 * 1024 * 1024).unwrap();
        assert_eq!(image::guess_format(&converted_bytes).unwrap(), image::ImageFormat::WebP);
        assert!(!unlink_registered_toolbox_temporary_output(
            &fixture.persistence,
            &compressed,
        ));
        assert!(!unlink_registered_toolbox_temporary_output(
            &fixture.persistence,
            &converted,
        ));
        assert!(fixture.authority.read_image_source(&compressed, 100 * 1024 * 1024).is_ok());
        assert!(fixture.authority.read_image_source(&converted, 100 * 1024 * 1024).is_ok());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn late_local_worker_completion_after_426_never_mutates_visible_state() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        set_compression_images(&state, vec![CompressionImageItem {
            id: "late-item".into(),
            name: "source.png".into(),
            source_path: "/never-read/source.png".into(),
            size_text: "1 KB".into(),
            image: Image::default(),
            status: "pending".into(),
            result_path: "".into(),
        }]);
        let (sender, receiver) = mpsc::channel();
        let worker = spawn_toolbox_worker(fixture.persistence.lease().clone(), move |_| {
            sender.send(CompressionOutcome::Completed {
                id: "late-item".into(),
                result_path: "/must-not-appear.png".into(),
                size_text: "2 KB".into(),
            }).unwrap();
        }).unwrap();
        poll_local_compression(
            app.as_weak(),
            fixture.context.store.clone(),
            fixture.persistence.clone(),
            Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
        );
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: None });
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        slint::platform::update_timers_and_animations();
        let item = state.get_compression_images().row_data(0).unwrap();
        assert_eq!(item.status.as_str(), "pending");
        assert!(item.result_path.is_empty());
        drain_toolbox_fixture(&fixture);
    }

    #[test]
    fn late_local_worker_completion_after_namespace_retirement_never_mutates_new_ui() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_images_tests::scoped_inputs::Fixture::new();
        let _workers = ToolboxFixtureWorkerDrain::for_fixture(&fixture);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        set_conversion_images(&state, vec![CompressionImageItem {
            id: "old-user-item".into(), name: "source.png".into(),
            source_path: "/never-read/source.png".into(), size_text: "1 KB".into(),
            image: Image::default(), status: "pending".into(), result_path: "".into(),
        }]);
        let (sender, receiver) = mpsc::channel();
        let worker = spawn_toolbox_worker(fixture.persistence.lease().clone(), move |_| {
            sender.send(ConversionOutcome::Completed {
                id: "old-user-item".into(), result_path: "/must-not-appear.webp".into(), size_text: "2 KB".into(),
            }).unwrap();
        }).unwrap();
        poll_local_conversion(
            app.as_weak(), fixture.context.store.clone(), fixture.persistence.clone(),
            Rc::new(RefCell::new(Some(CapturedToolboxWork::new(receiver, worker)))),
        );
        drain_toolbox_fixture(&fixture);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        slint::platform::update_timers_and_animations();
        let item = state.get_conversion_images().row_data(0).unwrap();
        assert_eq!(item.status.as_str(), "pending");
        assert!(item.result_path.is_empty());
    }

    #[test]
    fn local_conversion_worker_writes_a_detectable_result() {
        let test_root =
            std::env::temp_dir().join(format!("artforge-conversion-test-{}", Uuid::new_v4()));
        let source = test_root.join("source.jpg");
        let output_dir = test_root.join("results");
        fs::create_dir_all(&test_root).expect("create conversion test directory");
        let rgba = image::RgbaImage::from_pixel(6, 4, image::Rgba([30, 100, 220, 255]));
        let source_bytes =
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode source png");
        fs::write(&source, source_bytes).expect("write png with jpeg extension");

        let (sender, receiver) = mpsc::channel();
        run_local_conversion_worker(
            vec![ConversionInput {
                id: "item-1".to_string(),
                source_path: source.display().to_string(),
            }],
            "webp".to_string(),
            output_dir,
            sender,
        );
        let outcomes = receiver.into_iter().collect::<Vec<_>>();
        let result_path = outcomes
            .iter()
            .find_map(|outcome| match outcome {
                ConversionOutcome::Completed { result_path, .. } => {
                    Some(PathBuf::from(result_path))
                }
                _ => None,
            })
            .expect("completed conversion outcome");

        assert!(matches!(
            image::guess_format(&fs::read(&result_path).expect("read conversion result")),
            Ok(image::ImageFormat::WebP)
        ));
        assert!(outcomes.iter().any(|outcome| matches!(
            outcome,
            ConversionOutcome::Finished {
                succeeded: 1,
                failed: 0
            }
        )));
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn save_destination_always_uses_the_converted_extension() {
        let directory = Path::new("chosen-folder");
        assert_eq!(
            normalize_conversion_destination(&directory.join("image"), "png"),
            directory.join("image.png")
        );
        assert_eq!(
            normalize_conversion_destination(&directory.join("image.jpg"), "webp"),
            directory.join("image.webp")
        );
        assert_eq!(
            normalize_conversion_destination(&directory.join("image.WEBP"), "webp"),
            directory.join("image.WEBP")
        );
    }
}

#[cfg(test)]
mod billing_capture_tests {
    use super::*;
    #[test]
    fn billing_capture_watermark_worker_keeps_persisted_payer() {
        backend_generation::billing_capture_test_support::assert_generation_worker(
            "watermark_removal",
            |backend, authority, billing, scope, record, sender| {
                run_watermark_worker(
                    backend,
                    authority,
                    Some(billing),
                    scope,
                    record,
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    sender,
                )
            },
        );
    }
    #[test]
    fn billing_capture_colorization_worker_keeps_persisted_payer() {
        backend_generation::billing_capture_test_support::assert_generation_worker(
            "image_colorization",
            |backend, authority, billing, scope, record, sender| {
                run_image_colorization_worker(
                    backend,
                    authority,
                    Some(billing),
                    scope,
                    record,
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    sender,
                )
            },
        );
    }

    #[test]
    fn actual_saved_toolbox_detail_mismatch_preserves_the_original_recovery_row() {
        use backend_generation::billing_capture_test_support as support;
        for body in [
            r#"{"request_id":"mismatch","data":{"id":"different-task","billing_account_group_id":"22222222-2222-4222-8222-222222222222","status":"processing","progress_percent":10,"success_count":0,"failure_count":0,"failure":null,"prompt":null,"result_prompt":null,"items":[]},"error":null,"meta":null}"#,
            r#"{"request_id":"mismatch","data":{"id":"saved-server-task","billing_account_group_id":"33333333-3333-4333-8333-333333333333","status":"processing","progress_percent":10,"success_count":0,"failure_count":0,"failure":null,"prompt":null,"result_prompt":null,"items":[]},"error":null,"meta":null}"#,
        ] {
            let (listener, url) = support::listener();
            let fixture = support::fixture(&url);
            let mut record = support::generation_record(&fixture.scope, "image_watermark_removal");
            record.server_task_id = "saved-server-task".to_string();
            upsert_pending_generation_for_namespace(
                &fixture.authority,
                &fixture.scope,
                record.clone(),
            )
            .unwrap();
            let (release, transport) = support::capture_response(
                listener,
                fixture.authority.clone(),
                "pending-generations.json",
                "200 OK",
                body,
            );
            let (sender, receiver) = mpsc::channel();
            let backend = fixture.backend.clone();
            let authority = fixture.authority.clone();
            let billing = fixture.scope.clone();
            let session = fixture.scope.request.session.clone();
            let worker = std::thread::spawn(move || {
                run_watermark_worker(
                    backend,
                    authority,
                    Some(billing),
                    session,
                    record,
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    sender,
                )
            });
            release.send(()).unwrap();
            let captured = transport.join().unwrap();
            assert!(captured.request.starts_with("GET /v1/generation/tasks/saved-server-task "));
            worker.join().unwrap();
            assert!(matches!(
                receiver.try_recv(),
                Ok(ToolboxRemoteOutcome::Failure { .. })
            ));
            let retained = load_pending_generations_for_namespace(&fixture.authority).unwrap();
            assert_eq!(retained.len(), 1);
            assert_eq!(retained[0].server_task_id, "saved-server-task");
            assert_eq!(
                retained[0].billing_account_group_id,
                support::PAYER
            );
        }
    }

    #[test]
    fn actual_toolbox_worker_rejects_malformed_retained_reference_proof_before_upload() {
        use backend_generation::billing_capture_test_support as support;
        let (listener, url) = support::listener();
        let fixture = support::fixture(&url);
        let mut record = support::generation_record(&fixture.scope, "image_colorization");
        record.server_task_id.clear();
        record.uploaded_file_ids.clear();
        record.reference_paths = vec!["/must-not-read-without-proof.png".to_string()];
        record.reference_sha256.clear();
        record.reference_size_bytes = vec![1];
        upsert_pending_generation_for_namespace(
            &fixture.authority,
            &fixture.scope,
            record.clone(),
        )
        .unwrap();
        let (sender, receiver) = mpsc::channel();
        run_image_colorization_worker(
            fixture.backend.clone(),
            fixture.authority.clone(),
            Some(fixture.scope.clone()),
            fixture.scope.request.session.clone(),
            record,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sender,
        );
        support::assert_no_request(&listener);
        assert!(matches!(
            receiver.try_recv(),
            Ok(ToolboxRemoteOutcome::Failure { .. })
        ));
        assert_eq!(
            load_pending_generations_for_namespace(&fixture.authority)
                .unwrap()
                .len(),
            1
        );
    }
}
