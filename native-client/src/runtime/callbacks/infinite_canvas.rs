use super::*;
use std::hash::{Hash, Hasher};

pub(super) const MAX_CANVAS_NODES: usize = 200;
pub(super) const MAX_CANVAS_LINKS: usize = 400;
const MAX_CANVAS_SPLIT_AXIS: u32 = 64;
const MAX_CANVAS_WORKERS: usize = 64;

struct PreparedCanvasImage {
    path: String,
    width: f32,
    height: f32,
}

struct CanvasActionPermit {
    persistence: PrivatePersistence,
    _activity: UserActivityPermit,
    _effect: api::OrdinaryBlockingEffectPermit,
}

#[derive(Clone)]
struct CanvasActionCapture {
    persistence: PrivatePersistence,
    workspace_id: String,
}

impl CanvasActionCapture {
    fn capture(store: &Rc<RefCell<Store>>) -> Option<Self> {
        let store = store.borrow();
        let persistence = store.private_persistence.clone()?;
        let capture = Self {
            persistence,
            workspace_id: normalize_canvas_workspace_id(&store.active_canvas_workspace_id),
        };
        capture.is_current(store.private_persistence.as_ref(), &store).then_some(capture)
    }

    fn is_current(&self, binding: Option<&PrivatePersistence>, store: &Store) -> bool {
        self.persistence.is_current()
            && binding.is_some_and(|current| current.same_binding(&self.persistence))
            && normalize_canvas_workspace_id(&store.active_canvas_workspace_id) == self.workspace_id
    }

    fn binding_matches_without_latch(&self, store: &Store) -> bool {
        store.private_persistence.as_ref()
            .is_some_and(|current| current.same_binding_metadata(&self.persistence))
            && normalize_canvas_workspace_id(&store.active_canvas_workspace_id) == self.workspace_id
    }

    fn apply<R>(
        &self,
        store: &Rc<RefCell<Store>>,
        apply: impl FnOnce() -> R,
    ) -> Option<R> {
        let activity = self.persistence.begin_activity().ok()?;
        let result = self.persistence.upgrade_latch().apply_if_open(|| {
            let current = self.binding_matches_without_latch(&store.borrow());
            current.then(apply)
        });
        drop(activity);
        result.ok().flatten()
    }

    fn begin_effect(&self, store: &Rc<RefCell<Store>>) -> Option<CanvasActionPermit> {
        let current = {
            let store = store.borrow();
            self.is_current(store.private_persistence.as_ref(), &store)
        };
        if !current { return None; }
        let (_activity, _effect) = self.persistence.begin_effect().ok()?;
        let current = {
            let store = store.borrow();
            self.is_current(store.private_persistence.as_ref(), &store)
        };
        if !current { return None; }
        Some(CanvasActionPermit { persistence: self.persistence.clone(), _activity, _effect })
    }
}

fn persist_canvas_managed_image(
    authority: &NamespaceStorageAuthority,
    area: ManagedUserArea,
    prefix: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    let _mutation = authority.begin_ordinary_mutation()?;
    let key = ManagedFileKey::new(
        area,
        &format!("{prefix}-{}.{}", Uuid::new_v4(), image_extension(bytes)),
    )?;
    let mut file = authority.create_temporary_regular_for(&key)?;
    authority.write_new_regular_from(&mut file, &mut std::io::Cursor::new(bytes))?;
    authority.sync_regular(&mut file)?;
    authority.publish_regular(&mut file, NamespaceManagedPublication::Absent(&key))?;
    let registration = NamespacedManagedFileRegistration::new(authority, file, "canvas", "user")?;
    authority.delivery_index()?.register_file_for_namespace(authority, &registration)?;
    Ok(authority.lease().namespace.path(key.area()).join(key.relative_name().as_str()))
}

#[derive(Clone, Debug)]
struct CanvasSplitSource {
    id: String,
    image_path: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Debug)]
struct CanvasSplitTile {
    path: String,
    row: u32,
    column: u32,
    width: u32,
    height: u32,
}

type CanvasSplitOutcome = std::result::Result<Vec<CanvasSplitTile>, String>;

#[derive(Clone, Debug)]
struct CanvasExtractedElement {
    path: String,
    width: u32,
    height: u32,
}

type CanvasExtractionOutcome = std::result::Result<Vec<CanvasExtractedElement>, String>;

struct CanvasWorkerTicket {
    id: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

struct RegisteredCanvasWorker {
    id: u64,
    lease: NamespaceLease,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

#[derive(Default)]
struct CanvasWorkerRegistry {
    closing: bool,
    failed: bool,
    workers: Vec<RegisteredCanvasWorker>,
}

fn canvas_workers() -> &'static Mutex<CanvasWorkerRegistry> {
    static WORKERS: std::sync::OnceLock<Mutex<CanvasWorkerRegistry>> =
        std::sync::OnceLock::new();
    WORKERS.get_or_init(|| Mutex::new(CanvasWorkerRegistry::default()))
}

fn spawn_canvas_worker(
    persistence: PrivatePersistence,
    work: impl FnOnce(Arc<std::sync::atomic::AtomicBool>, UserActivityPermit) + Send + 'static,
) -> Result<CanvasWorkerTicket> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let activity = persistence.begin_activity()?;
    let mut registry = canvas_workers().lock().map_err(|_| anyhow!("canvas worker registry unavailable"))?;
    anyhow::ensure!(!registry.closing, "canvas worker admission closed");
    anyhow::ensure!(!registry.failed, "a prior canvas worker failed");
    anyhow::ensure!(registry.workers.len() < MAX_CANVAS_WORKERS, "canvas worker limit reached");
    let id = NEXT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1))
        .map_err(|_| anyhow!("canvas worker identifier exhausted"))?;
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let lease = persistence.lease().clone();
    let handle = std::thread::Builder::new().name(format!("canvas-{id}"))
        .spawn(move || work(worker_cancel, activity))?;
    registry.workers.push(RegisteredCanvasWorker { id, lease, cancel: cancel.clone(), handle });
    Ok(CanvasWorkerTicket { id, cancel })
}

fn finish_canvas_worker_if_ready(id: u64) -> Result<bool> {
    let worker = {
        let mut registry = canvas_workers().lock().map_err(|_| anyhow!("canvas worker registry unavailable"))?;
        let Some(index) = registry.workers.iter().position(|worker| worker.id == id) else {
            anyhow::ensure!(!registry.failed, "a canvas worker failed");
            return Ok(true);
        };
        if !registry.workers[index].handle.is_finished() { return Ok(false); }
        registry.workers.swap_remove(index)
    };
    if worker.handle.join().is_err() {
        canvas_workers().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).failed = true;
        anyhow::bail!("canvas worker panicked");
    }
    Ok(true)
}

fn schedule_canvas_worker_reap(id: u64) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        match finish_canvas_worker_if_ready(id) {
            Ok(false) => schedule_canvas_worker_reap(id),
            Ok(true) | Err(_) => {}
        }
    });
}

pub(super) fn cancel_canvas_workers_for_lease(lease: &NamespaceLease) {
    let registry = canvas_workers().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for worker in registry.workers.iter().filter(|worker| &worker.lease == lease) {
        worker.cancel.store(true, Ordering::Release);
    }
}

pub(super) fn drain_canvas_workers_for_shutdown() -> Result<()> {
    let workers = {
        let mut registry = canvas_workers().lock().map_err(|_| anyhow!("canvas worker registry unavailable"))?;
        registry.closing = true;
        std::mem::take(&mut registry.workers)
    };
    for worker in &workers { worker.cancel.store(true, Ordering::Release); }
    let failed = workers.into_iter().fold(false, |failed, worker| worker.handle.join().is_err() || failed);
    let mut registry = canvas_workers().lock().map_err(|_| anyhow!("canvas worker registry unavailable"))?;
    registry.failed |= failed;
    anyhow::ensure!(!registry.failed, "a canvas worker failed");
    Ok(())
}

#[cfg(test)]
pub(super) fn drain_canvas_workers_for_lease_for_test(lease: &NamespaceLease) -> Result<()> {
    cancel_canvas_workers_for_lease(lease);
    let workers = {
        let mut registry = canvas_workers().lock().map_err(|_| anyhow!("canvas worker registry unavailable"))?;
        let mut selected = Vec::new();
        let mut index = registry.workers.len();
        while index > 0 {
            index -= 1;
            if &registry.workers[index].lease == lease { selected.push(registry.workers.swap_remove(index)); }
        }
        selected
    };
    let failed = workers.into_iter().fold(false, |failed, worker| worker.handle.join().is_err() || failed);
    let mut registry = canvas_workers().lock().map_err(|_| anyhow!("canvas worker registry unavailable"))?;
    registry.failed |= failed;
    anyhow::ensure!(!registry.failed, "a canvas worker failed");
    Ok(())
}

fn prepare_canvas_store_ack_worker(
    persistence: PrivatePersistence,
) -> Result<(
    CanvasWorkerTicket,
    mpsc::Sender<mpsc::Receiver<WriteResult>>,
    mpsc::Receiver<bool>,
)> {
    let (command_sender, command_receiver) = mpsc::channel::<mpsc::Receiver<WriteResult>>();
    let (acknowledgment_sender, acknowledgment_receiver) = mpsc::channel();
    let ticket = spawn_canvas_worker(persistence, move |cancel, activity| {
        let Ok(receiver) = command_receiver.recv() else { return; };
        let acknowledged = matches!(receiver.recv(), Ok(Ok(())))
            && !cancel.load(Ordering::Acquire)
            && !activity.is_quiescing();
        let _ = acknowledgment_sender.send(acknowledged);
    })?;
    Ok((ticket, command_sender, acknowledgment_receiver))
}

struct PreparedCanvasEdit {
    write: PreparedPrivateStoreWrite,
    ticket: CanvasWorkerTicket,
    command: mpsc::Sender<mpsc::Receiver<WriteResult>>,
    acknowledgment: mpsc::Receiver<bool>,
}

struct AppliedCanvasEdit<R> {
    value: R,
    persistence: PrivatePersistence,
    effects: CanvasPreviewEffects,
    ticket: CanvasWorkerTicket,
    acknowledgment: mpsc::Receiver<bool>,
}

fn prepare_canvas_edit(capture: &CanvasActionCapture) -> Result<PreparedCanvasEdit> {
    let write = capture.persistence.prepare_ordered_save()?;
    let (ticket, command, acknowledgment) =
        prepare_canvas_store_ack_worker(capture.persistence.clone())?;
    Ok(PreparedCanvasEdit { write, ticket, command, acknowledgment })
}

fn apply_canvas_edit<R>(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    capture: &CanvasActionCapture,
    edit: impl FnOnce(&mut Store) -> Option<R>,
) -> Option<AppliedCanvasEdit<R>> {
    apply_canvas_edit_checked(app, store, capture, || true, edit)
}

fn apply_canvas_edit_checked<R>(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    capture: &CanvasActionCapture,
    target_is_current: impl FnOnce() -> bool,
    edit: impl FnOnce(&mut Store) -> Option<R>,
) -> Option<AppliedCanvasEdit<R>> {
    let prepared = match prepare_canvas_edit(capture) {
        Ok(prepared) => prepared,
        Err(error) => {
            // Preparation has not staged any Store mutation. Disclose only to
            // the original admitted target; never expose a storage error body.
            let _ = capture.apply(store, || {
                if target_is_current() {
                    let state = app.global::<AppState>();
                    state.set_generation_status(if state.get_language().as_str() == "en" {
                        "Canvas save was not confirmed"
                    } else {
                        "画布保存未确认"
                    }.into());
                }
            });
            drop(error);
            return None;
        }
    };
    let PreparedCanvasEdit { write, ticket, command, acknowledgment } = prepared;
    let mut write = Some(write);
    let mut command = Some(command);
    let applied = capture.apply(store, || {
        if !target_is_current() { return None; }
        let mut store = store.borrow_mut();
        let value = edit(&mut store)?;
        let data = local_store_data(app, &store);
        let projection = prepare_canvas_projection(app, &store);
        let receiver = write.take().expect("prepared canvas write").enqueue(data);
        let effects = projection.publish_metadata(app);
        Some((value, receiver, effects))
    }).flatten();

    let Some((value, receiver, effects)) = applied else {
        drop(command);
        schedule_canvas_worker_reap(ticket.id);
        return None;
    };
    let receiver = match receiver {
        Ok(receiver) => receiver,
        Err(error) => {
            // Guard-owning enqueue errors must leave the latch before they are dropped.
            drop(error);
            drop(command);
            schedule_canvas_worker_reap(ticket.id);
            let _ = capture.apply(store, || {
                let state = app.global::<AppState>();
                state.set_generation_status(if state.get_language().as_str() == "en" {
                    "Canvas save was not confirmed"
                } else {
                    "画布保存未确认"
                }.into());
            });
            return None;
        }
    };
    if command.take().expect("prepared canvas ack command").send(receiver).is_err() {
        schedule_canvas_worker_reap(ticket.id);
        return None;
    }
    Some(AppliedCanvasEdit {
        value,
        persistence: capture.persistence.clone(),
        effects,
        ticket,
        acknowledgment,
    })
}

fn start_canvas_edit_preview(
    app: &AppWindow,
    persistence: PrivatePersistence,
    effects: CanvasPreviewEffects,
) {
    let weak = app.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(app) = weak.upgrade() {
            start_canvas_preview_effects(&app, persistence, effects);
        }
    });
}

fn finish_canvas_edit<R>(app: &AppWindow, edit: AppliedCanvasEdit<R>) -> R {
    let AppliedCanvasEdit { value, persistence, effects, ticket, acknowledgment } = edit;
    start_canvas_edit_preview(app, persistence, effects);
    drop(acknowledgment);
    schedule_canvas_worker_reap(ticket.id);
    value
}

fn finish_canvas_edit_with_status<R>(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    capture: CanvasActionCapture,
    edit: AppliedCanvasEdit<R>,
    completion: CanvasSaveCompletion,
) -> R {
    let AppliedCanvasEdit { value, persistence, effects, ticket, acknowledgment } = edit;
    start_canvas_edit_preview(app, persistence, effects);
    poll_canvas_save_status(
        app.as_weak(), store, capture, acknowledgment,
        Rc::new(RefCell::new(Some(ticket))), completion,
    );
    value
}

fn apply_canvas_ui<R>(
    store: &Rc<RefCell<Store>>,
    apply: impl FnOnce(&Store) -> R,
) -> Option<R> {
    let capture = CanvasActionCapture::capture(store)?;
    capture.apply(store, || apply(&store.borrow()))
}

enum CanvasSystemClipboard {
    Image {
        fingerprint: u64,
        width: u32,
        height: u32,
        bytes: Vec<u8>,
    },
    Text {
        fingerprint: u64,
        text: String,
    },
}

#[cfg(test)]
thread_local! {
    static CANVAS_PICKER_FIXTURE: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static CANVAS_EXPORT_FIXTURE: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static CANVAS_CLIPBOARD_FIXTURE: RefCell<Vec<CanvasSystemClipboard>> = const { RefCell::new(Vec::new()) };
    static CANVAS_DATA_ROOT_FIXTURE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static CANVAS_PICKER_RETURN_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn canvas_test_worker_exit_barrier() -> &'static Mutex<Option<Arc<std::sync::atomic::AtomicBool>>> {
    static BARRIER: std::sync::OnceLock<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>> =
        std::sync::OnceLock::new();
    BARRIER.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn canvas_test_worker_exit_reached() -> &'static std::sync::atomic::AtomicBool {
    static REACHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &REACHED
}

#[cfg(test)]
fn canvas_test_worker_panic_after_send() -> &'static std::sync::atomic::AtomicBool {
    static PANIC_AFTER_SEND: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    &PANIC_AFTER_SEND
}

fn choose_canvas_image_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        let selected = CANVAS_PICKER_FIXTURE.with(|fixture| fixture.borrow_mut().pop());
        if selected.is_some() {
            CANVAS_PICKER_RETURN_HOOK.with(|hook| {
                if let Some(hook) = hook.borrow_mut().take() {
                    hook();
                }
            });
            return selected;
        }
    }
    rfd::FileDialog::new()
        .add_filter("Images", crate::image_formats::picker_image_extensions())
        .pick_file()
}

fn choose_canvas_export_path(default_name: &str) -> Option<PathBuf> {
    #[cfg(test)]
    {
        let selected = CANVAS_EXPORT_FIXTURE.with(|fixture| fixture.borrow_mut().pop());
        if selected.is_some() {
            CANVAS_PICKER_RETURN_HOOK.with(|hook| {
                if let Some(hook) = hook.borrow_mut().take() {
                    hook();
                }
            });
            return selected;
        }
    }
    rfd::FileDialog::new()
        .add_filter("Images", crate::image_formats::picker_image_extensions())
        .set_file_name(default_name)
        .save_file()
}

#[cfg(test)]
fn wait_at_canvas_worker_exit_for_test() {
    canvas_test_worker_exit_reached().store(true, Ordering::Release);
    let barrier = canvas_test_worker_exit_barrier()
        .lock()
        .unwrap()
        .clone();
    if let Some(barrier) = barrier {
        while !barrier.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
    if canvas_test_worker_panic_after_send().swap(false, Ordering::AcqRel) {
        panic!("controlled canvas panic after result send");
    }
}

#[cfg(not(test))]
fn wait_at_canvas_worker_exit_for_test() {}

impl CanvasSystemClipboard {
    fn fingerprint(&self) -> u64 {
        match self {
            Self::Image { fingerprint, .. } | Self::Text { fingerprint, .. } => *fingerprint,
        }
    }
}

fn read_canvas_system_clipboard() -> Option<CanvasSystemClipboard> {
    #[cfg(test)]
    if let Some(value) = CANVAS_CLIPBOARD_FIXTURE.with(|fixture| fixture.borrow_mut().pop()) {
        return Some(value);
    }
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if let Ok(image) = clipboard.get_image() {
        let width = image.width as u32;
        let height = image.height as u32;
        let bytes = image.bytes.into_owned();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "image".hash(&mut hasher);
        width.hash(&mut hasher);
        height.hash(&mut hasher);
        bytes.hash(&mut hasher);
        return Some(CanvasSystemClipboard::Image {
            fingerprint: hasher.finish(),
            width,
            height,
            bytes,
        });
    }

    let text = clipboard.get_text().ok()?;
    if text.trim().is_empty() {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "text".hash(&mut hasher);
    text.hash(&mut hasher);
    Some(CanvasSystemClipboard::Text {
        fingerprint: hasher.finish(),
        text,
    })
}

fn terminal_remainder_span(total: u32, parts: u32, index: u32) -> (u32, u32) {
    let base = total / parts;
    let remainder = total % parts;
    let extras_start = parts - remainder;
    let extra_before = index.saturating_sub(extras_start);
    let start = base * index + extra_before;
    let size = base + u32::from(remainder > 0 && index >= extras_start);
    (start, size)
}

fn split_parts_from_lines(lines: u32) -> Option<u32> {
    lines.checked_add(1)
}

fn split_pixel_edges(total: u32, positions: &[f32]) -> Result<Vec<u32>> {
    let mut edges = vec![0];
    for &position in positions {
        if !position.is_finite() || position <= 0.0 || position >= 1.0 {
            return Err(anyhow!("Invalid split position"));
        }
        let edge = (position as f64 * total as f64).round() as u32;
        if edge <= *edges.last().unwrap() || edge >= total {
            return Err(anyhow!("Split lines must leave at least one pixel between them"));
        }
        edges.push(edge);
    }
    edges.push(total);
    Ok(edges)
}

#[cfg(test)]
fn split_canvas_image_to_directory(
    source_path: &Path,
    output_dir: &Path,
    data_root: &Path,
    configured_output_root: &Path,
    row_positions: &[f32],
    column_positions: &[f32],
) -> Result<Vec<CanvasSplitTile>> {
    let (decoded, _) = decode_image_file(source_path)?;
    let rgba = decoded.to_rgba8();
    let (image_width, image_height) = rgba.dimensions();
    let row_edges = split_pixel_edges(image_height, row_positions)?;
    let column_edges = split_pixel_edges(image_width, column_positions)?;
    let rows = row_edges.len() as u32 - 1;
    let columns = column_edges.len() as u32 - 1;
    ensure_managed_subdirectory_at(data_root, configured_output_root, output_dir)
        .then_some(())
        .ok_or_else(|| anyhow!("unable to prepare the canvas split output directory"))?;

    let mut tiles = Vec::with_capacity((rows * columns) as usize);
    let result = (|| -> Result<()> {
        for row in 0..rows {
            let top = row_edges[row as usize];
            let tile_height = row_edges[row as usize + 1] - top;
            for column in 0..columns {
                let left = column_edges[column as usize];
                let tile_width = column_edges[column as usize + 1] - left;
                let tile =
                    image::imageops::crop_imm(&rgba, left, top, tile_width, tile_height).to_image();
                let bytes = encode_png_rgba(&tile, tile_width, tile_height)?;
                let path = output_dir.join(format!("tile-r{:02}-c{:02}.png", row + 1, column + 1));
                atomic_write_file(&path, &bytes)?;
                tiles.push(CanvasSplitTile {
                    path: path.display().to_string(),
                    row,
                    column,
                    width: tile_width,
                    height: tile_height,
                });
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(output_dir);
        return Err(error);
    }
    Ok(tiles)
}

fn split_canvas_image_for_authority(
    authority: &NamespaceStorageAuthority,
    source_path: &Path,
    row_positions: &[f32],
    column_positions: &[f32],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Vec<CanvasSplitTile>> {
    let bytes = authority.read_image_source(source_path, 100 * 1024 * 1024)?;
    let (decoded, _) = decode_image_bytes(source_path, &bytes)?;
    let rgba = decoded.to_rgba8();
    let (image_width, image_height) = rgba.dimensions();
    let row_edges = split_pixel_edges(image_height, row_positions)?;
    let column_edges = split_pixel_edges(image_width, column_positions)?;
    let rows = row_edges.len() as u32 - 1;
    let columns = column_edges.len() as u32 - 1;
    let mut tiles = Vec::with_capacity((rows * columns) as usize);
    for row in 0..rows {
        let top = row_edges[row as usize];
        let tile_height = row_edges[row as usize + 1] - top;
        for column in 0..columns {
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "canvas split cancelled");
            let left = column_edges[column as usize];
            let tile_width = column_edges[column as usize + 1] - left;
            let tile = image::imageops::crop_imm(&rgba, left, top, tile_width, tile_height).to_image();
            let bytes = encode_png_rgba(&tile, tile_width, tile_height)?;
            let path = persist_canvas_managed_image(
                authority, ManagedUserArea::Canvas, &format!("split-r{}-c{}", row + 1, column + 1), &bytes,
            )?;
            tiles.push(CanvasSplitTile { path: path.display().to_string(), row, column, width: tile_width, height: tile_height });
        }
    }
    Ok(tiles)
}

#[cfg(test)]
fn remove_canvas_split_tiles(tiles: &[CanvasSplitTile]) {
    // Indexed owned results are retained when a completion becomes stale.
    let _ = tiles;
}

#[cfg(test)]
fn extract_canvas_elements_to_directory(
    source_path: &Path,
    output_dir: &Path,
    data_root: &Path,
    configured_output_root: &Path,
) -> Result<Vec<CanvasExtractedElement>> {
    let (decoded, _) = decode_image_file(source_path)?;
    let source = decoded.to_rgba8();
    let components = extract_ui_components(&source)?;
    ensure_managed_subdirectory_at(data_root, configured_output_root, output_dir)
        .then_some(())
        .ok_or_else(|| anyhow!("unable to prepare the canvas extraction output directory"))?;

    let mut elements = Vec::with_capacity(components.len());
    let result = (|| -> Result<()> {
        for (index, component) in components.into_iter().enumerate() {
            let width = component.image.width();
            let height = component.image.height();
            let bytes = encode_png_rgba(&component.image, width, height)?;
            let path = output_dir.join(format!("element-{:02}.png", index + 1));
            atomic_write_file(&path, &bytes)?;
            elements.push(CanvasExtractedElement {
                path: path.display().to_string(),
                width,
                height,
            });
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(output_dir);
        return Err(error);
    }
    Ok(elements)
}

fn extract_canvas_elements_for_authority(
    authority: &NamespaceStorageAuthority,
    source_path: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Vec<CanvasExtractedElement>> {
    let bytes = authority.read_image_source(source_path, 100 * 1024 * 1024)?;
    let (decoded, _) = decode_image_bytes(source_path, &bytes)?;
    let components = extract_ui_components(&decoded.to_rgba8())?;
    let mut elements = Vec::with_capacity(components.len());
    for (index, component) in components.into_iter().enumerate() {
        anyhow::ensure!(!cancel.load(Ordering::Acquire), "canvas extraction cancelled");
        let width = component.image.width();
        let height = component.image.height();
        let bytes = encode_png_rgba(&component.image, width, height)?;
        let path = persist_canvas_managed_image(
            authority, ManagedUserArea::Canvas, &format!("extracted-{}", index + 1), &bytes,
        )?;
        elements.push(CanvasExtractedElement { path: path.display().to_string(), width, height });
    }
    Ok(elements)
}

#[cfg(test)]
fn remove_canvas_extracted_elements(elements: &[CanvasExtractedElement]) {
    // Indexed owned results are retained when a completion becomes stale.
    // A display path is not sufficient unlink authority.
    let _ = elements;
}

fn clear_canvas_extraction_loading(state: &AppState, source_id: &str) {
    if state.get_canvas_extraction_loading_node_id().as_str() == source_id {
        state.set_canvas_extraction_loading_node_id("".into());
    }
}

fn poll_canvas_element_extraction(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    history: Rc<RefCell<CanvasController>>,
    capture: CanvasActionCapture,
    source: CanvasSplitSource,
    receiver: Rc<RefCell<Option<mpsc::Receiver<CanvasExtractionOutcome>>>>,
    ticket: Rc<RefCell<Option<CanvasWorkerTicket>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if app_weak.upgrade().is_none() {
            if let Some(ticket) = ticket.borrow().as_ref() {
                ticket.cancel.store(true, Ordering::Release);
            }
        }
        let cancelled = ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        let worker_ready = ticket.borrow().as_ref().map(|ticket| finish_canvas_worker_if_ready(ticket.id));
        let cancelled = cancelled || ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        let worker_failed = matches!(worker_ready, Some(Err(_)));
        match worker_ready {
            Some(Ok(false)) => {
                poll_canvas_element_extraction(app_weak, store, history, capture, source, receiver, ticket);
                return;
            }
            Some(Err(_)) => {
                ticket.borrow_mut().take();
            }
            Some(Ok(true)) | None => { ticket.borrow_mut().take(); }
        }
        let outcome = if worker_failed {
            receiver.borrow_mut().take();
            Some(Err("element extraction worker failed".to_string()))
        } else {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => {
                    slot.take();
                    Some(outcome)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("element extraction worker stopped unexpectedly".to_string()))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_canvas_element_extraction(app_weak, store, history, capture, source, receiver, ticket);
            return;
        };
        if cancelled { return; }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let elements = match outcome {
            Ok(elements) => elements,
            Err(_error) => {
                let _ = capture.apply(&store, || {
                    let state = app.global::<AppState>();
                    clear_canvas_extraction_loading(&state, &source.id);
                    state.set_generation_status(
                        (if state.get_language().as_str() == "en" { "Unable to extract canvas elements" }
                        else { "无法提取画布元素" }).into(),
                    );
                });
                return;
            }
        };
        let edit = apply_canvas_edit(&app, &store, &capture, |store_mut| {
        let state = app.global::<AppState>();
        let source_is_current = store_mut.canvas_notes.iter().any(|note| {
            note.id == source.id
                && note.image_path == source.image_path
                && matches!(note.kind.as_str(), "image" | "board-image")
        });
        if !source_is_current
            || store_mut.canvas_notes.len() + elements.len() > MAX_CANVAS_NODES
            || store_mut.canvas_links.len() + elements.len() > MAX_CANVAS_LINKS
        {
            clear_canvas_extraction_loading(&state, &source.id);
            if !source_is_current {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "The source image changed before extraction finished"
                    } else {
                        "提取完成前原图已被更换，请重新操作"
                    }
                    .into(),
                );
            } else {
                show_canvas_capacity_status(&app);
            }
            return None;
        }

        history.borrow_mut().record(canvas_snapshot(&store_mut));
        clear_selection(&mut store_mut.canvas_notes);
        let next_z = store_mut
            .canvas_notes
            .iter()
            .map(|note| note.z_index)
            .max()
            .unwrap_or(0)
            + 1;
        let origin_x = source.x + source.width + 64.0;
        let origin_y = source.y;
        let cell_width = 220.0;
        let cell_height = 200.0;
        let gap = 16.0;
        let first_id = elements.first().map(|_| Uuid::new_v4().to_string());
        let mut created_ids = Vec::with_capacity(elements.len());

        for (index, element) in elements.into_iter().enumerate() {
            let id = if index == 0 {
                first_id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string())
            } else {
                Uuid::new_v4().to_string()
            };
            let column = index % 4;
            let row = index / 4;
            let mut note = CanvasNoteData {
                id: id.clone(),
                kind: "board-image".to_string(),
                content: String::new(),
                width: 180.0,
                height: 180.0,
                parent_group_id: String::new(),
                z_index: next_z + index as i32,
                image_path: element.path,
                selected: index == 0,
                ..CanvasNoteData::default()
            };
            fit_image_node_to_intrinsic_aspect(
                &mut note,
                element.width as f32,
                element.height as f32,
            );
            note.x = origin_x + column as f32 * (cell_width + gap) + (cell_width - note.width) / 2.0;
            note.y = origin_y + row as f32 * (cell_height + gap) + (cell_height - note.height) / 2.0;
            store_mut.canvas_notes.push(note);
            created_ids.push(id);
        }
        for id in &created_ids {
            let _ = connect_nodes(&mut store_mut.canvas_links, &source.id, id);
        }
        sync_canvas_selection(&app, &store_mut);
        if let Some(first_id) = first_id {
            state.set_canvas_selected_id(first_id.into());
        }
        state.set_canvas_selected_link_id("".into());
        let count = created_ids.len();
        sync_history_state(&app, &history.borrow());
        Some(count)
        });
        let Some(edit) = edit else { return; };
        let count = edit.value;
        finish_canvas_edit_with_status(
            &app, store, capture, edit,
            CanvasSaveCompletion::Extraction { source_id: source.id.clone(), count },
        );
    });
}

fn clear_canvas_split_loading(state: &AppState, source_id: &str) {
    if state.get_canvas_split_loading_node_id().as_str() == source_id {
        state.set_canvas_split_loading_node_id("".into());
    }
}

fn poll_canvas_image_split(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    history: Rc<RefCell<CanvasController>>,
    capture: CanvasActionCapture,
    source: CanvasSplitSource,
    rows: u32,
    columns: u32,
    receiver: Rc<RefCell<Option<mpsc::Receiver<CanvasSplitOutcome>>>>,
    ticket: Rc<RefCell<Option<CanvasWorkerTicket>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if app_weak.upgrade().is_none() {
            if let Some(ticket) = ticket.borrow().as_ref() {
                ticket.cancel.store(true, Ordering::Release);
            }
        }
        let cancelled = ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        let worker_ready = ticket.borrow().as_ref().map(|ticket| finish_canvas_worker_if_ready(ticket.id));
        let cancelled = cancelled || ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        let worker_failed = matches!(worker_ready, Some(Err(_)));
        match worker_ready {
            Some(Ok(false)) => {
                poll_canvas_image_split(app_weak, store, history, capture, source, rows, columns, receiver, ticket);
                return;
            }
            Some(Err(_)) => { ticket.borrow_mut().take(); }
            Some(Ok(true)) | None => { ticket.borrow_mut().take(); }
        }
        let outcome = if worker_failed {
            receiver.borrow_mut().take();
            Some(Err("image split worker failed".to_string()))
        } else {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => {
                    slot.take();
                    Some(outcome)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("image split worker stopped unexpectedly".to_string()))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_canvas_image_split(app_weak, store, history, capture, source, rows, columns, receiver, ticket);
            return;
        };
        if cancelled { return; }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let tiles = match outcome {
            Ok(tiles) => tiles,
            Err(_error) => {
                let _ = capture.apply(&store, || {
                    let state = app.global::<AppState>();
                    clear_canvas_split_loading(&state, &source.id);
                    state.set_generation_status(
                        (if state.get_language().as_str() == "en" { "Unable to split the canvas image" }
                        else { "无法分割画布图片" }).into(),
                    );
                });
                return;
            }
        };
        let edit = apply_canvas_edit(&app, &store, &capture, |store_mut| {
        let state = app.global::<AppState>();
        let source_is_current = store_mut.canvas_notes.iter().any(|note| {
            note.id == source.id
                && note.image_path == source.image_path
                && matches!(note.kind.as_str(), "image" | "board-image")
        });
        if !source_is_current
            || store_mut.canvas_notes.len() + tiles.len() > MAX_CANVAS_NODES
            || store_mut.canvas_links.len() + tiles.len() > MAX_CANVAS_LINKS
        {
            clear_canvas_split_loading(&state, &source.id);
            if !source_is_current {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "The source image changed before splitting finished"
                    } else {
                        "分割完成前原图已被更换，请重新操作"
                    }
                    .into(),
                );
            } else {
                show_canvas_capacity_status(&app);
            }
            return None;
        }

        history.borrow_mut().record(canvas_snapshot(&store_mut));
        clear_selection(&mut store_mut.canvas_notes);
        let next_z = store_mut
            .canvas_notes
            .iter()
            .map(|note| note.z_index)
            .max()
            .unwrap_or(0)
            + 1;
        let raw_tile_width = source.width / columns as f32;
        let raw_tile_height = source.height / rows as f32;
        let display_scale = (80.0 / raw_tile_width).max(80.0 / raw_tile_height).max(1.0);
        let tile_width = raw_tile_width * display_scale;
        let tile_height = raw_tile_height * display_scale;
        let gap = 16.0;
        let origin_x = source.x + source.width + 64.0;
        let origin_y = source.y;
        let first_id = tiles.first().map(|_| Uuid::new_v4().to_string());
        let mut created_ids = Vec::with_capacity(tiles.len());

        for (index, tile) in tiles.into_iter().enumerate() {
            let scale = (tile_width / tile.width as f32).min(tile_height / tile.height as f32);
            let id = if index == 0 {
                first_id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string())
            } else {
                Uuid::new_v4().to_string()
            };
            store_mut.canvas_notes.push(CanvasNoteData {
                id: id.clone(),
                kind: "board-image".to_string(),
                content: String::new(),
                x: origin_x + tile.column as f32 * (tile_width + gap),
                y: origin_y + tile.row as f32 * (tile_height + gap),
                width: tile.width as f32 * scale,
                height: tile.height as f32 * scale,
                parent_group_id: String::new(),
                z_index: next_z + index as i32,
                image_path: tile.path,
                selected: index == 0,
                ..CanvasNoteData::default()
            });
            created_ids.push(id);
        }
        for id in &created_ids {
            let _ = connect_nodes(&mut store_mut.canvas_links, &source.id, id);
        }
        sync_canvas_selection(&app, &store_mut);
        if let Some(first_id) = first_id {
            state.set_canvas_selected_id(first_id.into());
        }
        state.set_canvas_selected_link_id("".into());
        sync_history_state(&app, &history.borrow());
        Some(())
        });
        let Some(edit) = edit else { return; };
        finish_canvas_edit_with_status(
            &app, store, capture, edit,
            CanvasSaveCompletion::Split { source_id: source.id.clone(), rows, columns },
        );
    });
}

fn choose_canvas_image_for_capture(capture: &CanvasActionCapture) -> Option<PathBuf> {
    let _dialog_activity = capture.persistence.begin_activity().ok()?;
    choose_canvas_image_path()
}

enum CanvasImageImportSource {
    Selected(PathBuf),
    Clipboard { width: u32, height: u32, bytes: Vec<u8> },
}

enum CanvasImageImportTarget {
    Existing { id: String, original_path: String },
    New { id: String, center_x: f32, center_y: f32 },
}

fn next_canvas_image_import_request(epoch: &Cell<u64>) -> Option<u64> {
    let next = epoch.get().checked_add(1)?;
    epoch.set(next);
    Some(next)
}

fn start_canvas_image_import(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    history: Rc<RefCell<CanvasController>>,
    capture: CanvasActionCapture,
    request_epoch: Rc<Cell<u64>>,
    request_id: u64,
    source: CanvasImageImportSource,
    target: CanvasImageImportTarget,
) -> Result<()> {
    let persistence = capture.persistence.clone();
    let worker_persistence = persistence.clone();
    let (sender, receiver) = mpsc::channel();
    let ticket = spawn_canvas_worker(persistence, move |cancel, _activity| {
        let result = (|| -> Result<PreparedCanvasImage> {
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "canvas image import cancelled");
            let authority = worker_persistence.storage_authority()?;
            let (source_name, bytes) = match source {
                CanvasImageImportSource::Selected(path) => {
                    let bytes = authority.read_image_source(&path, 100 * 1024 * 1024)?;
                    (path, bytes)
                }
                CanvasImageImportSource::Clipboard { width, height, bytes } => {
                    let rgba = image::RgbaImage::from_raw(width, height, bytes)
                        .ok_or_else(|| anyhow!("invalid clipboard pixels"))?;
                    (PathBuf::from("clipboard.png"), encode_png_rgba(&rgba, width, height)?)
                }
            };
            let (decoded, _) = decode_image_bytes(&source_name, &bytes)?;
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "canvas image import cancelled");
            let path = persist_canvas_managed_image(
                &authority, ManagedUserArea::CanvasUploads, "canvas-import", &bytes,
            )?;
            Ok(PreparedCanvasImage {
                path: path.display().to_string(),
                width: decoded.width() as f32,
                height: decoded.height() as f32,
            })
        })();
        let _ = sender.send(result);
        wait_at_canvas_worker_exit_for_test();
    })?;
    poll_canvas_image_import(
        app.as_weak(), store, history, capture, request_epoch, request_id,
        target, receiver, Rc::new(RefCell::new(Some(ticket))),
    );
    Ok(())
}

fn poll_canvas_image_import(
    weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    history: Rc<RefCell<CanvasController>>,
    capture: CanvasActionCapture,
    request_epoch: Rc<Cell<u64>>,
    request_id: u64,
    target: CanvasImageImportTarget,
    receiver: mpsc::Receiver<Result<PreparedCanvasImage>>,
    ticket: Rc<RefCell<Option<CanvasWorkerTicket>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        if weak.upgrade().is_none() {
            if let Some(ticket) = ticket.borrow().as_ref() {
                ticket.cancel.store(true, Ordering::Release);
            }
        }
        let cancelled = ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        let ready = ticket.borrow().as_ref().map(|ticket| finish_canvas_worker_if_ready(ticket.id));
        let worker_failed = matches!(ready, Some(Err(_)));
        let cancelled = cancelled || ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        if matches!(ready, Some(Ok(false))) {
            poll_canvas_image_import(
                weak, store, history, capture, request_epoch, request_id, target, receiver, ticket,
            );
            return;
        }
        ticket.borrow_mut().take();
        if cancelled {
            return;
        }
        let result = if worker_failed {
            Err(anyhow!("canvas image import worker failed"))
        } else {
            receiver.try_recv().unwrap_or_else(|_| Err(anyhow!("canvas image import worker stopped unexpectedly")))
        };
        let Some(app) = weak.upgrade() else { return; };
        if request_epoch.get() != request_id { return; }
        let image = match result {
            Ok(image) => image,
            Err(_error) => {
                let _ = capture.apply(&store, || {
                    let state = app.global::<AppState>();
                    state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "Unable to import the canvas image".to_string()
                    } else {
                        "无法导入画布图片".to_string()
                    }
                    .into())
                });
                return;
            }
        };
        let edit = apply_canvas_edit(&app, &store, &capture, |store_mut| {
            if request_epoch.get() != request_id { return None; }
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES
                && matches!(&target, CanvasImageImportTarget::New { .. })
            {
                show_canvas_capacity_status(&app);
                return None;
            }
            let result = match target {
                CanvasImageImportTarget::Existing { id, original_path } => {
                    let Some(index) = store_mut.canvas_notes.iter().position(|node| {
                        node.id == id && node.kind == "image" && node.image_path == original_path
                    }) else { return None; };
                    history.borrow_mut().record(canvas_snapshot(store_mut));
                    store_mut.canvas_notes[index].image_path = image.path;
                    fit_image_node_to_intrinsic_aspect(
                        &mut store_mut.canvas_notes[index], image.width, image.height,
                    );
                    (id, "Image added to the node", "图片已添加到节点")
                }
                CanvasImageImportTarget::New { id, center_x, center_y } => {
                    history.borrow_mut().record(canvas_snapshot(store_mut));
                    let (_, width, height) = canvas_node_defaults("image", false);
                    let mut note = CanvasNoteData {
                        id: id.clone(), kind: "board-image".into(),
                        x: center_x - width / 2.0, y: center_y - height / 2.0,
                        width, height, image_path: image.path, selected: true,
                        ..CanvasNoteData::default()
                    };
                    fit_image_node_to_intrinsic_aspect(&mut note, image.width, image.height);
                    clear_selection(&mut store_mut.canvas_notes);
                    store_mut.canvas_notes.push(note);
                    (id, "Image added to the canvas", "图片已添加到画布")
                }
            };
            let state = app.global::<AppState>();
            sync_canvas_selection(&app, store_mut);
            state.set_canvas_selected_id(result.0.clone().into()); state.set_canvas_selected_link_id("".into());
            sync_history_state(&app, &history.borrow()); Some((result.1, result.2))
        });
        let Some(edit) = edit else { return; };
        let (success_en, success_zh) = edit.value;
        finish_canvas_edit_with_status(
            &app, store, capture, edit,
            CanvasSaveCompletion::Status { success_en, success_zh },
        );
    });
}

#[cfg(test)]
fn start_viewer_image_import_to_canvas(
    app: &AppWindow,
    context: AppContext,
    source_path: PathBuf,
    completed: impl FnOnce(Result<()>) + 'static,
) -> Result<()> {
    let workspace=normalize_canvas_workspace_id(&context.store.borrow().active_canvas_workspace_id);
    start_captured_viewer_image_import_to_canvas(app, context, source_path, workspace, |_,_,_|true, completed)
}

// Private staged identity: never a durable save receipt.
#[derive(Clone)]
struct ViewerCanvasFileIdentity { key:ManagedFileKey, indexed:ManagedFileRecord, sha256:String }
impl ViewerCanvasFileIdentity {
    fn capture(authority:&NamespaceStorageAuthority,path:&Path)->Result<Self> {
        let relative=path.strip_prefix(authority.lease().namespace.path(ManagedUserArea::CanvasUploads))?;
        let key=ManagedFileKey::new(ManagedUserArea::CanvasUploads,relative.to_str().ok_or_else(||anyhow!("canvas image path encoding"))?)?;
        let mut file=authority.open_existing_regular(&key)?;
        let indexed=authority.delivery_index()?.find_file_by_path_for_namespace(authority,key.area(),key.relative_name().as_str())?
            .ok_or_else(||anyhow!("canvas import index missing"))?;
        let metadata=authority.inspect_regular(&file)?;
        anyhow::ensure!(metadata.link_count==1 && metadata.byte_size<=100*1024*1024 && metadata.byte_size>0
            && metadata.identity==indexed.physical_identity && !indexed.pending_delete,"canvas imported file changed");
        let sha256=Self::hash(authority,&mut file)?;
        let identity=Self{key,indexed,sha256};identity.verify(authority,&mut file)?;Ok(identity)
    }
    fn hash(authority:&NamespaceStorageAuthority,file:&mut NamespaceManagedFile)->Result<String>{
        use sha2::Digest;
        authority.with_regular_reader(file,|reader|{
            use std::io::Read;
            let mut bytes=Vec::new();reader.take(100*1024*1024+1).read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len()<=100*1024*1024,"canvas input too large");
            Ok(format!("{:x}",sha2::Sha256::digest(&bytes)))
        })
    }
    fn verify(&self,authority:&NamespaceStorageAuthority,file:&mut NamespaceManagedFile)->Result<()> {
        let metadata=authority.inspect_regular(file)?;
        let current=authority.delivery_index()?.find_file_by_path_for_namespace(authority,self.key.area(),self.key.relative_name().as_str())?
            .ok_or_else(||anyhow!("staged canvas index missing"))?;
        anyhow::ensure!(current.id==self.indexed.id && current.physical_identity==self.indexed.physical_identity
            && current.byte_size==self.indexed.byte_size && !current.pending_delete
            && metadata.identity==self.indexed.physical_identity && metadata.byte_size==self.indexed.byte_size
            && metadata.link_count==1 && Self::hash(authority,file)?==self.sha256,"staged canvas input changed");
        Ok(())
    }
}
pub(super) struct StagedViewerCanvasImport {
    persistence:PrivatePersistence,workspace:String,id:String,path:String,file:ViewerCanvasFileIdentity,
}
pub(super) type ViewerCanvasImportStage=Rc<RefCell<Option<Rc<StagedViewerCanvasImport>>>>;

pub(super) fn retry_staged_viewer_canvas_import(
    app:&AppWindow,context:AppContext,staged:Rc<StagedViewerCanvasImport>,
    source_current:impl Fn(&AppWindow,&AppContext,bool)->bool+'static,
    completed:impl FnOnce(Result<()>)+'static,
)->Result<()> {
    anyhow::ensure!(staged.persistence.is_current() && source_current(app,&context,true),"staged original viewer changed");
    let capture=CanvasActionCapture{persistence:staged.persistence.clone(),workspace_id:staged.workspace.clone()};
    anyhow::ensure!(capture.binding_matches_without_latch(&context.store.borrow()),"staged canvas binding changed");
    let target=ViewerCanvasTarget{workspace:staged.workspace.clone(),context:context.clone(),source_current:Rc::new(source_current),stage:Rc::new(RefCell::new(Some(staged.clone())))};
    let identity=staged.file.clone();let worker_persistence=staged.persistence.clone();
    let (ready_tx,ready_rx)=mpsc::channel();let(command_tx,command_rx)=mpsc::channel::<mpsc::Receiver<WriteResult>>();
    let(ack_tx,ack_rx)=mpsc::channel();
    let ticket=spawn_canvas_worker(staged.persistence.clone(),move|cancel,activity|{
        let prepared=(||->Result<(Arc<NamespaceStorageAuthority>,NamespaceManagedFile)>{
            let authority=worker_persistence.storage_authority()?;
            let mut file=authority.open_existing_regular(&identity.key)?;identity.verify(&authority,&mut file)?;
            anyhow::ensure!(!cancel.load(Ordering::Acquire) && !activity.is_quiescing(),"staged canvas retry cancelled");
            Ok((authority,file))
        })();
        let(authority,mut file)=match prepared {
            Ok(value)=>value,Err(error)=>{let _=ready_tx.send(Err(error));return;}
        };
        if ready_tx.send(Ok(())).is_err(){return;}
        // No UI Drop joins this live worker. Cancellation must also break the
        // pre-enqueue handoff while an event loop is shutting down.
        let receiver=loop {
            if cancel.load(Ordering::Acquire) || activity.is_quiescing(){return;}
            match command_rx.recv_timeout(Duration::from_millis(25)) {
                Ok(receiver)=>break receiver,
                Err(mpsc::RecvTimeoutError::Timeout)=>continue,
                Err(mpsc::RecvTimeoutError::Disconnected)=>return,
            }
        };
        let saved=matches!(receiver.recv(),Ok(Ok(()))) && !cancel.load(Ordering::Acquire) && !activity.is_quiescing();
        let acknowledged=saved && identity.verify(&authority,&mut file).is_ok();
        let _=ack_tx.send(acknowledged);
    })?;
    poll_staged_viewer_canvas_retry(app.as_weak(),context,capture,target,staged,ready_rx,command_tx,ack_rx,
        Rc::new(RefCell::new(Some(ticket))),Rc::new(RefCell::new(Some(Box::new(completed)))));
    Ok(())
}

fn poll_staged_viewer_canvas_retry(
    weak:Weak<AppWindow>,context:AppContext,capture:CanvasActionCapture,target:ViewerCanvasTarget,
    staged:Rc<StagedViewerCanvasImport>,ready:mpsc::Receiver<Result<()>>,
    command:mpsc::Sender<mpsc::Receiver<WriteResult>>,acknowledgment:mpsc::Receiver<bool>,
    ticket:Rc<RefCell<Option<CanvasWorkerTicket>>>,completed:ViewerCanvasImportCompletion,
){
    slint::Timer::single_shot(Duration::from_millis(50),move||{
        let app=weak.upgrade();
        let worker=ticket.borrow().as_ref().map(|ticket|(ticket.id,ticket.cancel.clone()));
        let Some((id,cancel))=worker else{return;};
        let finished=finish_canvas_worker_if_ready(id);
        let outcome=ready.try_recv();
        let still_current=app.as_ref().is_some_and(|app|capture.persistence.is_current()
            && target.is_current(app,&context,true));
        if still_current && matches!(finished,Ok(false)) && matches!(outcome,Err(mpsc::TryRecvError::Empty)) {
            poll_staged_viewer_canvas_retry(weak,context,capture,target,staged,ready,command,acknowledgment,ticket,completed);return;
        }
        let mut admitted=false;
        if still_current && matches!(finished,Ok(false)) && matches!(&outcome,Ok(Ok(()))) {
            let app=app.as_ref().unwrap();
            if let Ok(write)=capture.persistence.prepare_ordered_save(){
                let mut write=Some(write);
                let queued=capture.apply(&context.store,||{
                    if !target.is_current(app,&context,true){return None;}
                    let store=context.store.borrow();
                    if !store.canvas_notes.iter().any(|note|note.id==staged.id && note.kind=="board-image" && note.image_path==staged.path){return None;}
                    Some(write.take().unwrap().enqueue(local_store_data(app,&store)))
                }).flatten();
                drop(write);
                if let Some(Ok(receiver))=queued {admitted=command.send(receiver).is_ok();}
            }
        }
        drop(outcome);drop(command);
        if admitted {
            poll_canvas_save_status(weak,context.store,capture,acknowledgment,ticket,
                CanvasSaveCompletion::Viewer{id:staged.id.clone(),completed,target});
        }else{
            cancel.store(true,Ordering::Release);ticket.borrow_mut().take();schedule_canvas_worker_reap(id);
            complete_viewer_canvas_import(&completed,Err(anyhow!("staged canvas save was not confirmed")));
        }
    });
}

#[derive(Clone)]
struct ViewerCanvasTarget {
    workspace: String,
    context: AppContext,
    source_current: Rc<dyn Fn(&AppWindow, &AppContext, bool) -> bool>,
    stage:ViewerCanvasImportStage,
}

impl ViewerCanvasTarget {
    fn is_current(&self,app:&AppWindow,context:&AppContext,after:bool)->bool {
        if !(self.source_current)(app,context,after){return false;}
        if !after{return true;}
        let stage=self.stage.borrow();
        let Some(stage)=stage.as_ref()else{return false;};
        let store=context.store.borrow();
        normalize_canvas_workspace_id(&store.active_canvas_workspace_id)==stage.workspace
            && store.private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(&stage.persistence))
            && store.canvas_notes.iter().any(|note|note.id==stage.id && note.kind=="board-image" && note.image_path==stage.path)
    }
}

/// The source predicate is pure metadata and is checked before target mutation
/// and again after the real writer acknowledgement. It never grants file access.
pub(super) fn start_captured_viewer_image_import_to_canvas(
    app: &AppWindow,
    context: AppContext,
    source_path: PathBuf,
    target_workspace: String,
    source_current: impl Fn(&AppWindow, &AppContext, bool) -> bool + 'static,
    completed: impl FnOnce(Result<()>) + 'static,
) -> Result<()> {
    start_captured_viewer_image_import_to_canvas_with_stage(app,context,source_path,target_workspace,
        Rc::new(RefCell::new(None)),source_current,completed)
}

pub(super) fn start_captured_viewer_image_import_to_canvas_with_stage(
    app:&AppWindow,context:AppContext,source_path:PathBuf,target_workspace:String,
    stage:ViewerCanvasImportStage,
    source_current:impl Fn(&AppWindow,&AppContext,bool)->bool+'static,
    completed:impl FnOnce(Result<()>)+'static,
)->Result<()> {
    anyhow::ensure!(source_current(app, &context, false), "original viewer source changed");
    let target=ViewerCanvasTarget { workspace:normalize_canvas_workspace_id(&target_workspace),
        context:context.clone(), source_current:Rc::new(source_current),stage };
    let capture = CanvasActionCapture::capture(&context.store)
        .ok_or_else(|| anyhow!("canvas Store not activated"))?;
    let entry = capture.begin_effect(&context.store)
        .ok_or_else(|| anyhow!("canvas binding changed"))?;
    // Capacity belongs to the explicit destination and is checked atomically
    // with its mutation below, not against the unrelated source workspace.
    let persistence = entry.persistence.clone();
    drop(entry);
    let worker_persistence = persistence.clone();
    let (sender, receiver) = mpsc::channel();
    let ticket = spawn_canvas_worker(persistence, move |cancel, _activity| {
        let result = (|| -> Result<(PreparedCanvasImage,ViewerCanvasFileIdentity)> {
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "canvas import cancelled");
            let authority = worker_persistence.storage_authority()?;
            let bytes = authority.read_image_source(&source_path, 100 * 1024 * 1024)?;
            let (decoded, _) = decode_image_bytes(&source_path, &bytes)?;
            let path = persist_canvas_managed_image(
                &authority, ManagedUserArea::CanvasUploads, "viewer-import", &bytes,
            )?;
            let identity=ViewerCanvasFileIdentity::capture(&authority,&path)?;
            Ok((PreparedCanvasImage {
                path: path.display().to_string(),
                width: decoded.width() as f32,
                height: decoded.height() as f32,
            },identity))
        })();
        let _ = sender.send(result);
    })?;
    poll_viewer_canvas_import(
        app.as_weak(), context, capture, target, receiver, Rc::new(RefCell::new(Some(ticket))),
        Rc::new(RefCell::new(Some(Box::new(completed)))),
    );
    Ok(())
}

type ViewerCanvasImportCompletion = Rc<RefCell<Option<Box<dyn FnOnce(Result<()>)>>>>;

fn complete_viewer_canvas_import(
    completed: &ViewerCanvasImportCompletion,
    result: Result<()>,
) {
    let callback = { completed.borrow_mut().take() };
    if let Some(callback) = callback { callback(result); }
}

fn poll_viewer_canvas_import(
    weak: Weak<AppWindow>,
    context: AppContext,
    capture: CanvasActionCapture,
    target: ViewerCanvasTarget,
    receiver: mpsc::Receiver<Result<(PreparedCanvasImage,ViewerCanvasFileIdentity)>>,
    ticket: Rc<RefCell<Option<CanvasWorkerTicket>>>,
    completed: ViewerCanvasImportCompletion,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        if weak.upgrade().is_none() {
            if let Some(ticket) = ticket.borrow().as_ref() {
                ticket.cancel.store(true, Ordering::Release);
            }
        }
        let cancelled = ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        let ready = ticket.borrow().as_ref().map(|ticket| finish_canvas_worker_if_ready(ticket.id));
        let worker_failed = matches!(ready, Some(Err(_)));
        let cancelled = cancelled || ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        if matches!(ready, Some(Ok(false))) {
            poll_viewer_canvas_import(weak, context, capture, target, receiver, ticket, completed);
            return;
        }
        ticket.borrow_mut().take();
        if cancelled || worker_failed {
            complete_viewer_canvas_import(
                &completed, Err(anyhow!("canvas import worker did not complete safely")),
            );
            return;
        }
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(_) => {
                complete_viewer_canvas_import(
                    &completed, Err(anyhow!("canvas import result was unavailable")),
                );
                return;
            }
        };
        let (image,identity) = match result {
            Ok(image) => image,
            Err(error) => {
                complete_viewer_canvas_import(&completed, Err(error));
                return;
            }
        };
        let Some(app) = weak.upgrade() else {
            complete_viewer_canvas_import(
                &completed, Err(anyhow!("canvas window closed before import completion")),
            );
            return;
        };
        let applied = apply_canvas_edit_checked(&app, &context.store, &capture,
            || target.is_current(&app, &context, false), |store| {
            let target_count=if normalize_canvas_workspace_id(&store.active_canvas_workspace_id)==target.workspace {
                store.canvas_notes.len()
            } else { store.canvas_workspaces.get(&target.workspace).map_or(0, |workspace|workspace.notes.len()) };
            if target_count >= MAX_CANVAS_NODES { return None; }
            let prompt=switch_canvas_workspace(store, app.global::<AppState>().get_canvas_workflow_prompt().as_str(), &target.workspace);
            app.global::<AppState>().set_canvas_workflow_prompt(prompt.into());
            let id = Uuid::new_v4().to_string();
            let (_, width, height) = canvas_node_defaults("image", false);
            let mut note = CanvasNoteData {
                id: id.clone(), kind: "board-image".into(), width, height,
                image_path: image.path, selected: true, ..CanvasNoteData::default()
            };
            fit_image_node_to_intrinsic_aspect(&mut note, image.width, image.height);
            let mut anchors = selected_ids(&store.canvas_notes);
            if anchors.is_empty() { anchors.extend(store.canvas_notes.iter().map(|item| item.id.clone())); }
            if let Some(bounds) = selection_bounds(&store.canvas_notes, &anchors) {
                note.x = bounds.x + bounds.width + 64.0;
                note.y = bounds.y;
            }
            clear_selection(&mut store.canvas_notes);
            let path=note.image_path.clone();
            store.canvas_notes.push(note);
            // Minted only after this exact node was actually staged, before
            // enqueue can fail. It authorizes a retry, never a success claim.
            *target.stage.borrow_mut()=Some(Rc::new(StagedViewerCanvasImport{
                persistence:capture.persistence.clone(),workspace:target.workspace.clone(),
                id:id.clone(),path,file:identity,
            }));
            sync_canvas_selection(&app, store);
            let references=prepare_canvas_reference_projection(&app, store).publish_metadata(&app);
            Some((id, references))
        });
        let Some(applied) = applied else {
            complete_viewer_canvas_import(
                &completed, Err(anyhow!("canvas import save was not admitted")),
            );
            return;
        };
        let AppliedCanvasEdit { value: (id, references), persistence, effects, ticket, acknowledgment } = applied;
        start_canvas_reference_preview_effects(&app, persistence.clone(), references);
        start_canvas_edit_preview(&app, persistence, effects);
        let capture=CanvasActionCapture { persistence:capture.persistence, workspace_id:target.workspace.clone() };
        poll_canvas_save_status(
            app.as_weak(), context.store, capture, acknowledgment,
            Rc::new(RefCell::new(Some(ticket))),
            CanvasSaveCompletion::Viewer { id, completed, target },
        );
    });
}

fn target_at_input(
    store: &Store,
    source_id: &str,
    x: f32,
    y: f32,
    tolerance: f32,
) -> Option<String> {
    store
        .canvas_notes
        .iter()
        .filter(|note| note.id != source_id && note.kind != "group")
        .filter_map(|note| {
            let dx = note.x - x;
            let dy = note.y + note.height / 2.0 - y;
            let distance = (dx * dx + dy * dy).sqrt();
            (distance <= tolerance).then_some((distance, note.id.clone()))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, id)| id)
}

fn source_at_output(
    store: &Store,
    target_id: &str,
    x: f32,
    y: f32,
    tolerance: f32,
) -> Option<String> {
    store
        .canvas_notes
        .iter()
        .filter(|note| note.id != target_id && note.kind != "group")
        .filter_map(|note| {
            let dx = note.x + note.width - x;
            let dy = note.y + note.height / 2.0 - y;
            let distance = (dx * dx + dy * dy).sqrt();
            (distance <= tolerance).then_some((distance, note.id.clone()))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, id)| id)
}

fn canvas_node_defaults(kind: &str, english: bool) -> (String, f32, f32) {
    match kind {
        "image" => (String::new(), 340.0, 250.0),
        "video" => (String::new(), 400.0, 270.0),
        "audio" => (String::new(), 340.0, 190.0),
        "group" => (
            if english { "Group" } else { "节点组" }.to_string(),
            680.0,
            360.0,
        ),
        _ => (String::new(), 320.0, 210.0),
    }
}

fn sync_history_state(app: &AppWindow, history: &CanvasController) {
    let state = app.global::<AppState>();
    state.set_canvas_can_undo(history.can_undo());
    state.set_canvas_can_redo(history.can_redo());
}

enum CanvasSaveCompletion {
    Status { success_en: &'static str, success_zh: &'static str },
    Split { source_id: String, rows: u32, columns: u32 },
    Extraction { source_id: String, count: usize },
    Viewer {
        id: String,
        completed: ViewerCanvasImportCompletion,
        target: ViewerCanvasTarget,
    },
}

fn poll_canvas_save_status(
    weak: Weak<AppWindow>, store: Rc<RefCell<Store>>, capture: CanvasActionCapture,
    receiver: mpsc::Receiver<bool>, ticket: Rc<RefCell<Option<CanvasWorkerTicket>>>,
    completion: CanvasSaveCompletion,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        if weak.upgrade().is_none() {
            if let Some(ticket) = ticket.borrow().as_ref() {
                ticket.cancel.store(true, Ordering::Release);
            }
        }
        let cancelled = ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        let ready = ticket.borrow().as_ref().map(|ticket| finish_canvas_worker_if_ready(ticket.id));
        let worker_failed = matches!(ready, Some(Err(_)));
        let cancelled = cancelled || ticket.borrow().as_ref()
            .is_some_and(|ticket| ticket.cancel.load(Ordering::Acquire));
        if matches!(ready, Some(Ok(false))) {
            poll_canvas_save_status(weak, store, capture, receiver, ticket, completion);
            return;
        }
        ticket.borrow_mut().take();
        if cancelled {
            if let CanvasSaveCompletion::Viewer { completed, .. } = &completion {
                complete_viewer_canvas_import(
                    completed,
                    Err(anyhow!("canvas save completion was cancelled")),
                );
            }
            return;
        }
        let acknowledged = !worker_failed && receiver.try_recv().unwrap_or(false);
        let Some(app) = weak.upgrade() else {
            if let CanvasSaveCompletion::Viewer { completed, .. } = &completion {
                complete_viewer_canvas_import(
                    completed, Err(anyhow!("canvas window closed before save completion")),
                );
            }
            return;
        };
        let mut viewer_completion = None;
        let applied = capture.apply(&store, || {
            if let CanvasSaveCompletion::Viewer { completed, target, .. } = &completion {
                if !target.is_current(&app, &target.context, true) {
                    viewer_completion=Some((completed.clone(),Err(anyhow!("original viewer source changed before save completion"))));
                    return;
                }
            }
            let state = app.global::<AppState>();
            let english = state.get_language().as_str() == "en";
            let message = if !acknowledged {
                if english { "Canvas save was not confirmed".to_string() } else { "画布保存未确认".to_string() }
            } else {
                match &completion {
                    CanvasSaveCompletion::Status { success_en, success_zh } =>
                        if english { (*success_en).to_string() } else { (*success_zh).to_string() },
                    CanvasSaveCompletion::Split { source_id, rows, columns } => {
                        clear_canvas_split_loading(&state, source_id);
                        if english { format!("Split evenly into {rows} rows × {columns} columns") }
                        else { format!("已平均分割为 {rows} 行 × {columns} 列，共 {} 张", *rows * *columns) }
                    }
                    CanvasSaveCompletion::Extraction { source_id, count } => {
                        clear_canvas_extraction_loading(&state, source_id);
                        if english { format!("Extracted {count} transparent PNG elements from the current image") }
                        else { format!("已从当前图片提取 {count} 个透明 PNG 元素") }
                    }
                    CanvasSaveCompletion::Viewer { id, completed, .. } => {
                        state.set_canvas_selected_id(id.clone().into());
                        state.set_canvas_focus_request(state.get_canvas_focus_request().saturating_add(1));
                        viewer_completion = Some((completed.clone(), Ok(())));
                        if english { "Image added to the canvas".to_string() }
                        else { "图片已添加到画布".to_string() }
                    }
                }
            };
            if !acknowledged {
                match &completion {
                    CanvasSaveCompletion::Split { source_id, .. } => clear_canvas_split_loading(&state, source_id),
                    CanvasSaveCompletion::Extraction { source_id, .. } => clear_canvas_extraction_loading(&state, source_id),
                    CanvasSaveCompletion::Status { .. } => {}
                    CanvasSaveCompletion::Viewer { completed, .. } => {
                        viewer_completion = Some((
                            completed.clone(), Err(anyhow!("canvas save was not confirmed")),
                        ));
                    }
                }
            }
            state.set_generation_status(message.into());
        });
        if applied.is_none() {
            if let CanvasSaveCompletion::Viewer { completed, .. } = &completion {
                viewer_completion = Some((
                    completed.clone(), Err(anyhow!("canvas binding changed before save completion")),
                ));
            }
        }
        if let Some((completed, result)) = viewer_completion {
            complete_viewer_canvas_import(&completed, result);
        }
    });
}

fn migrate_legacy_auto_sized_canvas_images(notes: &mut [CanvasNoteData]) -> bool {
    // Legacy dimensions are preserved until a captured, owned image operation
    // has bytes in hand. Opening a workspace must never inspect saved paths.
    let _ = notes;
    false
}

fn show_canvas_capacity_status(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_generation_status(
        if state.get_language().as_str() == "en" {
            "Canvas limit reached (200 nodes / 400 connections)."
        } else {
            "画布已达到上限（200 个节点 / 400 条连线）。"
        }
        .into(),
    );
}

fn sync_canvas_selection_metrics(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let ids = selected_ids(&store.canvas_notes);
    state.set_canvas_selected_count(ids.len() as i32);
    if let Some(bounds) = selection_bounds(&store.canvas_notes, &ids) {
        state.set_canvas_focus_x(bounds.x);
        state.set_canvas_focus_y(bounds.y);
        state.set_canvas_focus_width(bounds.width);
        state.set_canvas_focus_height(bounds.height);
    } else {
        state.set_canvas_focus_width(0.0);
        state.set_canvas_focus_height(0.0);
    }
}

fn sync_canvas_selection(app: &AppWindow, store: &Store) {
    sync_canvas_selection_rows(app, store);
}

fn sync_canvas_selection_rows(app: &AppWindow, store: &Store) {
    sync_canvas_selection_metrics(app, store);

    let state = app.global::<AppState>();
    let canvas_notes = state.get_canvas_notes();
    for row in 0..canvas_notes.row_count() {
        let Some(mut note) = canvas_notes.row_data(row) else {
            continue;
        };
        let selected = store
            .canvas_notes
            .iter()
            .find(|stored| stored.id == note.id.as_str())
            .is_some_and(|stored| stored.selected);
        if note.selected != selected {
            note.selected = selected;
            canvas_notes.set_row_data(row, note);
        }
    }

    let canvas_links = state.get_canvas_links();
    for row in 0..canvas_links.row_count() {
        let Some(mut link) = canvas_links.row_data(row) else {
            continue;
        };
        let source_selected = store
            .canvas_notes
            .iter()
            .find(|note| note.id == link.source_id.as_str())
            .is_some_and(|note| note.selected);
        let target_selected = store
            .canvas_notes
            .iter()
            .find(|note| note.id == link.target_id.as_str())
            .is_some_and(|note| note.selected);
        if link.source_selected != source_selected || link.target_selected != target_selected {
            link.source_selected = source_selected;
            link.target_selected = target_selected;
            canvas_links.set_row_data(row, link);
        }
    }
}

pub(super) fn normalize_canvas_workflow_prompt(prompt: &str) -> String {
    if !prompt
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '\u{2028}' | '\u{2029}'))
    {
        return prompt.to_string();
    }

    prompt
        .split(|character| matches!(character, '\r' | '\n' | '\u{2028}' | '\u{2029}'))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn compose_canvas_workflow_prompt(
    template: &str,
    user_description: &str,
    requested_step_count: i32,
    english: bool,
) -> String {
    let user_description = user_description.trim();
    if template.trim().is_empty() {
        return user_description.to_string();
    }
    if template.contains("横版无缝地图规范：")
        || template.contains("Side-scrolling seamless map specification:")
    {
        return compose_side_scroll_map_prompt(user_description, english);
    }

    let step_count = requested_step_count.clamp(4, 12);
    let top_count = (step_count + 1) / 2;
    let bottom_count = step_count / 2;
    let template = template
        .trim()
        .replace("{count}", &step_count.to_string());
    let is_upgrade_evolution = if english {
        template.contains("consecutive upgrade-evolution stages")
    } else {
        template.contains("连续升级进化阶段")
    };
    let workflow_visual_rules = if is_upgrade_evolution && english {
        "Mandatory upgrade-tier visuals: automatically map the actual stages from low to high through the universal rarity order white, green, blue, purple, orange, and red. The lowest stage must begin with white-quality details and the highest base stage must reach red-quality details; when there are fewer than six stages, sample this sequence evenly while preserving its order. Tier colors are localized quality accents only, applied to accessories, weapons, armor trim, gems, emblems, functional parts, mechanical modules, crystals, or energy lines. Never tint the whole subject or change its original skin tone, hair color, outfit main color, core palette, base material, or identity. Beyond the six base tiers, add soft localized back glows in this order: green, blue, purple, gold, and red. Confine each subject's back glow and every other effect to its own isolated area; they must not cross the solid-background gap between subjects or touch or connect to neighboring effects. Keep the base background uniform outside each glow."
    } else if is_upgrade_evolution {
        "强制升级视觉等级：根据实际阶段数量，从低到高自动映射白、绿、蓝、紫、橙、红的通用稀有度顺序。最低阶必须从白色品质细节开始，最高基础阶必须达到红色品质细节；不足六阶时按顺序均匀取样。等级色只能作为局部品质标识，用于配饰、武器、护甲镶边、宝石、纹章、功能部件、机械模块、晶体或能量纹路等，不得给整个主体统一染色，不得改变主体原有肤色、发色、服装主色、核心配色、基础材质和身份特征。超过基础六阶后，依次增加绿光、蓝光、紫光、金光、红光的柔和局部背光。每个主体的背光和其他光效必须限制在自己的独立区域内，不得跨越主体之间的纯色背景间距，不得与相邻主体的光效接触或相连；光晕区域之外的基础背景必须保持均匀纯色。"
    } else {
        ""
    };
    let permits_localized_glow = is_upgrade_evolution;
    let is_building_derivation = template.contains("建筑功能衍生：")
        || template.contains("Building function derivation:");
    let composition_rules = if english {
        let background_rule = if permits_localized_glow {
            "Outside any workflow-requested soft localized back glow strictly confined behind one subject, do not add gradients, textures, patterns, scenery, environments, decorations, or any other background elements."
        } else {
            "Do not add gradients, textures, patterns, scenery, environments, decorations, or any other background elements."
        };
        let layout = if step_count > 5 {
            format!(
                "Arrange all {step_count} subjects in two rows, ordered left to right and then top to bottom: exactly {top_count} subjects in the top row and {bottom_count} in the bottom row.{}",
                if is_building_derivation { " Order buildings by function, not by upgrade level." } else { "" }
            )
        } else if is_building_derivation {
            format!("Arrange all {step_count} buildings in one row by function, not by upgrade level.")
        } else {
            format!("Arrange all {step_count} subjects in one row in progression order.")
        };
        format!(
            "Mandatory composition rules: use one solid-color background only. {background_rule} Do not include numbers, numbering, text labels, titles, captions, explanatory text, or watermarks. Keep a clear, continuous solid-background gap between every pair of subjects. No silhouettes, clothing, weapons, gear, effects, or shadows may touch, overlap, or connect. If space is insufficient, uniformly scale down all subjects within the image; keep the selected canvas ratio and normal 2K or 4K output dimensions unchanged. Prefer more empty space over compressed gaps so that each subject can be cleanly extracted on its own. {layout} Count contract: exactly {step_count} complete subjects, with one independent subject in every planned position and no empty positions. The reference is not an extra output subject. Stage examples never limit the selected count; add distinct intermediate stages as needed, without merging or omitting subjects. If space is insufficient, shrink subjects, never reduce their count. Check each row and the total before finalizing; the visible total must equal {step_count}. Do not draw these counting instructions or any numbers on the image."
        )
    } else {
        let background_rule = if permits_localized_glow {
            "除工作流明确要求且严格限制在单个主体后方的局部柔和光晕外，不得添加渐变、纹理、图案、风景、环境、装饰或其他背景元素。"
        } else {
            "不得添加渐变、纹理、图案、风景、环境、装饰或其他背景元素。"
        };
        let layout = if step_count > 5 {
            format!(
                "将全部{step_count}个对象分成上下两行，上排恰好{top_count}个，下排恰好{bottom_count}个，按从左到右、从上到下的顺序排列。{}",
                if is_building_derivation { "建筑按功能顺序排列，不按升级等级排列。" } else { "" }
            )
        } else if is_building_derivation {
            format!("将全部{step_count}座建筑按功能顺序排列在同一行，不按升级等级排列。")
        } else {
            format!("将全部{step_count}个对象按演变顺序排列在同一行。")
        };
        format!(
            "强制画面规范：必须使用单一纯色背景，{background_rule}画面中不得出现编号、序号、文字标签、标题、说明文字或水印。任意两个主体之间必须保留清晰、连续的纯色背景间距，主体的轮廓、服装、武器、装备、特效和阴影均不得互相接触、重叠或连接。空间不足时必须统一缩小所有主体在画面中的占比，保持所选画布比例及正常2K或4K输出尺寸不变，宁可增加留白也不得压缩间距，确保每个主体都能被单独完整抠图。{layout} 数量硬约束：总共恰好{step_count}个完整主体，每个预定位置必须有且仅有一个独立主体，不得留空位。参考图不作为额外主体加入结果。阶段示例不能限制所选数量；不足时补充有明显差异的中间阶段，不得合并或省略主体。空间不足时缩小主体，不能减少数量。输出前逐排检查并核对总数，画面可见主体总数必须等于{step_count}。这些计数要求只用于规划，禁止在图片上画出计数文字或编号。"
        )
    };
    let label = if english {
        "User description: "
    } else {
        "用户描述："
    };
    if workflow_visual_rules.is_empty() {
        format!("{template}\n\n{composition_rules}\n\n{label}{user_description}")
    } else {
        format!(
            "{template}\n\n{workflow_visual_rules}\n\n{composition_rules}\n\n{label}{user_description}"
        )
    }
}

pub(super) fn normalize_canvas_workspace_prompts(
    workspaces: &mut BTreeMap<String, CanvasWorkspaceData>,
) -> bool {
    let mut changed = false;
    for workspace in workspaces.values_mut() {
        let normalized = normalize_canvas_workflow_prompt(&workspace.prompt);
        if normalized != workspace.prompt {
            workspace.prompt = normalized;
            changed = true;
        }
    }
    changed
}

pub(super) fn switch_canvas_workspace(
    store: &mut Store,
    current_prompt: &str,
    target_workspace_id: &str,
) -> String {
    let current_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    store.canvas_workspaces.insert(
        current_workspace_id,
        CanvasWorkspaceData {
            notes: store.canvas_notes.clone(),
            links: store.canvas_links.clone(),
            prompt: normalize_canvas_workflow_prompt(current_prompt),
            references: store.canvas_references.clone(),
        },
    );

    let target_workspace_id = normalize_canvas_workspace_id(target_workspace_id);
    let mut target = store
        .canvas_workspaces
        .get(&target_workspace_id)
        .cloned()
        .unwrap_or_default();
    target.prompt = normalize_canvas_workflow_prompt(&target.prompt);
    store.active_canvas_workspace_id = target_workspace_id.clone();
    store.canvas_notes = target.notes;
    store.canvas_links = target.links;
    store.canvas_references = target.references;
    clear_selection(&mut store.canvas_notes);
    store.canvas_workspaces.insert(
        target_workspace_id,
        CanvasWorkspaceData {
            notes: store.canvas_notes.clone(),
            links: store.canvas_links.clone(),
            prompt: target.prompt.clone(),
            references: store.canvas_references.clone(),
        },
    );
    target.prompt
}

pub(super) fn wire_infinite_canvas_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();
    let history = context.canvas_history.clone();
    let image_import_epoch = Rc::new(Cell::new(0_u64));

    state.on_normalize_canvas_workflow_prompt(|prompt| {
        normalize_canvas_workflow_prompt(prompt.as_str()).into()
    });
    state.on_compose_canvas_workflow_prompt(|template, prompt, step_count, english| {
        compose_canvas_workflow_prompt(
            template.as_str(),
            prompt.as_str(),
            step_count,
            english,
        )
        .into()
    });

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_open_canvas_workspace(move |workspace_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let edit = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let state = app.global::<AppState>();
                let prompt = switch_canvas_workspace(
                    store_mut,
                    state.get_canvas_workflow_prompt().as_str(),
                    workspace_id.as_str(),
                );
                migrate_legacy_auto_sized_canvas_images(&mut store_mut.canvas_notes);
                *history.borrow_mut() = CanvasController::default();
                state.set_canvas_workflow_prompt(prompt.into());
                let reference_effects =
                    prepare_canvas_reference_projection(&app, store_mut).publish_metadata(&app);
                state.set_canvas_selected_id("".into()); state.set_canvas_selected_link_id("".into());
                state.set_canvas_selected_count(0); state.set_canvas_node_info_open(false);
                state.set_canvas_group_name_dialog_open(false); state.set_canvas_can_undo(false);
                state.set_canvas_can_redo(false);
                state.set_canvas_workspace_switch_request(state.get_canvas_workspace_switch_request().saturating_add(1));
                Some(reference_effects)
            });
            if let Some(edit) = edit {
                let effects = finish_canvas_edit(&app, edit);
                start_canvas_reference_preview_effects(
                    &app, capture.persistence.clone(), effects,
                );
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_generate_canvas_node(move |source_node_id, prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&context.store) else { return; };
            let source_is_current = capture.apply(&context.store, || context.store.borrow()
                .canvas_notes.iter().any(|note| note.id == source_node_id.as_str()))
                .unwrap_or(false);
            if !source_is_current { return; }
            start_canvas_generation(
                &app,
                context.clone(),
                source_node_id.to_string(),
                prompt.to_string(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_extract_canvas_ui_elements(move |source_node_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let source = capture.apply(&store, || {
                let state = app.global::<AppState>();
                if !state.get_canvas_extraction_loading_node_id().is_empty()
                    || !state.get_canvas_split_loading_node_id().is_empty() {
                    state.set_generation_status(
                        (if state.get_language().as_str() == "en" { "Wait for the current canvas image operation to finish" }
                        else { "请等待当前画布图片操作完成" }).into(),
                    );
                    return None;
                }
                let store_ref = store.borrow();
                if store_ref.canvas_notes.len() + MAX_EXTRACTED_COMPONENTS > MAX_CANVAS_NODES
                    || store_ref.canvas_links.len() + MAX_EXTRACTED_COMPONENTS > MAX_CANVAS_LINKS
                {
                    show_canvas_capacity_status(&app);
                    return None;
                }
                let Some(note) = store_ref.canvas_notes.iter().find(|note| {
                    note.id == source_node_id.as_str()
                        && matches!(note.kind.as_str(), "image" | "board-image")
                        && !note.image_path.trim().is_empty()
                }) else {
                    state.set_generation_status(
                        if state.get_language().as_str() == "en" {
                            "Select an uploaded canvas image before extracting elements"
                        } else {
                            "请先选择已上传的画布图片"
                        }
                        .into(),
                    );
                    return None;
                };
                let source = CanvasSplitSource {
                    id: note.id.clone(),
                    image_path: note.image_path.clone(),
                    x: note.x,
                    y: note.y,
                    width: note.width,
                    height: note.height,
                };
                state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Extracting transparent PNG elements from the current image..."
                } else {
                    "正在从当前图片提取透明 PNG 元素..."
                }
                .into(),
                );
                state.set_canvas_extraction_loading_node_id(source.id.clone().into());
                Some(source)
            }).flatten();
            let Some(source) = source else { return; };
            let source_path = PathBuf::from(&source.image_path);
            let (sender, receiver) = mpsc::channel::<CanvasExtractionOutcome>();
            let persistence = capture.persistence.clone();
            let worker_persistence = persistence.clone();
            let ticket = match spawn_canvas_worker(persistence, move |cancel, _activity| {
                let outcome = worker_persistence.storage_authority()
                    .and_then(|authority| extract_canvas_elements_for_authority(&authority, &source_path, &cancel))
                    .map_err(|error| error.to_string());
                let _ = sender.send(outcome);
                wait_at_canvas_worker_exit_for_test();
            }) {
                Ok(ticket) => ticket,
                Err(_error) => {
                    let _ = capture.apply(&store, || {
                        let state = app.global::<AppState>();
                        clear_canvas_extraction_loading(&state, &source.id);
                        state.set_generation_status(
                            (if state.get_language().as_str() == "en" { "Unable to start canvas extraction" }
                            else { "无法启动画布元素提取" }).into(),
                        );
                    });
                    return;
                }
            };
            poll_canvas_element_extraction(
                app.as_weak(),
                store.clone(),
                history.clone(),
                capture,
                source,
                Rc::new(RefCell::new(Some(receiver))),
                Rc::new(RefCell::new(Some(ticket))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_canvas_split_positions(|count| {
            let count = count.clamp(0, 64);
            ModelRc::new(VecModel::from((1..=count).map(|index| index as f32 / (count + 1) as f32).collect::<Vec<_>>()))
        });
        state.on_split_canvas_image(move |source_node_id, rows, columns, row_positions, column_positions| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let horizontal_lines = rows.trim().parse::<u32>();
            let vertical_lines = columns.trim().parse::<u32>();
            let parsed = match (horizontal_lines, vertical_lines) {
                (Ok(horizontal), Ok(vertical)) => Some((horizontal, vertical)),
                _ => None,
            };
            let Some((horizontal_lines, vertical_lines)) = parsed else {
                let _ = capture.apply(&store, || {
                    let state = app.global::<AppState>();
                    state.set_generation_status(
                        (if state.get_language().as_str() == "en" { "Enter non-negative whole numbers for split lines" }
                        else { "横向和纵向分割线数量请输入非负整数" }).into(),
                    );
                });
                return;
            };
            if (horizontal_lines == 0 && vertical_lines == 0)
                || horizontal_lines > MAX_CANVAS_SPLIT_AXIS
                || vertical_lines > MAX_CANVAS_SPLIT_AXIS
            {
                let _ = capture.apply(&store, || {
                    let state = app.global::<AppState>();
                    state.set_generation_status(
                        (if state.get_language().as_str() == "en" { "Use 0 to 64 lines per axis and add at least one split line" }
                        else { "每个方向可设置 0 到 64 条线，请至少添加一条分割线" }).into(),
                    );
                });
                return;
            }
            let row_positions: Vec<f32> = row_positions.iter().collect();
            let column_positions: Vec<f32> = column_positions.iter().collect();
            if row_positions.len() != horizontal_lines as usize || column_positions.len() != vertical_lines as usize {
                let _ = capture.apply(&store, || app.global::<AppState>().set_generation_status(
                    "分割线位置与数量不一致，请重试 / Split line positions do not match the count".into()));
                return;
            }
            let Some(rows) = split_parts_from_lines(horizontal_lines) else {
                let _ = capture.apply(&store, || show_canvas_capacity_status(&app));
                return;
            };
            let Some(columns) = split_parts_from_lines(vertical_lines) else {
                let _ = capture.apply(&store, || show_canvas_capacity_status(&app));
                return;
            };
            let tile_count = match rows.checked_mul(columns) {
                Some(count) => count as usize,
                None => {
                    let _ = capture.apply(&store, || show_canvas_capacity_status(&app));
                    return;
                }
            };
            let source = capture.apply(&store, || {
                let state = app.global::<AppState>();
                if !state.get_canvas_split_loading_node_id().is_empty()
                    || !state.get_canvas_extraction_loading_node_id().is_empty() {
                    state.set_generation_status(
                        (if state.get_language().as_str() == "en" { "Wait for the current image split to finish" }
                        else { "请等待当前图片分割完成" }).into(),
                    );
                    return None;
                }
                let store_ref = store.borrow();
                if store_ref.canvas_notes.len() + tile_count > MAX_CANVAS_NODES
                    || store_ref.canvas_links.len() + tile_count > MAX_CANVAS_LINKS
                {
                    show_canvas_capacity_status(&app);
                    return None;
                }
                let Some(note) = store_ref.canvas_notes.iter().find(|note| {
                    note.id == source_node_id.as_str()
                        && matches!(note.kind.as_str(), "image" | "board-image")
                        && !note.image_path.trim().is_empty()
                }) else {
                    state.set_generation_status(
                        if state.get_language().as_str() == "en" {
                            "Select an uploaded image before splitting"
                        } else {
                            "请先选择已上传图片的节点"
                        }
                        .into(),
                    );
                    return None;
                };
                let source = CanvasSplitSource {
                    id: note.id.clone(),
                    image_path: note.image_path.clone(),
                    x: note.x,
                    y: note.y,
                    width: note.width,
                    height: note.height,
                };
                state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Splitting image locally..."
                } else {
                    "正在按分割线位置裁切图片..."
                }
                .into(),
                );
                state.set_canvas_split_loading_node_id(source.id.clone().into());
                Some(source)
            }).flatten();
            let Some(source) = source else { return; };
            let source_path = PathBuf::from(&source.image_path);
            let (sender, receiver) = mpsc::channel::<CanvasSplitOutcome>();
            let persistence = capture.persistence.clone();
            let worker_persistence = persistence.clone();
            let ticket = match spawn_canvas_worker(persistence, move |cancel, _activity| {
                let outcome = worker_persistence.storage_authority()
                    .and_then(|authority| split_canvas_image_for_authority(&authority, &source_path, &row_positions, &column_positions, &cancel))
                    .map_err(|error| error.to_string());
                let _ = sender.send(outcome);
                wait_at_canvas_worker_exit_for_test();
            }) {
                Ok(ticket) => ticket,
                Err(_error) => {
                    let _ = capture.apply(&store, || {
                        let state = app.global::<AppState>();
                        clear_canvas_split_loading(&state, &source.id);
                        state.set_generation_status(
                            (if state.get_language().as_str() == "en" { "Unable to start canvas image splitting" }
                            else { "无法启动画布图片分割" }).into(),
                        );
                    });
                    return;
                }
            };
            poll_canvas_image_split(
                app.as_weak(),
                store.clone(),
                history.clone(),
                capture,
                source,
                rows,
                columns,
                Rc::new(RefCell::new(Some(receiver))),
                Rc::new(RefCell::new(Some(ticket))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_save_canvas_image(move |node_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let source = {
                let store_ref = store.borrow();
                store_ref
                    .canvas_notes
                    .iter()
                    .find(|note| {
                        note.id == node_id.as_str()
                            && matches!(note.kind.as_str(), "image" | "board-image")
                            && !note.image_path.trim().is_empty()
                    })
                    .map(|note| PathBuf::from(&note.image_path))
            };
            let Some(source) = source.filter(|path| path.is_file()) else {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "The canvas image is no longer available"
                    } else {
                        "画布图片文件已不存在"
                    }
                    .into(),
                );
                return;
            };
            let default_name = source
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("canvas-image.png");
            let Some(destination) = choose_canvas_export_path(default_name)
            else {
                return;
            };
            let result = if destination == source {
                Ok(())
            } else {
                fs::read(&source).and_then(|bytes| {
                    atomic_write_file(&destination, &bytes)
                        .map_err(|error| std::io::Error::other(error.to_string()))
                })
            };
            state.set_generation_status(
                match result {
                    Ok(()) if state.get_language().as_str() == "en" => {
                        "Canvas image saved".to_string()
                    }
                    Ok(()) => "画布图片已保存到本地".to_string(),
                    Err(error) if state.get_language().as_str() == "en" => {
                        format!("Unable to save the canvas image: {error}")
                    }
                    Err(error) => format!("保存画布图片失败：{error}"),
                }
                .into(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_show_canvas_node_info(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let _ = capture.apply(&store, || {
                let store_ref = store.borrow();
                let Some(node) = store_ref.canvas_notes.iter().find(|node| node.id == id.as_str()) else { return; };
                let json = serde_json::to_string_pretty(&serde_json::json!({
                    "id": node.id, "type": node.kind, "content": node.content,
                    "width": node.width, "height": node.height, "x": node.x, "y": node.y,
                    "parent_group_id": node.parent_group_id, "z_index": node.z_index,
                    "image_path": node.image_path, "font_size": node.font_size, "status": "idle"
                })).unwrap_or_else(|_| "{}".to_string());
                let state = app.global::<AppState>();
                state.set_canvas_node_info_id(node.id.clone().into());
                state.set_canvas_node_info_kind(node.kind.clone().into());
                state.set_canvas_node_info_x(node.x); state.set_canvas_node_info_y(node.y);
                state.set_canvas_node_info_width(node.width); state.set_canvas_node_info_height(node.height);
                state.set_canvas_node_info_status("idle".into()); state.set_canvas_node_info_json(json.into());
                state.set_canvas_node_info_tab("info".into()); state.set_canvas_node_info_open(true);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        let image_import_epoch = image_import_epoch.clone();
        state.on_choose_canvas_node_image(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let original_path = store.borrow().canvas_notes.iter()
                .find(|node| node.id == id.as_str() && node.kind == "image")
                .map(|node| node.image_path.clone());
            let Some(original_path) = original_path else { return; };
            let Some(request_id) = next_canvas_image_import_request(&image_import_epoch) else { return; };
            let Some(source) = choose_canvas_image_for_capture(&capture) else { return; };
            let still_current = capture.apply(&store, || store.borrow().canvas_notes.iter().any(|node| {
                node.id == id.as_str() && node.kind == "image" && node.image_path == original_path
            })).unwrap_or(false);
            if !still_current { return; }
            if start_canvas_image_import(
                &app, store.clone(), history.clone(), capture.clone(),
                image_import_epoch.clone(), request_id,
                CanvasImageImportSource::Selected(source),
                CanvasImageImportTarget::Existing { id: id.to_string(), original_path },
            ).is_err() {
                let _ = capture.apply(&store, || {
                    let state = app.global::<AppState>();
                    state.set_generation_status(
                        (if state.get_language().as_str() == "en" { "Unable to start the canvas image import" }
                        else { "无法启动画布图片导入" }).into(),
                    );
                });
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        let image_import_epoch = image_import_epoch.clone();
        state.on_add_canvas_uploaded_image(move |center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            match capture.apply(&store, || store.borrow().canvas_notes.len() < MAX_CANVAS_NODES) {
                Some(true) => {}
                Some(false) => { let _ = capture.apply(&store, || show_canvas_capacity_status(&app)); return; }
                None => return,
            }
            let id = Uuid::new_v4().to_string();
            let Some(request_id) = next_canvas_image_import_request(&image_import_epoch) else { return; };
            let Some(source) = choose_canvas_image_for_capture(&capture) else { return; };
            match capture.apply(&store, || store.borrow().canvas_notes.len() < MAX_CANVAS_NODES) {
                Some(true) => {}
                Some(false) => { let _ = capture.apply(&store, || show_canvas_capacity_status(&app)); return; }
                None => return,
            }
            if start_canvas_image_import(
                &app, store.clone(), history.clone(), capture.clone(),
                image_import_epoch.clone(), request_id,
                CanvasImageImportSource::Selected(source),
                CanvasImageImportTarget::New { id, center_x, center_y },
            ).is_err() {
                let _ = capture.apply(&store, || {
                    let state = app.global::<AppState>();
                    state.set_generation_status(
                        (if state.get_language().as_str() == "en" { "Unable to start the canvas image import" }
                        else { "无法启动画布图片导入" }).into(),
                    );
                });
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_create_canvas_generation_source(move |prompt, center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return "".into();
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return "".into(); };
            let prompt = prompt.trim().to_string();
            if prompt.is_empty() {
                return "".into();
            }

            let edit = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let state = app.global::<AppState>();
                if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES {
                    show_canvas_capacity_status(&app);
                    return None;
                }
                let (_, width, height) =
                    canvas_node_defaults("image", state.get_language().as_str() == "en");
                let (x, y) = nearest_free_canvas_position(
                    &store_mut.canvas_notes, center_x - width / 2.0,
                    center_y - height / 2.0, width, height, None,
                );
                let id = Uuid::new_v4().to_string();
                history.borrow_mut().record(canvas_snapshot(store_mut));
                clear_selection(&mut store_mut.canvas_notes);
                store_mut.canvas_notes.push(CanvasNoteData {
                    id: id.clone(), kind: "image".to_string(), content: prompt,
                    x, y, width, height, selected: true, ..CanvasNoteData::default()
                });
                sync_canvas_selection(&app, store_mut);
                state.set_canvas_selected_id(id.clone().into());
                sync_history_state(&app, &history.borrow());
                Some(id)
            });
            edit.map(|edit| finish_canvas_edit(&app, edit).into()).unwrap_or_default()
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_add_canvas_node(move |kind, center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let state = app.global::<AppState>();
                if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES {
                    show_canvas_capacity_status(&app);
                    return None;
                }
                let node_kind = match kind.as_str() {
                    "image" | "group" => kind.to_string(),
                    _ => "text".to_string(),
                };
                let (mut content, width, height) =
                    canvas_node_defaults(&node_kind, state.get_language().as_str() == "en");
                if node_kind == "group" {
                    content = next_group_name(&store_mut.canvas_notes, state.get_language().as_str() == "en");
                }
                let id = Uuid::new_v4().to_string();
                history.borrow_mut().record(canvas_snapshot(store_mut));
                clear_selection(&mut store_mut.canvas_notes);
                store_mut.canvas_notes.push(CanvasNoteData {
                    id: id.clone(), kind: node_kind, content,
                    x: center_x - width / 2.0, y: center_y - height / 2.0,
                    width, height, selected: true, ..CanvasNoteData::default()
                });
                sync_canvas_selection(&app, store_mut);
                state.set_canvas_selected_id(id.into());
                sync_history_state(&app, &history.borrow());
                Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_adjust_canvas_text_font_size(move |id, delta| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let index = store_mut.canvas_notes.iter()
                    .position(|node| node.id == id.as_str() && node.kind == "text")?;
                let next = (store_mut.canvas_notes[index].font_size + delta).clamp(8.0, 72.0);
                if next == store_mut.canvas_notes[index].font_size { return None; }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                store_mut.canvas_notes[index].font_size = next;
                sync_history_state(&app, &history.borrow());
                Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_rename_canvas_group(move |id, name| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return false; };
            let name = name.trim();
            if name.is_empty() {
                let _ = capture.apply(&store, || app.global::<AppState>()
                    .set_generation_status("分组名称不能为空".into()));
                return false;
            }
            let edit = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let index = store_mut.canvas_notes.iter()
                    .position(|note| note.id == id.as_str() && note.kind == "group")?;
                if store_mut.canvas_notes[index].content == name { return Some(true); }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                store_mut.canvas_notes[index].content = name.to_string();
                sync_history_state(&app, &history.borrow());
                Some(true)
            });
            edit.map(|edit| finish_canvas_edit(&app, edit)).unwrap_or(false)
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_resize_canvas_group(move |id, width, height| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let before = canvas_snapshot(store_mut);
                if !resize_group(&mut store_mut.canvas_notes, id.as_str(), width.max(1.0), height.max(1.0)) { return None; }
                history.borrow_mut().record(before);
                sync_canvas_selection(&app, store_mut);
                sync_history_state(&app, &history.borrow());
                Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_resize_canvas_image_node(move |id, width, height| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let before = canvas_snapshot(store_mut);
                if !resize_image_node_proportionally(&mut store_mut.canvas_notes, id.as_str(), width.max(1.0), height.max(1.0)) { return None; }
                history.borrow_mut().record(before);
                sync_canvas_selection(&app, store_mut);
                sync_history_state(&app, &history.borrow());
                Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_prepare_canvas_focus(move |viewport_width, viewport_height| {
            let Some(app) = app_weak.upgrade() else {
                return 100;
            };
            apply_canvas_ui(&store, |store_ref| {
                let mut ids = selected_ids(&store_ref.canvas_notes);
                if ids.is_empty() { ids.extend(store_ref.canvas_notes.iter().map(|note| note.id.clone())); }
                let Some(bounds) = selection_bounds(&store_ref.canvas_notes, &ids) else { return 100; };
                let state = app.global::<AppState>();
                state.set_canvas_focus_x(bounds.x); state.set_canvas_focus_y(bounds.y);
                state.set_canvas_focus_width(bounds.width); state.set_canvas_focus_height(bounds.height);
                let safe_width = bounds.width.max(1.0); let safe_height = bounds.height.max(1.0);
                ((viewport_width.max(1.0) / safe_width).min(viewport_height.max(1.0) / safe_height) * 84.0)
                    .clamp(5.0, 500.0).round() as i32
            }).unwrap_or(100)
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_update_canvas_node(move |id, content, x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let index = store_mut.canvas_notes.iter().position(|note| note.id == id.as_str())?;
                let content = content.to_string();
                if store_mut.canvas_notes[index].content == content
                    && store_mut.canvas_notes[index].x == x && store_mut.canvas_notes[index].y == y { return None; }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                let node = &mut store_mut.canvas_notes[index];
                node.content = content; node.x = x; node.y = y;
                sync_history_state(&app, &history.borrow());
                Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_canvas_node(move |id, toggle| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let _ = capture.apply(&store, || {
                let mut store_mut = store.borrow_mut();
                select_node(&mut store_mut.canvas_notes, id.as_str(), toggle);
                let selected = store_mut.canvas_notes.iter().find(|note| note.id == id.as_str())
                    .is_some_and(|note| note.selected);
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(if selected { id } else { "".into() });
                state.set_canvas_selected_link_id("".into());
                sync_canvas_selection_rows(&app, &store_mut);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_canvas_rect(move |x1, y1, x2, y2, additive| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let _ = capture.apply(&store, || {
                let mut store_mut = store.borrow_mut();
                select_in_rect(&mut store_mut.canvas_notes, CanvasRect::normalized(x1, y1, x2, y2), additive);
                let primary = store_mut.canvas_notes.iter().find(|note| note.selected)
                    .map(|note| note.id.clone()).unwrap_or_default();
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(primary.into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, &store_mut);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_clear_canvas_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let _ = capture.apply(&store, || {
                let mut store_mut = store.borrow_mut();
                clear_selection(&mut store_mut.canvas_notes);
                let state = app.global::<AppState>();
                state.set_canvas_selected_id("".into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, &store_mut);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_all_canvas_nodes(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let _ = capture.apply(&store, || {
                let mut store_mut = store.borrow_mut();
                for note in &mut store_mut.canvas_notes { note.selected = true; }
                let primary = store_mut.canvas_notes.first().map(|note| note.id.clone()).unwrap_or_default();
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(primary.into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, &store_mut);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_move_canvas_selection(move |dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if dx == 0.0 && dy == 0.0 {
                return;
            }
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                if expanded_selection_ids(&store_mut.canvas_notes).is_empty() { return None; }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                move_selection(&mut store_mut.canvas_notes, dx, dy);
                fit_groups_to_children(&mut store_mut.canvas_notes);
                sync_canvas_selection(&app, store_mut);
                sync_history_state(&app, &history.borrow());
                Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let store = store.clone();
        let history = history.clone();
        state.on_copy_canvas_selection(move || {
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let Some(effect) = capture.begin_effect(&store) else { return; };
            let fingerprint = read_canvas_system_clipboard().map(|content| content.fingerprint());
            drop(effect);
            let _ = capture.apply(&store, || {
                let store_ref = store.borrow();
                let mut controller = history.borrow_mut();
                controller.copy_selection(&store_ref.canvas_notes, &store_ref.canvas_links);
                controller.remember_system_clipboard(fingerprint);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_paste_canvas_selection(move |offset_x, offset_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let (notes, links) = history.borrow().paste_clipboard(offset_x.max(0.0), offset_y.max(0.0));
                if notes.is_empty() { return None; }
                if store_mut.canvas_notes.len() + notes.len() > MAX_CANVAS_NODES
                    || store_mut.canvas_links.len() + links.len() > MAX_CANVAS_LINKS {
                    show_canvas_capacity_status(&app); return None;
                }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                clear_selection(&mut store_mut.canvas_notes);
                let primary = notes.first().map(|note| note.id.clone()).unwrap_or_default();
                store_mut.canvas_notes.extend(notes); store_mut.canvas_links.extend(links);
                fit_groups_to_children(&mut store_mut.canvas_notes);
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(primary.into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, store_mut); sync_history_state(&app, &history.borrow());
                Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        let image_import_epoch = image_import_epoch.clone();
        state.on_paste_canvas_content(move |center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let Some(effect) = capture.begin_effect(&store) else { return; };
            let system_clipboard = read_canvas_system_clipboard();
            let system_fingerprint = system_clipboard
                .as_ref()
                .map(CanvasSystemClipboard::fingerprint);
            let paste_system_clipboard = capture.apply(&store, || history.borrow()
                .should_paste_system_clipboard(system_fingerprint)).unwrap_or(true);
            if !paste_system_clipboard {
                drop(effect);
                app.global::<AppState>().invoke_paste_canvas_selection(24.0, 24.0);
                return;
            }
            let Some(system_clipboard) = system_clipboard else {
                return;
            };
            let capacity = capture.apply(&store, || store.borrow().canvas_notes.len() < MAX_CANVAS_NODES);
            if capacity != Some(true) {
                if capacity == Some(false) { let _ = capture.apply(&store, || show_canvas_capacity_status(&app)); }
                return;
            }

            let id = Uuid::new_v4().to_string();
            let state = app.global::<AppState>();
            let Some(request_id) = next_canvas_image_import_request(&image_import_epoch) else { return; };
            let note = match system_clipboard {
                CanvasSystemClipboard::Image {
                    width,
                    height,
                    bytes,
                    ..
                } => {
                    let result = start_canvas_image_import(
                        &app, store.clone(), history.clone(), capture.clone(),
                        image_import_epoch.clone(), request_id,
                        CanvasImageImportSource::Clipboard { width, height, bytes },
                        CanvasImageImportTarget::New { id, center_x, center_y },
                    );
                    if result.is_err() {
                        let _ = capture.apply(&store, || state.set_generation_status(
                            (if state.get_language().as_str() == "en" { "Unable to start the canvas paste" }
                            else { "无法启动画布粘贴" }).into()));
                    }
                    return;
                }
                CanvasSystemClipboard::Text { text, .. } => {
                    let (_, width, height) =
                        canvas_node_defaults("text", state.get_language().as_str() == "en");
                    CanvasNoteData {
                        id: id.clone(),
                        kind: "text".into(),
                        content: text,
                        x: center_x - width / 2.0,
                        y: center_y - height / 2.0,
                        width,
                        height,
                        selected: true,
                        ..CanvasNoteData::default()
                    }
                }
            };

            drop(effect);
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES {
                    show_canvas_capacity_status(&app); return None;
                }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                clear_selection(&mut store_mut.canvas_notes); store_mut.canvas_notes.push(note);
                sync_canvas_selection(&app, store_mut);
                state.set_canvas_selected_id(id.into()); state.set_canvas_selected_link_id("".into());
                sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_duplicate_canvas_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let (notes, links) = {
                    let mut controller = history.borrow_mut();
                    controller.copy_selection(&store_mut.canvas_notes, &store_mut.canvas_links);
                    controller.paste_clipboard(24.0, 24.0)
                };
                if notes.is_empty() { return None; }
                if store_mut.canvas_notes.len() + notes.len() > MAX_CANVAS_NODES
                    || store_mut.canvas_links.len() + links.len() > MAX_CANVAS_LINKS {
                    show_canvas_capacity_status(&app); return None;
                }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                clear_selection(&mut store_mut.canvas_notes);
                let primary = notes.first().map(|note| note.id.clone()).unwrap_or_default();
                store_mut.canvas_notes.extend(notes); store_mut.canvas_links.extend(links);
                fit_groups_to_children(&mut store_mut.canvas_notes);
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(primary.into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, store_mut); sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_remove_canvas_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                if selected_ids(&store_mut.canvas_notes).is_empty() { return None; }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                let mut links = std::mem::take(&mut store_mut.canvas_links);
                remove_selection(&mut store_mut.canvas_notes, &mut links); store_mut.canvas_links = links;
                fit_groups_to_children(&mut store_mut.canvas_notes);
                let state = app.global::<AppState>();
                state.set_canvas_selected_id("".into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, store_mut); sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_group_canvas_selection(move |center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES { show_canvas_capacity_status(&app); return None; }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                let english = app.global::<AppState>().get_language().as_str() == "en";
                let id = if let Some(id) = group_selection(&mut store_mut.canvas_notes, english) { id } else {
                let (_, width, height) = canvas_node_defaults("group", english);
                let content = next_group_name(&store_mut.canvas_notes, english);
                clear_selection(&mut store_mut.canvas_notes);
                let id = Uuid::new_v4().to_string();
                store_mut.canvas_notes.push(CanvasNoteData {
                    id: id.clone(),
                    kind: "group".into(),
                    content,
                    x: center_x - width / 2.0,
                    y: center_y - height / 2.0,
                    width,
                    height,
                    selected: true,
                    ..CanvasNoteData::default()
                });
                id
                };
                fit_groups_to_children(&mut store_mut.canvas_notes);
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(id.into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, store_mut); sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_ungroup_canvas_node(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let before = canvas_snapshot(store_mut);
                if !ungroup_node(&mut store_mut.canvas_notes, id.as_str()) { return None; }
                history.borrow_mut().record(before); fit_groups_to_children(&mut store_mut.canvas_notes);
                let primary = store_mut.canvas_notes.iter().find(|note| note.selected)
                    .map(|note| note.id.clone()).unwrap_or_default();
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(primary.into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, store_mut); sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_ungroup_canvas_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                if !store_mut.canvas_notes.iter().any(|note| note.selected && note.kind == "group") { return None; }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                ungroup_selection(&mut store_mut.canvas_notes); fit_groups_to_children(&mut store_mut.canvas_notes);
                let primary = store_mut.canvas_notes.iter().find(|note| note.selected)
                    .map(|note| note.id.clone()).unwrap_or_default();
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(primary.into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, store_mut); sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_remove_canvas_group_with_children(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let before = canvas_snapshot(store_mut); let mut links = std::mem::take(&mut store_mut.canvas_links);
                let removed = remove_group_with_descendants(&mut store_mut.canvas_notes, &mut links, id.as_str());
                store_mut.canvas_links = links; if removed.is_empty() { return None; }
                history.borrow_mut().record(before); fit_groups_to_children(&mut store_mut.canvas_notes);
                let primary = store_mut.canvas_notes.iter().find(|note| note.selected)
                    .map(|note| note.id.clone()).unwrap_or_default();
                let state = app.global::<AppState>();
                state.set_canvas_selected_id(primary.into()); state.set_canvas_selected_link_id("".into());
                sync_canvas_selection(&app, store_mut); sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_remove_canvas_node(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
            if !store_mut.canvas_notes.iter().any(|note| note.id == id.as_str()) { return None; }
            history.borrow_mut().record(canvas_snapshot(store_mut));
            let removed_parent = store_mut.canvas_notes.iter()
                .find(|note| note.id == id.as_str() && note.kind == "group")
                .map(|note| note.parent_group_id.clone());
            if let Some(parent_id) = removed_parent {
                for child in store_mut
                    .canvas_notes
                    .iter_mut()
                    .filter(|note| note.parent_group_id == id.as_str())
                {
                    child.parent_group_id = parent_id.clone();
                }
            }
            store_mut.canvas_notes.retain(|note| note.id != id.as_str());
            fit_groups_to_children(&mut store_mut.canvas_notes);
            store_mut
                .canvas_links
                .retain(|link| link.source_id != id.as_str() && link.target_id != id.as_str());
            let state = app.global::<AppState>();
            if state.get_canvas_selected_id().as_str() == id.as_str() {
                state.set_canvas_selected_id("".into());
            }
            state.set_canvas_selected_link_id("".into());
            sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_search_canvas_node_types(move |query| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let query = query.trim().to_lowercase();
            let options = [
                ("text", ["text", "文本", "prompt", "提示词"]),
                ("image", ["image", "图片", "picture", "图像"]),
            ];
            let results = options
                .into_iter()
                .filter(|(_, keywords)| {
                    query.is_empty()
                        || keywords
                            .iter()
                            .any(|keyword| keyword.to_lowercase().contains(&query))
                })
                .map(|(kind, _)| SharedString::from(kind))
                .collect::<Vec<_>>();
            if let Some(capture) = CanvasActionCapture::capture(&store) {
                let _ = capture.apply(&store, || app.global::<AppState>()
                    .set_canvas_node_search_results(ModelRc::new(VecModel::from(results))));
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_add_connected_canvas_node(move |kind, source_id, x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES
                || store_mut.canvas_links.len() >= MAX_CANVAS_LINKS
                || !store_mut
                    .canvas_notes
                    .iter()
                    .any(|note| note.id == source_id.as_str() && note.kind != "group")
            {
                show_canvas_capacity_status(&app);
                return None;
            }
            let node_kind = if kind.as_str() == "image" {
                "image".to_string()
            } else {
                "text".to_string()
            };
            let state = app.global::<AppState>();
            let (content, width, height) =
                canvas_node_defaults(&node_kind, state.get_language().as_str() == "en");
            let id = Uuid::new_v4().to_string();
            let before = canvas_snapshot(&store_mut);
            clear_selection(&mut store_mut.canvas_notes);
            store_mut.canvas_notes.push(CanvasNoteData {
                id: id.clone(),
                kind: node_kind,
                content,
                x,
                y,
                width,
                height,
                selected: true,
                ..CanvasNoteData::default()
            });
            let CanvasConnectResult::Connected { link_id, .. } =
                connect_nodes(&mut store_mut.canvas_links, source_id.as_str(), &id)
            else {
                store_mut.canvas_notes.pop();
                return None;
            };
            history.borrow_mut().record(before);
            sync_canvas_selection(&app, &store_mut);
            state.set_canvas_selected_id(id.into());
            state.set_canvas_selected_link_id(link_id.into());
            sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_preview_canvas_link_target(move |source_id, x, y, tolerance| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            let _ = capture.apply(&store, || {
                let store_ref = store.borrow();
                let target_id = target_at_input(&store_ref, source_id.as_str(), x, y, tolerance.max(8.0)).unwrap_or_default();
                let valid = !target_id.is_empty() && connection_allowed(&store_ref.canvas_links, source_id.as_str(), target_id.as_str());
                let state = app.global::<AppState>();
                state.set_canvas_link_hover_target_id(target_id.into()); state.set_canvas_link_hover_valid(valid);
            });
        });
    }

    {
        let store = store.clone();
        state.on_canvas_input_link(move |target_id| {
            apply_canvas_ui(&store, |store| store.canvas_links.iter()
                .find(|link| link.target_id == target_id.as_str())
                .map(|link| link.id.clone()).unwrap_or_default().into()).unwrap_or_default()
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_finish_canvas_link(move |source_id, x, y, tolerance| {
            let Some(app) = app_weak.upgrade() else {
                return "rejected".into();
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return "rejected".into(); };
            let edit = apply_canvas_edit(&app, &store, &capture, |store_mut| {
            if !store_mut.canvas_notes.iter().any(|note| note.id == source_id.as_str() && note.kind != "group") {
                return Some("rejected".to_string());
            }
            let Some(target_id) = target_at_input(store_mut, source_id.as_str(), x, y, tolerance.max(8.0))
            else { return Some("empty".to_string()); };
            let replacing = store_mut
                .canvas_links
                .iter()
                .any(|link| link.target_id == target_id);
            if !replacing && store_mut.canvas_links.len() >= MAX_CANVAS_LINKS {
                show_canvas_capacity_status(&app);
                return Some("rejected".to_string());
            }

            let before = canvas_snapshot(&store_mut);
            let CanvasConnectResult::Connected {
                link_id, target_id, ..
            } = connect_nodes(
                &mut store_mut.canvas_links,
                source_id.as_str(),
                target_id.as_str(),
            )
            else {
                return Some("rejected".to_string());
            };
            history.borrow_mut().record(before);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(target_id.into());
            state.set_canvas_selected_link_id(link_id.into());
            state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Connected. Upstream content will be used during generation."
                } else {
                    "连接成功，生成时将自动使用上游节点内容。"
                }
                .into(),
            );
            sync_history_state(&app, &history.borrow());
            Some("connected".to_string())
            });
            edit.map(|edit| SharedString::from(finish_canvas_edit(&app, edit)))
                .unwrap_or_else(|| "rejected".into())
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_finish_canvas_reconnect(move |target_id, x, y, tolerance| {
            let Some(app) = app_weak.upgrade() else {
                return "rejected".into();
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return "rejected".into(); };
            let edit = apply_canvas_edit(&app, &store, &capture, |store_mut| {
            let Some(source_id) =
                source_at_output(store_mut, target_id.as_str(), x, y, tolerance.max(8.0))
            else {
                return Some("rejected".to_string());
            };
            let before = canvas_snapshot(&store_mut);
            let CanvasConnectResult::Connected {
                link_id, target_id, ..
            } = connect_nodes_with_flow(
                &mut store_mut.canvas_links,
                source_id.as_str(),
                target_id.as_str(),
                true,
            )
            else {
                return Some("rejected".to_string());
            };
            history.borrow_mut().record(before);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(target_id.into());
            state.set_canvas_selected_link_id(link_id.into());
            sync_history_state(&app, &history.borrow());
            Some("connected".to_string())
            });
            edit.map(|edit| SharedString::from(finish_canvas_edit(&app, edit)))
                .unwrap_or_else(|| "rejected".into())
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_remove_canvas_link(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                if !store_mut.canvas_links.iter().any(|link| link.id == id.as_str()) { return None; }
                history.borrow_mut().record(canvas_snapshot(store_mut));
                store_mut.canvas_links.retain(|link| link.id != id.as_str());
                let state = app.global::<AppState>();
                if state.get_canvas_selected_link_id().as_str() == id.as_str() { state.set_canvas_selected_link_id("".into()); }
                sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_undo_canvas(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let previous = history.borrow_mut().undo(canvas_snapshot(store_mut))?;
                restore_canvas_snapshot(store_mut, previous);
                app.global::<AppState>().set_canvas_selected_id("".into());
                app.global::<AppState>().set_canvas_selected_link_id("".into());
                sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_redo_canvas(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = CanvasActionCapture::capture(&store) else { return; };
            if let Some(edit) = apply_canvas_edit(&app, &store, &capture, |store_mut| {
                let next = history.borrow_mut().redo(canvas_snapshot(store_mut))?;
                restore_canvas_snapshot(store_mut, next);
                app.global::<AppState>().set_canvas_selected_id("".into());
                app.global::<AppState>().set_canvas_selected_link_id("".into());
                sync_history_state(&app, &history.borrow()); Some(())
            }) { finish_canvas_edit(&app, edit); }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas_fixture_serial() -> &'static Mutex<()> {
        static SERIAL: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        SERIAL.get_or_init(|| Mutex::new(()))
    }

    struct CanvasTestSeams {
        _serial: std::sync::MutexGuard<'static, ()>,
        worker_release: Option<Arc<std::sync::atomic::AtomicBool>>,
        retirements: Vec<(UserActivityGate, NamespaceLease)>,
    }

    impl CanvasTestSeams {
        fn for_fixtures(
            data_root: &Path,
            fixtures: &[&video_image_callbacks::tests::scoped_inputs::Fixture],
        ) -> Self {
            let serial = canvas_fixture_serial().lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            CANVAS_DATA_ROOT_FIXTURE.with(|fixture| {
                *fixture.borrow_mut() = Some(data_root.to_path_buf());
            });
            Self {
                _serial: serial,
                worker_release: None,
                retirements: fixtures.iter().map(|fixture| (
                    fixture.context.user_activity.clone(), fixture.persistence.lease().clone(),
                )).collect(),
            }
        }

        fn with_worker_release(
            fixture: &video_image_callbacks::tests::scoped_inputs::Fixture,
            data_root: &Path,
            release: Arc<std::sync::atomic::AtomicBool>,
        ) -> Self {
            Self::with_worker_release_for_fixtures(data_root, &[fixture], release)
        }

        fn with_worker_release_for_fixtures(
            data_root: &Path,
            fixtures: &[&video_image_callbacks::tests::scoped_inputs::Fixture],
            release: Arc<std::sync::atomic::AtomicBool>,
        ) -> Self {
            let serial = canvas_fixture_serial().lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            CANVAS_DATA_ROOT_FIXTURE.with(|fixture| {
                *fixture.borrow_mut() = Some(data_root.to_path_buf());
            });
            *canvas_test_worker_exit_barrier().lock().unwrap() = Some(release.clone());
            canvas_test_worker_exit_reached().store(false, Ordering::Release);
            Self {
                _serial: serial,
                worker_release: Some(release),
                retirements: fixtures.iter().map(|fixture| (
                    fixture.context.user_activity.clone(), fixture.persistence.lease().clone(),
                )).collect(),
            }
        }
    }

    impl Drop for CanvasTestSeams {
        fn drop(&mut self) {
            canvas_test_worker_panic_after_send().store(false, Ordering::Release);
            if let Some(release) = self.worker_release.take() {
                release.store(true, Ordering::Release);
            }
            let mut cleanup_failures = Vec::new();
            for (activity, lease) in self.retirements.drain(..) {
                if let Err(error) = drain_canvas_preview_workers_for_lease_for_test(&lease) {
                    cleanup_failures.push(format!("preview: {error:#}"));
                }
                if let Err(error) = drain_canvas_workers_for_lease_for_test(&lease) {
                    cleanup_failures.push(format!("canvas: {error:#}"));
                }
                match activity.begin_quiesce(&lease) {
                    Ok(quiesced) => quiesced.retire(),
                    Err(error) => cleanup_failures.push(format!("quiesce: {error:#}")),
                }
            }
            *canvas_test_worker_exit_barrier().lock().unwrap() = None;
            CANVAS_PICKER_FIXTURE.with(|fixture| fixture.borrow_mut().clear());
            CANVAS_EXPORT_FIXTURE.with(|fixture| fixture.borrow_mut().clear());
            CANVAS_CLIPBOARD_FIXTURE.with(|fixture| fixture.borrow_mut().clear());
            CANVAS_DATA_ROOT_FIXTURE.with(|fixture| *fixture.borrow_mut() = None);
            CANVAS_PICKER_RETURN_HOOK.with(|hook| *hook.borrow_mut() = None);
            if !cleanup_failures.is_empty() && !std::thread::panicking() {
                panic!("canvas fixture cleanup failed: {}", cleanup_failures.join("; "));
            }
        }
    }

    struct CanvasWorkerReleases(Vec<Arc<std::sync::atomic::AtomicBool>>);
    impl Drop for CanvasWorkerReleases {
        fn drop(&mut self) {
            for release in &self.0 {
                release.store(true, Ordering::Release);
            }
        }
    }

    fn png(path: &Path, width: u32, height: u32, color: [u8; 4]) -> PathBuf {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        fs::write(path, encode_png_rgba(&image, width, height).unwrap()).unwrap();
        path.to_path_buf()
    }

    fn image_node(id: &str, source: impl Into<String>) -> CanvasNoteData {
        CanvasNoteData {
            id: id.into(),
            kind: "image".into(),
            image_path: source.into(),
            width: 340.0,
            height: 250.0,
            ..CanvasNoteData::default()
        }
    }

    fn owned_image(
        fixture: &video_image_callbacks::tests::scoped_inputs::Fixture,
        source: &Path,
    ) -> PathBuf {
        let bytes = fs::read(source).unwrap();
        persist_reference_image_for_namespace(&fixture.authority, &decode_reference_bytes(&bytes).unwrap()).unwrap()
    }

    fn node(id: &str) -> CanvasNoteData {
        CanvasNoteData {
            id: id.to_string(),
            kind: "text".to_string(),
            content: id.to_string(),
            x: 0.0,
            y: 0.0,
            width: 320.0,
            height: 210.0,
            ..CanvasNoteData::default()
        }
    }

    #[test]
    fn canvas_history_round_trips_undo_and_redo() {
        let mut history = CanvasController::default();
        history.record(CanvasSnapshot {
            notes: vec![node("before")],
            links: Vec::new(),
        });
        let previous = history
            .undo(CanvasSnapshot {
                notes: vec![node("after")],
                links: Vec::new(),
            })
            .expect("undo state");
        assert_eq!(previous.notes, vec![node("before")]);
        let next = history.redo(previous).expect("redo state");
        assert_eq!(next.notes, vec![node("after")]);
    }

    #[test]
    fn legacy_canvas_notes_receive_text_node_defaults() {
        let legacy = r#"{"id":"legacy","content":"old note","x":12.0,"y":24.0}"#;
        let parsed: CanvasNoteData = serde_json::from_str(legacy).expect("legacy canvas note");

        assert_eq!(parsed.kind, "text");
        assert_eq!(parsed.width, 280.0);
        assert_eq!(parsed.height, 176.0);
        assert_eq!(parsed.font_size, 12.0);
        assert_eq!(parsed.content, "old note");
    }

    #[test]
    fn legacy_canvas_links_keep_forward_flow_by_default() {
        let legacy = r#"{"id":"link","source_id":"a","target_id":"b"}"#;
        let parsed: CanvasLinkData = serde_json::from_str(legacy).expect("legacy canvas link");

        assert!(!parsed.flow_reversed);
    }

    #[test]
    fn canvas_links_reject_cycles_and_find_the_nearest_input() {
        let store = Store {
            canvas_notes: vec![
                node("source"),
                CanvasNoteData {
                    id: "target".to_string(),
                    x: 400.0,
                    ..node("target")
                },
            ],
            ..Store::default()
        };
        assert_eq!(
            target_at_input(&store, "source", 404.0, 105.0, 24.0).as_deref(),
            Some("target")
        );
        let links = vec![CanvasLinkData {
            id: "link".to_string(),
            source_id: "source".to_string(),
            target_id: "target".to_string(),
            flow_reversed: false,
        }];
        assert!(link_reaches(&links, "source", "target"));
        assert!(!link_reaches(&links, "target", "source"));
    }

    #[test]
    fn actual_canvas_callbacks_fail_closed_without_private_persistence() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let context = AppContext::default();
        app.global::<AppState>().set_generation_status("sentinel".into());
        wire_infinite_canvas_callbacks(&app, context.clone());

        app.global::<AppState>()
            .invoke_add_canvas_node("text".into(), 120.0, 90.0);

        assert!(context.store.borrow().canvas_notes.is_empty());
        assert_eq!(app.global::<AppState>().get_generation_status(), "sentinel");
    }

    #[test]
    fn actual_canvas_picker_never_applies_original_dialog_to_replacement_binding() {
        i_slint_backend_testing::init_no_event_loop();
        let original = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let replacement = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&original, &replacement]);
        let source = png(&temp.path().join("picker.png"), 8, 6, [20, 80, 180, 255]);
        original.context.store.borrow_mut().canvas_notes = vec![image_node("target", "")];
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, original.context.clone());
        CANVAS_PICKER_FIXTURE.with(|fixture| fixture.borrow_mut().push(source));
        let store = original.context.store.clone();
        let replacement_persistence = replacement.persistence.clone();
        CANVAS_PICKER_RETURN_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                store.borrow_mut().private_persistence = Some(replacement_persistence);
            }));
        });

        app.global::<AppState>()
            .invoke_choose_canvas_node_image("target".into());

        assert!(original.context.store.borrow().canvas_notes[0].image_path.is_empty());
    }

    #[test]
    fn actual_canvas_picker_and_clipboard_publish_only_indexed_namespace_inputs() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let source = png(&temp.path().join("picker.png"), 7, 5, [70, 30, 210, 255]);
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());
        CANVAS_PICKER_FIXTURE.with(|picker| picker.borrow_mut().push(source));

        app.global::<AppState>().invoke_add_canvas_uploaded_image(100.0, 80.0);
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.context.store.borrow().canvas_notes.len() == 1
        });
        let picked = fixture.context.store.borrow().canvas_notes[0].image_path.clone();
        assert!(Path::new(&picked).starts_with(
            fixture.authority.lease().namespace.path(ManagedUserArea::CanvasUploads)
        ));
        let picked_leaf = Path::new(&picked).file_name().unwrap().to_str().unwrap();
        assert!(fixture.authority.delivery_index().unwrap()
            .find_file_by_path_for_namespace(
                &fixture.authority, ManagedUserArea::CanvasUploads, picked_leaf,
            ).unwrap().is_some());

        CANVAS_CLIPBOARD_FIXTURE.with(|clipboard| clipboard.borrow_mut().push(
            CanvasSystemClipboard::Image {
                fingerprint: 71,
                width: 2,
                height: 2,
                bytes: [40, 90, 180, 255].repeat(4),
            },
        ));
        app.global::<AppState>().invoke_paste_canvas_content(140.0, 100.0);
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.context.store.borrow().canvas_notes.len() == 2
        });
        let pasted = fixture.context.store.borrow().canvas_notes.iter()
            .find(|note| note.id != fixture.context.store.borrow().canvas_notes[0].id)
            .unwrap().image_path.clone();
        assert!(Path::new(&pasted).starts_with(
            fixture.authority.lease().namespace.path(ManagedUserArea::CanvasUploads)
        ));
        let pasted_leaf = Path::new(&pasted).file_name().unwrap().to_str().unwrap();
        assert!(fixture.authority.delivery_index().unwrap()
            .find_file_by_path_for_namespace(
                &fixture.authority, ManagedUserArea::CanvasUploads, pasted_leaf,
            ).unwrap().is_some());
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
                .ok().flatten().is_some_and(|saved| saved.canvas_notes.len() == 2)
        });
    }

    #[test]
    fn actual_canvas_copy_paste_preserves_internal_fingerprint_behavior_and_sqlite_order() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        fixture.context.store.borrow_mut().canvas_notes = vec![CanvasNoteData {
            id: "copy-source".into(), kind: "text".into(), content: "copied".into(),
            width: 100.0, height: 80.0, selected: true, ..CanvasNoteData::default()
        }];
        for _ in 0..2 {
            CANVAS_CLIPBOARD_FIXTURE.with(|clipboard| clipboard.borrow_mut().push(
                CanvasSystemClipboard::Text { fingerprint: 88, text: "external".into() },
            ));
        }
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        app.global::<AppState>().invoke_copy_canvas_selection();
        app.global::<AppState>().invoke_paste_canvas_content(200.0, 160.0);
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
                .ok().flatten().is_some_and(|saved| saved.canvas_notes.len() == 2)
        });

        let saved = fixture.writer
            .load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap()
            .unwrap();
        assert_eq!(saved.canvas_notes.len(), 2);
        assert!(saved.canvas_notes.iter().all(|note| note.content == "copied"));
    }

    #[test]
    fn actual_canvas_sqlite_rejection_does_not_poison_later_saves() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        fixture.context.store.borrow_mut().custom_prompts.push("force-trigger".into());
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        fixture.writer.reject_custom_prompt_inserts_for_test(true);
        app.global::<AppState>().invoke_add_canvas_node("text".into(), 40.0, 40.0);
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            canvas_workers().lock().unwrap().workers.is_empty()
        });
        assert!(!canvas_workers().lock().unwrap().failed);

        fixture.writer.reject_custom_prompt_inserts_for_test(false);
        app.global::<AppState>().invoke_add_canvas_node("text".into(), 120.0, 80.0);
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
                .ok().flatten().is_some_and(|saved| saved.canvas_notes.len() == 2)
        });
        assert!(!canvas_workers().lock().unwrap().failed);
    }

    #[test]
    fn actual_canvas_trip_before_commit_blocks_store_projection_and_sqlite() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());
        let capture = CanvasActionCapture::capture(&fixture.context.store).unwrap();
        let effect = capture.begin_effect(&fixture.context.store).unwrap();
        let latch = fixture.persistence.upgrade_latch();
        let trip_latch = latch.clone();
        let trip = std::thread::spawn(move || {
            trip_latch.trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        });
        while latch.snapshot().is_none() { std::thread::yield_now(); }

        app.global::<AppState>().invoke_add_canvas_node("text".into(), 40.0, 40.0);

        assert!(fixture.context.store.borrow().canvas_notes.is_empty());
        assert_eq!(app.global::<AppState>().get_canvas_notes().row_count(), 0);
        assert!(fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap().is_none());
        drop(effect);
        trip.join().unwrap();
    }

    #[test]
    fn actual_canvas_admitted_commit_holds_activity_until_pure_publish_returns() {
        struct RetirementJoin(Option<std::thread::JoinHandle<()>>);

        impl RetirementJoin {
            fn join(mut self) {
                self.0
                    .take()
                    .expect("retirement worker must have started")
                    .join()
                    .expect("retirement worker must finish cleanly");
            }
        }

        impl Drop for RetirementJoin {
            fn drop(&mut self) {
                if let Some(worker) = self.0.take() {
                    // On assertion unwind, CanvasActionCapture::apply has already
                    // dropped its activity permit before this outer guard runs.
                    let _ = worker.join();
                }
            }
        }

        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let app = AppWindow::new().unwrap();
        let capture = CanvasActionCapture::capture(&fixture.context.store).unwrap();
        let gate = fixture.context.user_activity.clone();
        let lease = fixture.persistence.lease().clone();
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let quiesce_finished = finished.clone();
        let mut retirement = RetirementJoin(None);

        let applied = capture.apply(&fixture.context.store, || {
            let worker_gate = gate.clone();
            let worker_lease = lease.clone();
            retirement.0 = Some(std::thread::spawn(move || {
                let _quiesced = worker_gate.begin_quiesce(&worker_lease).unwrap();
                quiesce_finished.store(true, Ordering::Release);
            }));
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut quiescing = false;
            while std::time::Instant::now() < deadline {
                if gate.begin_recovery_unit(&lease).is_err() {
                    quiescing = true;
                    break;
                }
                std::thread::yield_now();
            }
            let held_during_publish = !finished.load(Ordering::Acquire);
            if quiescing && held_during_publish {
                fixture.context.store.borrow_mut().canvas_notes.push(CanvasNoteData {
                    id: "admitted".into(), kind: "text".into(), ..CanvasNoteData::default()
                });
                app.global::<AppState>().set_generation_status("admitted-publish".into());
            }
            (quiescing, held_during_publish)
        });
        retirement.join();
        assert_eq!(applied, Some((true, true)));
        assert!(finished.load(Ordering::Acquire));
        assert_eq!(fixture.context.store.borrow().canvas_notes[0].id, "admitted");
        assert_eq!(app.global::<AppState>().get_generation_status(), "admitted-publish");
    }

    #[test]
    fn actual_canvas_workspace_switch_publishes_and_reaches_sqlite_without_nested_latch() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        fixture.context.store.borrow_mut().canvas_workspaces.insert(
            "alternate".into(),
            CanvasWorkspaceData {
                notes: vec![CanvasNoteData {
                    id: "alternate-note".into(), kind: "text".into(), content: "alternate".into(),
                    ..CanvasNoteData::default()
                }],
                prompt: "alternate prompt".into(),
                ..CanvasWorkspaceData::default()
            },
        );
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        app.global::<AppState>().invoke_open_canvas_workspace("alternate".into());
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
                .ok().flatten().is_some_and(|saved| {
                    saved.active_canvas_workspace_id == "alternate"
                        && saved.canvas_notes.iter().any(|note| note.id == "alternate-note")
                })
        });

        assert_eq!(fixture.context.store.borrow().active_canvas_workspace_id, "alternate");
        assert_eq!(app.global::<AppState>().get_canvas_notes().row_count(), 1);
        assert!(app.global::<AppState>().get_canvas_workspace_switch_request() > 0);
    }

    #[test]
    fn actual_canvas_export_overwrites_existing_destination_and_keeps_source() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let external_source = png(&temp.path().join("source.png"), 4, 4, [12, 34, 56, 255]);
        let source = owned_image(&fixture, &external_source);
        let source_bytes = fixture.authority.read_image_source(&source, 1024 * 1024).unwrap();
        assert_ne!(source_bytes, b"preserve-existing");
        let destination = temp.path().join("existing.png");
        fs::write(&destination, b"preserve-existing").unwrap();
        fixture.context.store.borrow_mut().canvas_notes = vec![image_node(
            "export", source.display().to_string(),
        )];
        CANVAS_EXPORT_FIXTURE.with(|picker| picker.borrow_mut().push(destination.clone()));
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        app.global::<AppState>().invoke_save_canvas_image("export".into());

        // The user accepts same-name overwrite; the original source stays intact.
        assert_eq!(fs::read(&destination).unwrap(), source_bytes);
        assert_eq!(fixture.authority.read_image_source(&source, 1024 * 1024).unwrap(), source_bytes);
    }

    #[test]
    fn actual_canvas_split_terminal_waits_for_the_registered_worker_exit() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let external = png(&temp.path().join("source.png"), 4, 4, [90, 120, 180, 255]);
        let source = owned_image(&fixture, &external);
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _seams = CanvasTestSeams::with_worker_release(&fixture, temp.path(), release.clone());
        fixture.context.store.borrow_mut().canvas_notes = vec![image_node(
            "split", source.display().to_string(),
        )];
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        app.global::<AppState>()
            .invoke_split_canvas_image("split".into(), "1".into(), "1".into(), ModelRc::new(VecModel::from(vec![0.5])), ModelRc::new(VecModel::from(vec![0.5])));
        video_image_callbacks::tests::scoped_inputs::pump(|| canvas_test_worker_exit_reached().load(Ordering::Acquire));
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        slint::platform::update_timers_and_animations();

        assert_eq!(app.global::<AppState>().get_canvas_split_loading_node_id(), "split");
        assert_eq!(fixture.context.store.borrow().canvas_notes.len(), 1);
        release.store(true, Ordering::Release);
        video_image_callbacks::tests::scoped_inputs::pump(|| app.global::<AppState>().get_canvas_split_loading_node_id().is_empty());
    }

    #[test]
    fn legacy_canvas_open_does_not_read_or_rewrite_arbitrary_saved_paths() {
        let temp = tempfile::tempdir().unwrap();
        let source = png(&temp.path().join("legacy.png"), 16, 9, [20, 20, 20, 255]);
        let mut notes = vec![CanvasNoteData {
            id: "legacy".into(),
            kind: "image".into(),
            image_path: source.display().to_string(),
            x: 40.0,
            y: 60.0,
            width: 340.0,
            height: 191.25,
            ..CanvasNoteData::default()
        }];

        assert!(!migrate_legacy_auto_sized_canvas_images(&mut notes));
        assert_eq!((notes[0].x, notes[0].y, notes[0].width, notes[0].height),
            (40.0, 60.0, 340.0, 191.25));
    }

    #[test]
    fn actual_canvas_picker_never_reports_success_without_ordered_sqlite_ack() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let source = png(&temp.path().join("rejected.png"), 6, 4, [200, 30, 40, 255]);
        fixture.context.store.borrow_mut().canvas_notes = vec![image_node("target", "")];
        fixture.writer.deactivate(fixture.persistence.lease()).unwrap();
        CANVAS_PICKER_FIXTURE.with(|picker| picker.borrow_mut().push(source));
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        app.global::<AppState>()
            .invoke_choose_canvas_node_image("target".into());

        video_image_callbacks::tests::scoped_inputs::pump(|| {
            let status = app.global::<AppState>().get_generation_status();
            status.contains("未确认") || status.contains("not confirmed")
        });

        assert!(!app.global::<AppState>().get_generation_status().contains("添加"));
        assert!(fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap().is_none());
    }

    #[test]
    fn actual_canvas_picker_reports_success_only_after_index_and_ordered_sqlite_ack() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let source = png(&temp.path().join("accepted.png"), 6, 4, [40, 160, 90, 255]);
        fixture.context.store.borrow_mut().canvas_notes = vec![image_node("target", "")];
        CANVAS_PICKER_FIXTURE.with(|picker| picker.borrow_mut().push(source));
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        app.global::<AppState>()
            .invoke_choose_canvas_node_image("target".into());
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            let status = app.global::<AppState>().get_generation_status();
            status.contains("添加") || status.contains("Image added")
        });

        let saved = fixture.writer
            .load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap()
            .unwrap();
        assert_eq!(saved.canvas_notes.len(), 1);
        let saved_path = PathBuf::from(&saved.canvas_notes[0].image_path);
        assert!(saved_path.starts_with(
            fixture.authority.lease().namespace.path(ManagedUserArea::CanvasUploads)
        ));
        let leaf = saved_path.file_name().unwrap().to_str().unwrap();
        assert!(fixture.authority.delivery_index().unwrap()
            .find_file_by_path_for_namespace(
                &fixture.authority, ManagedUserArea::CanvasUploads, leaf,
            ).unwrap().is_some());
    }

    #[test]
    fn actual_canvas_newer_picker_request_wins_when_workers_finish_in_reverse_order() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let first = png(&temp.path().join("first.png"), 8, 4, [220, 40, 40, 255]);
        let second = png(&temp.path().join("second.png"), 4, 8, [40, 220, 40, 255]);
        let first_release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let second_release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _releases = CanvasWorkerReleases(vec![first_release.clone(), second_release.clone()]);
        fixture.context.store.borrow_mut().canvas_notes = vec![image_node("target", "")];
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        *canvas_test_worker_exit_barrier().lock().unwrap() = Some(first_release.clone());
        canvas_test_worker_exit_reached().store(false, Ordering::Release);
        CANVAS_PICKER_FIXTURE.with(|picker| picker.borrow_mut().push(first));
        app.global::<AppState>().invoke_choose_canvas_node_image("target".into());
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            canvas_test_worker_exit_reached().load(Ordering::Acquire)
        });

        *canvas_test_worker_exit_barrier().lock().unwrap() = Some(second_release.clone());
        canvas_test_worker_exit_reached().store(false, Ordering::Release);
        CANVAS_PICKER_FIXTURE.with(|picker| picker.borrow_mut().push(second));
        app.global::<AppState>().invoke_choose_canvas_node_image("target".into());
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            canvas_test_worker_exit_reached().load(Ordering::Acquire)
        });

        first_release.store(true, Ordering::Release);
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            canvas_workers().lock().unwrap().workers.len() == 1
        });
        assert!(fixture.context.store.borrow().canvas_notes[0].image_path.is_empty());

        second_release.store(true, Ordering::Release);
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            !fixture.context.store.borrow().canvas_notes[0].image_path.is_empty()
        });
        let selected = PathBuf::from(&fixture.context.store.borrow().canvas_notes[0].image_path);
        let bytes = fixture.authority.read_image_source(&selected, 1024 * 1024).unwrap();
        let (decoded, _) = decode_image_bytes(&selected, &bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 8));
    }

    #[test]
    fn actual_canvas_split_late_completion_never_mutates_replacement_binding() {
        i_slint_backend_testing::init_no_event_loop();
        let original = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let replacement = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let external = png(&temp.path().join("late-source.png"), 4, 4, [40, 90, 180, 255]);
        let source = owned_image(&original, &external);
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _seams = CanvasTestSeams::with_worker_release_for_fixtures(
            temp.path(), &[&original, &replacement], release.clone(),
        );
        original.context.store.borrow_mut().canvas_notes = vec![image_node(
            "split", source.display().to_string(),
        )];
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, original.context.clone());

        app.global::<AppState>()
            .invoke_split_canvas_image("split".into(), "1".into(), "1".into(), ModelRc::new(VecModel::from(vec![0.5])), ModelRc::new(VecModel::from(vec![0.5])));
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            canvas_test_worker_exit_reached().load(Ordering::Acquire)
        });
        {
            let mut store = original.context.store.borrow_mut();
            store.private_persistence = Some(replacement.persistence.clone());
            store.canvas_notes.clear();
        }
        app.global::<AppState>().set_canvas_split_loading_node_id("replacement".into());
        app.global::<AppState>().set_generation_status("replacement-status".into());
        release.store(true, Ordering::Release);
        let original_lease = original.persistence.lease().clone();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            !canvas_workers().lock().unwrap().workers.iter()
                .any(|worker| worker.lease == original_lease)
        });

        assert!(original.context.store.borrow().canvas_notes.is_empty());
        assert_eq!(app.global::<AppState>().get_canvas_split_loading_node_id(), "replacement");
        assert_eq!(app.global::<AppState>().get_generation_status(), "replacement-status");
    }

    #[test]
    fn actual_viewer_canvas_import_publishes_index_before_ordered_store() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let source = png(&temp.path().join("viewer.png"), 5, 7, [120, 60, 190, 255]);
        let app = AppWindow::new().unwrap();

        let completion = Rc::new(Cell::new(false));
        let completed = completion.clone();
        start_viewer_image_import_to_canvas(&app, fixture.context.clone(), source, move |result| {
            result.unwrap();
            completed.set(true);
        }).unwrap();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            completion.get()
        });

        let saved = fixture.writer
            .load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap()
            .unwrap();
        let path = PathBuf::from(&saved.canvas_notes[0].image_path);
        assert!(path.starts_with(
            fixture.authority.lease().namespace.path(ManagedUserArea::CanvasUploads)
        ));
        let leaf = path.file_name().unwrap().to_str().unwrap();
        assert!(fixture.authority.delivery_index().unwrap()
            .find_file_by_path_for_namespace(
                &fixture.authority, ManagedUserArea::CanvasUploads, leaf,
            ).unwrap().is_some());
    }

    #[test]
    fn actual_viewer_canvas_import_completion_requires_sqlite_ack() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let source = png(&temp.path().join("viewer-rejected.png"), 5, 7, [90, 40, 160, 255]);
        fixture.writer.deactivate(fixture.persistence.lease()).unwrap();
        let app = AppWindow::new().unwrap();
        let completion = Rc::new(Cell::new(None));
        let completed = completion.clone();

        start_viewer_image_import_to_canvas(&app, fixture.context.clone(), source, move |result| {
            completed.set(Some(result.is_ok()));
        }).unwrap();
        video_image_callbacks::tests::scoped_inputs::pump(|| completion.get().is_some());

        assert_eq!(completion.get(), Some(false));
        assert!(fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap().is_none());
    }

    #[test]
    fn actual_canvas_split_and_extraction_commit_owned_indexed_outputs_to_sqlite() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let _seams = CanvasTestSeams::for_fixtures(temp.path(), &[&fixture]);
        let external = temp.path().join("components.png");
        let mut image = image::RgbaImage::from_pixel(480, 360, image::Rgba([250, 249, 246, 255]));
        for (left, top, right, bottom, color) in [
            (30, 35, 155, 130, [32, 74, 180, 255]),
            (280, 30, 430, 120, [204, 62, 72, 255]),
            (45, 225, 175, 325, [56, 164, 92, 255]),
            (290, 210, 440, 330, [128, 68, 184, 255]),
        ] {
            for y in top..bottom {
                for x in left..right {
                    image.put_pixel(x, y, image::Rgba(color));
                }
            }
        }
        fs::write(&external, encode_png_rgba(&image, image.width(), image.height()).unwrap()).unwrap();
        let source = owned_image(&fixture, &external);
        fixture.context.store.borrow_mut().canvas_notes = vec![image_node(
            "source", source.display().to_string(),
        )];
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());

        app.global::<AppState>()
            .invoke_split_canvas_image("source".into(), "1".into(), "1".into(), ModelRc::new(VecModel::from(vec![0.5])), ModelRc::new(VecModel::from(vec![0.5])));
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            app.global::<AppState>().get_canvas_split_loading_node_id().is_empty()
                && fixture.context.store.borrow().canvas_notes.len() == 5
        });
        app.global::<AppState>().invoke_extract_canvas_ui_elements("source".into());
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            app.global::<AppState>().get_canvas_extraction_loading_node_id().is_empty()
                && fixture.context.store.borrow().canvas_notes.len() == 9
        });

        let saved = fixture.writer
            .load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap()
            .unwrap();
        assert_eq!(saved.canvas_notes.len(), 9);
        for output in saved.canvas_notes.iter().filter(|note| note.id != "source") {
            let path = PathBuf::from(&output.image_path);
            assert!(path.starts_with(
                fixture.authority.lease().namespace.path(ManagedUserArea::Canvas)
            ));
            let leaf = path.file_name().unwrap().to_str().unwrap();
            assert!(fixture.authority.delivery_index().unwrap()
                .find_file_by_path_for_namespace(
                    &fixture.authority, ManagedUserArea::Canvas, leaf,
                ).unwrap().is_some());
        }
    }

    #[test]
    fn actual_canvas_window_loss_still_joins_registered_split_worker() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let source = owned_image(
            &fixture,
            &png(&temp.path().join("window-loss.png"), 4, 4, [40, 100, 200, 255]),
        );
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _seams = CanvasTestSeams::with_worker_release(&fixture, temp.path(), release.clone());
        fixture.context.store.borrow_mut().canvas_notes = vec![image_node(
            "split", source.display().to_string(),
        )];
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());
        app.global::<AppState>()
            .invoke_split_canvas_image("split".into(), "1".into(), "1".into(), ModelRc::new(VecModel::from(vec![0.5])), ModelRc::new(VecModel::from(vec![0.5])));
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            canvas_test_worker_exit_reached().load(Ordering::Acquire)
        });

        drop(app);
        let lease = fixture.persistence.lease().clone();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            canvas_workers().lock().unwrap().workers.iter()
                .find(|worker| worker.lease == lease)
                .is_some_and(|worker| worker.cancel.load(Ordering::Acquire))
        });
        release.store(true, Ordering::Release);
        drain_canvas_workers_for_lease_for_test(fixture.persistence.lease()).unwrap();
    }

    #[test]
    fn actual_canvas_worker_panic_is_sticky_in_isolated_process() {
        const CHILD: &str = "ARTFORGE_CANVAS_PANIC_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("actual_canvas_worker_panic_is_sticky_in_isolated_process")
                .arg("--test-threads=1")
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }

        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let ticket = spawn_canvas_worker(fixture.persistence.clone(), |_, _| {
            panic!("controlled canvas worker panic");
        }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !canvas_workers().lock().unwrap().workers.iter()
            .find(|worker| worker.id == ticket.id)
            .is_some_and(|worker| worker.handle.is_finished())
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert!(finish_canvas_worker_if_ready(ticket.id).is_err());
        assert!(spawn_canvas_worker(fixture.persistence.clone(), |_, _| {}).is_err());
        assert!(drain_canvas_workers_for_shutdown().is_err());
        assert!(drain_canvas_workers_for_shutdown().is_err());
        fixture.context.user_activity
            .begin_quiesce(fixture.persistence.lease()).unwrap().retire();
    }

    #[test]
    fn actual_canvas_send_then_panic_never_applies_success_payload() {
        const CHILD: &str = "ARTFORGE_CANVAS_SEND_PANIC_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("actual_canvas_send_then_panic_never_applies_success_payload")
                .arg("--test-threads=1")
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }

        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let temp = tempfile::tempdir().unwrap();
        let source = owned_image(
            &fixture,
            &png(&temp.path().join("panic-source.png"), 4, 4, [80, 120, 200, 255]),
        );
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _releases = CanvasWorkerReleases(vec![release.clone()]);
        *canvas_test_worker_exit_barrier().lock().unwrap() = Some(release.clone());
        canvas_test_worker_exit_reached().store(false, Ordering::Release);
        canvas_test_worker_panic_after_send().store(true, Ordering::Release);
        fixture.context.store.borrow_mut().canvas_notes = vec![image_node(
            "split", source.display().to_string(),
        )];
        let app = AppWindow::new().unwrap();
        wire_infinite_canvas_callbacks(&app, fixture.context.clone());
        app.global::<AppState>()
            .invoke_split_canvas_image("split".into(), "1".into(), "1".into(), ModelRc::new(VecModel::from(vec![0.5])), ModelRc::new(VecModel::from(vec![0.5])));
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            canvas_test_worker_exit_reached().load(Ordering::Acquire)
        });

        release.store(true, Ordering::Release);
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            app.global::<AppState>().get_canvas_split_loading_node_id().is_empty()
        });

        assert_eq!(fixture.context.store.borrow().canvas_notes.len(), 1);
        assert!(drain_canvas_workers_for_shutdown().is_err());
        *canvas_test_worker_exit_barrier().lock().unwrap() = None;
        fixture.context.user_activity
            .begin_quiesce(fixture.persistence.lease()).unwrap().retire();
    }

    #[test]
    fn actual_canvas_shutdown_cancels_and_joins_workers_in_isolated_process() {
        const CHILD: &str = "ARTFORGE_CANVAS_SHUTDOWN_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("actual_canvas_shutdown_cancels_and_joins_workers_in_isolated_process")
                .arg("--test-threads=1")
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }

        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let (reached, observed) = mpsc::channel();
        spawn_canvas_worker(fixture.persistence.clone(), move |cancel, _| {
            let _ = reached.send(());
            while !cancel.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        }).unwrap();
        observed.recv_timeout(Duration::from_secs(3)).unwrap();
        drain_canvas_workers_for_shutdown().unwrap();
        assert!(spawn_canvas_worker(fixture.persistence.clone(), |_, _| {}).is_err());
        fixture.context.user_activity
            .begin_quiesce(fixture.persistence.lease()).unwrap().retire();
    }

    #[test]
    fn split_spans_assign_pixel_remainders_to_terminal_tiles() {
        assert_eq!(terminal_remainder_span(10, 3, 0), (0, 3));
        assert_eq!(terminal_remainder_span(10, 3, 1), (3, 3));
        assert_eq!(terminal_remainder_span(10, 3, 2), (6, 4));
    }

    #[test]
    fn split_line_counts_create_one_more_tile_part_per_axis() {
        assert_eq!(split_parts_from_lines(0), Some(1));
        assert_eq!(split_parts_from_lines(1), Some(2));
        assert_eq!(split_parts_from_lines(2), Some(3));
        assert_eq!(split_parts_from_lines(64), Some(65));
    }

    #[test]
    fn moved_split_lines_preserve_pixel_boundaries_and_reject_empty_tiles() {
        assert_eq!(split_pixel_edges(100, &[0.2, 0.73]).unwrap(), vec![0, 20, 73, 100]);
        assert_eq!(split_pixel_edges(100, &[]).unwrap(), vec![0, 100]);
        for positions in [vec![0.0], vec![1.0], vec![f32::NAN], vec![0.7, 0.2], vec![0.201, 0.202]] {
            assert!(split_pixel_edges(100, &positions).is_err());
        }
    }

    #[test]
    fn canvas_image_split_is_lossless_and_covers_every_pixel() {
        let temp = tempfile::tempdir().expect("temporary split directory");
        let source_path = temp.path().join("source.png");
        let data_root = temp.path().join("data");
        let configured_output_root = temp.path().join("output");
        let output_dir = data_root.join("canvas/splits/fixture");
        fs::create_dir(&data_root).expect("prepare fixture data root");
        let sentinel = temp.path().join("preserve");
        fs::write(&sentinel, b"untouched").unwrap();
        let mut source = image::RgbaImage::new(5, 3);
        for y in 0..3 {
            for x in 0..5 {
                source.put_pixel(x, y, image::Rgba([x as u8, y as u8, 42, 255]));
            }
        }
        let bytes = encode_png_rgba(&source, 5, 3).expect("encode split source");
        atomic_write_file(&source_path, &bytes).expect("write split source");

        for (row_positions, column_positions, expected_x, expected_y) in [
            (vec![1.0 / 3.0], vec![0.4], vec![0, 2], vec![0, 1]),
            (vec![], vec![0.2, 0.8], vec![0, 1, 4], vec![0]),
            (vec![2.0 / 3.0], vec![], vec![0], vec![0, 2]),
        ] {
            let tiles = split_canvas_image_to_directory(&source_path, &output_dir, &data_root, &configured_output_root, &row_positions, &column_positions).expect("split source");
            assert_eq!(tiles.len(), expected_x.len() * expected_y.len());
            let mut rebuilt = image::RgbaImage::new(5, 3);
            for tile in &tiles {
                let left = expected_x[tile.column as usize];
                let top = expected_y[tile.row as usize];
                let decoded = image::open(&tile.path).expect("read tile").to_rgba8();
                assert_eq!(decoded.dimensions(), (tile.width, tile.height));
                image::imageops::replace(&mut rebuilt, &decoded, left, top);
            }
            assert_eq!(rebuilt, source);
            remove_canvas_split_tiles(&tiles);
        }
        assert_eq!(fs::read(sentinel).unwrap(), b"untouched");
        assert_eq!(fs::read(source_path).unwrap(), bytes);
        assert!(!configured_output_root.exists());
    }

    #[test]
    fn canvas_element_extraction_saves_transparent_png_board_images() {
        let temp = tempfile::tempdir().expect("temporary extraction source directory");
        let source_path = temp.path().join("source.png");
        let data_root = temp.path().join("data");
        let configured_output_root = temp.path().join("output");
        let output_dir = data_root.join("canvas/ui-extractions/fixture");
        fs::create_dir(&data_root).expect("prepare fixture data root");
        let sentinel = temp.path().join("preserve");
        fs::write(&sentinel, b"untouched").unwrap();
        let mut source = image::RgbaImage::from_pixel(480, 360, image::Rgba([250, 249, 246, 255]));
        for (left, top, right, bottom, color) in [
            (30, 35, 155, 130, [32, 74, 180, 255]),
            (280, 30, 430, 120, [204, 62, 72, 255]),
            (45, 225, 175, 325, [56, 164, 92, 255]),
            (290, 210, 440, 330, [128, 68, 184, 255]),
        ] {
            for y in top..bottom {
                for x in left..right {
                    source.put_pixel(x, y, image::Rgba(color));
                }
            }
        }
        let bytes = encode_png_rgba(&source, source.width(), source.height())
            .expect("encode extraction source");
        atomic_write_file(&source_path, &bytes).expect("write extraction source");

        let elements = extract_canvas_elements_to_directory(
            &source_path, &output_dir, &data_root, &configured_output_root,
        )
            .expect("extract current canvas image");

        assert_eq!(elements.len(), 4);
        for element in &elements {
            assert!(Path::new(&element.path).starts_with(&output_dir));
            assert!(element.path.ends_with(".png"));
            let decoded = image::open(&element.path)
                .expect("read extracted PNG")
                .to_rgba8();
            assert!(decoded.pixels().any(|pixel| pixel[3] == 0));
        }
        remove_canvas_extracted_elements(&elements);
        assert_eq!(fs::read(sentinel).unwrap(), b"untouched");
        assert_eq!(fs::read(source_path).unwrap(), bytes);
        assert!(!configured_output_root.exists());
    }
}
