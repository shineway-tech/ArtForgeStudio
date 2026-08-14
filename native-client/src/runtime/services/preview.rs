use super::*;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const PREVIEW_CACHE_VERSION: u32 = 1;
const MEMORY_CACHE_LIMIT_BYTES: usize = 96 * 1024 * 1024;
const DISK_CACHE_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
const DISK_CACHE_TARGET_BYTES: u64 = DISK_CACHE_LIMIT_BYTES * 4 / 5;
const MAX_SOURCE_PIXELS: u64 = 100_000_000;
const MAX_PREVIEW_SOURCE_PIXELS: u64 = 50_000_000;
const PREVIEW_WORKER_COUNT: usize = 1;
const PREVIEW_QUEUE_CAPACITY: usize = 256;
const DISK_CLEANUP_WRITE_BYTES: u64 = 8 * 1024 * 1024;
const PREVIEW_RETRY_ATTEMPTS: u8 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreviewPurpose {
    Gallery,
    Reference,
    Toolbox,
    Canvas,
    Showcase,
    Viewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreviewCollection {
    Assets,
    Generations,
    Inspiration,
}

#[derive(Clone, Debug)]
pub(super) struct PreviewTarget {
    pub(super) collection: PreviewCollection,
    pub(super) asset_id: String,
    pub(super) source_path: String,
    pub(super) flat_row: Option<usize>,
    pub(super) group_row: Option<usize>,
    pub(super) group_item_row: Option<usize>,
}

impl PartialEq for PreviewTarget {
    fn eq(&self, other: &Self) -> bool {
        self.collection == other.collection
            && self.asset_id == other.asset_id
            && self.source_path == other.source_path
    }
}

impl Eq for PreviewTarget {}

impl PreviewPurpose {
    pub(super) fn longest_edge(self) -> u32 {
        match self {
            Self::Reference | Self::Toolbox => 256,
            Self::Gallery => 384,
            // Canvas images can be resized and zoomed well beyond thumbnail size.
            // Keep enough source pixels to avoid magnifying a small cached preview,
            // while still bounding memory usage when many images share the canvas.
            Self::Canvas => 1024,
            Self::Showcase => 1024,
            Self::Viewer => 2048,
        }
    }

    fn cache_name(self) -> &'static str {
        match self {
            Self::Gallery => "gallery",
            Self::Reference => "reference",
            Self::Toolbox => "toolbox",
            Self::Canvas => "canvas",
            Self::Showcase => "showcase",
            Self::Viewer => "viewer",
        }
    }
}

#[derive(Clone)]
struct MemoryEntry {
    image: Image,
    bytes: usize,
    last_used: u64,
}

#[derive(Default)]
struct PreviewMemoryCache {
    entries: BTreeMap<String, MemoryEntry>,
    bytes: usize,
    clock: u64,
}

impl PreviewMemoryCache {
    fn get(&mut self, key: &str) -> Option<Image> {
        let entry = self.entries.get_mut(key)?;
        self.clock = self.clock.saturating_add(1);
        entry.last_used = self.clock;
        Some(entry.image.clone())
    }

    fn insert(&mut self, key: String, image: Image, bytes: usize) {
        self.clock = self.clock.saturating_add(1);
        if let Some(previous) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.insert(
            key,
            MemoryEntry {
                image,
                bytes,
                last_used: self.clock,
            },
        );
        while self.bytes > MEMORY_CACHE_LIMIT_BYTES && self.entries.len() > 1 {
            let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest_key) {
                self.bytes = self.bytes.saturating_sub(removed.bytes);
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn remove(&mut self, keys: &[String]) {
        for key in keys {
            if let Some(removed) = self.entries.remove(key) {
                self.bytes = self.bytes.saturating_sub(removed.bytes);
            }
        }
    }
}

thread_local! {
    static PREVIEW_MEMORY_CACHE: RefCell<PreviewMemoryCache> = RefCell::new(PreviewMemoryCache::default());
}

static DISK_CLEANUP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static SOURCE_DECODE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static DISK_WRITTEN_BYTES: AtomicU64 = AtomicU64::new(0);
static PREVIEW_CACHE_EPOCH: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct PreviewPixels {
    rgba: Arc<Vec<u8>>,
    width: u32,
    height: u32,
}

pub(super) struct PreparedPreview {
    key: String,
    pixels: Arc<PreviewPixels>,
}

impl PreviewPixels {
    fn bytes(&self) -> usize {
        self.rgba.len()
    }
}

struct PreviewJob {
    job_id: u64,
    queue_key: String,
    path: PathBuf,
    longest_edge: u32,
    purpose: String,
    cache_epoch: u64,
}

struct PreviewSubscriber {
    app: Weak<AppWindow>,
    target: PreviewTarget,
}

struct PreviewSubscriberBucket {
    job_id: u64,
    subscribers: Vec<PreviewSubscriber>,
}

#[derive(Default)]
struct PreviewWorkState {
    next_job_id: u64,
    queue: VecDeque<PreviewJob>,
    subscribers: BTreeMap<String, PreviewSubscriberBucket>,
}

#[derive(Default)]
struct PreviewWorkQueue {
    state: Mutex<PreviewWorkState>,
    wakeup: Condvar,
}

impl PreviewWorkQueue {
    fn enqueue(&self, mut job: PreviewJob, subscriber: PreviewSubscriber) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if let Some(bucket) = state.subscribers.get_mut(&job.queue_key) {
            if !bucket
                .subscribers
                .iter()
                .any(|existing| existing.target == subscriber.target)
            {
                bucket.subscribers.push(subscriber);
            }
            return true;
        }
        if state.queue.len() >= PREVIEW_QUEUE_CAPACITY {
            return false;
        }
        state.next_job_id = state.next_job_id.wrapping_add(1).max(1);
        job.job_id = state.next_job_id;
        state.subscribers.insert(
            job.queue_key.clone(),
            PreviewSubscriberBucket {
                job_id: job.job_id,
                subscribers: vec![subscriber],
            },
        );
        state.queue.push_back(job);
        self.wakeup.notify_one();
        true
    }

    fn next_job(&self) -> PreviewJob {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(job) = state.queue.pop_front() {
                return job;
            }
            state = self
                .wakeup
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn has_subscribers(&self, key: &str, job_id: u64) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .subscribers
            .get(key)
            .is_some_and(|bucket| bucket.job_id == job_id && !bucket.subscribers.is_empty())
    }

    fn take_subscribers(&self, key: &str, job_id: u64) -> Vec<PreviewSubscriber> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state
            .subscribers
            .get(key)
            .is_some_and(|bucket| bucket.job_id == job_id)
        {
            return Vec::new();
        }
        state
            .subscribers
            .remove(key)
            .map(|bucket| bucket.subscribers)
            .unwrap_or_default()
    }

    fn cancel_collection(&self, collection: PreviewCollection) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        for bucket in state.subscribers.values_mut() {
            bucket
                .subscribers
                .retain(|subscriber| subscriber.target.collection != collection);
        }
        let empty_keys = state
            .subscribers
            .iter()
            .filter_map(|(key, bucket)| bucket.subscribers.is_empty().then_some(key.clone()))
            .collect::<BTreeSet<_>>();
        for key in &empty_keys {
            state.subscribers.remove(key);
        }
        state
            .queue
            .retain(|job| !empty_keys.contains(&job.queue_key));
    }

    fn cancel_all(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.queue.clear();
        state.subscribers.clear();
    }
}

static PREVIEW_WORK_QUEUE: OnceLock<Arc<PreviewWorkQueue>> = OnceLock::new();

fn preview_work_queue() -> &'static Arc<PreviewWorkQueue> {
    PREVIEW_WORK_QUEUE.get_or_init(|| {
        let queue = Arc::new(PreviewWorkQueue::default());
        for index in 0..PREVIEW_WORKER_COUNT {
            let worker_queue = queue.clone();
            let _ = std::thread::Builder::new()
                .name(format!("preview-worker-{}", index + 1))
                .spawn(move || preview_worker_loop(worker_queue));
        }
        queue
    })
}

fn preview_worker_loop(queue: Arc<PreviewWorkQueue>) {
    loop {
        let job = queue.next_job();
        let key = preview_key(&job.path, job.longest_edge).ok();
        let pixels = key.as_ref().and_then(|key| {
            load_or_create_preview_pixels_if(
                &job.path,
                job.longest_edge,
                key,
                &job.purpose,
                job.cache_epoch,
                &|| queue.has_subscribers(&job.queue_key, job.job_id),
            )
            .ok()
            .flatten()
        });
        let subscribers = queue.take_subscribers(&job.queue_key, job.job_id);
        if job.cache_epoch != PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire) {
            continue;
        }
        let Some(key) = key else {
            continue;
        };
        let Some(pixels) = pixels else {
            continue;
        };
        for subscriber in subscribers {
            let key = key.clone();
            let pixels = pixels.clone();
            let target = subscriber.target;
            let cache_epoch = job.cache_epoch;
            let _ = subscriber.app.upgrade_in_event_loop(move |app| {
                if cache_epoch != PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire) {
                    return;
                }
                let image = PREVIEW_MEMORY_CACHE.with(|cache| {
                    let mut cache = cache.borrow_mut();
                    if let Some(image) = cache.get(&key) {
                        return image;
                    }
                    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        pixels.rgba.as_slice(),
                        pixels.width,
                        pixels.height,
                    );
                    let image = Image::from_rgba8(buffer);
                    cache.insert(key, image.clone(), pixels.bytes());
                    image
                });
                apply_gallery_preview(&app, &target, image);
            });
        }
    }
}

pub(super) fn initialize_preview_cache() {
    let _ = std::thread::Builder::new()
        .name("preview-cache-maintenance".to_string())
        .spawn(|| {
            cleanup_old_preview_versions();
            cleanup_preview_disk_cache();
        });
}

pub(super) fn clear_preview_memory_cache() {
    PREVIEW_CACHE_EPOCH.fetch_add(1, AtomicOrdering::AcqRel);
    if let Some(queue) = PREVIEW_WORK_QUEUE.get() {
        queue.cancel_all();
    }
    PREVIEW_MEMORY_CACHE.with(|cache| cache.borrow_mut().clear());
}

pub(super) fn clear_preview_disk_cache_files() -> u64 {
    let lock = DISK_CLEANUP_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    let root = preview_cache_root_dir();
    if !safe_managed_subdirectory(&root) {
        return 0;
    }
    let Ok(entries) = fs::read_dir(&root) else {
        return 0;
    };
    let mut removed_bytes = 0_u64;
    for entry in entries.flatten() {
        let directory = entry.path();
        if !is_preview_version_directory(&directory) || !safe_managed_subdirectory(&directory) {
            continue;
        }
        removed_bytes = removed_bytes.saturating_add(remove_preview_files_in(&directory));
        if directory != preview_cache_dir() {
            let _ = fs::remove_dir(&directory);
        }
    }
    DISK_WRITTEN_BYTES.store(0, AtomicOrdering::Release);
    removed_bytes
}

fn cleanup_old_preview_versions() {
    let lock = DISK_CLEANUP_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    let root = preview_cache_root_dir();
    if !safe_managed_subdirectory(&root) {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let current = preview_cache_dir();
    for entry in entries.flatten() {
        let directory = entry.path();
        if directory == current
            || !is_preview_version_directory(&directory)
            || !safe_managed_subdirectory(&directory)
        {
            continue;
        }
        remove_preview_files_in(&directory);
        let _ = fs::remove_dir(directory);
    }
}

fn is_preview_version_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .and_then(|name| name.strip_prefix('v'))
        .is_some_and(|version| !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit()))
}

fn remove_preview_files_in(directory: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut removed_bytes = 0_u64;
    for entry in entries.flatten() {
        let candidate = entry.path();
        let Some(path) = managed_preview_path(&candidate) else {
            continue;
        };
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if (!metadata.file_type().is_file() || metadata.file_type().is_symlink())
            || fs::remove_file(&path).is_err()
        {
            continue;
        }
        removed_bytes = removed_bytes.saturating_add(metadata.len());
        remove_indexed_file(&path);
    }
    removed_bytes
}

pub(super) fn request_gallery_preview(
    app: &AppWindow,
    path: &Path,
    purpose: PreviewPurpose,
    target: PreviewTarget,
) {
    if path.as_os_str().is_empty() {
        return;
    }
    let longest_edge = purpose
        .longest_edge()
        .clamp(64, PreviewPurpose::Viewer.longest_edge());
    enqueue_gallery_preview_with_retry(
        app.as_weak(),
        path.to_path_buf(),
        purpose,
        longest_edge,
        target,
        PREVIEW_RETRY_ATTEMPTS,
    );
}

fn enqueue_gallery_preview_with_retry(
    app: Weak<AppWindow>,
    path: PathBuf,
    purpose: PreviewPurpose,
    longest_edge: u32,
    target: PreviewTarget,
    attempts_left: u8,
) {
    let queue_key = preview_request_key(&path, longest_edge);
    let enqueued = preview_work_queue().enqueue(
        PreviewJob {
            job_id: 0,
            queue_key,
            path: path.clone(),
            longest_edge,
            purpose: purpose.cache_name().to_string(),
            cache_epoch: PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire),
        },
        PreviewSubscriber {
            app: app.clone(),
            target: target.clone(),
        },
    );
    if enqueued || attempts_left == 0 {
        return;
    }
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(window) = app.upgrade() else {
            return;
        };
        if !preview_collection_is_active(&window, target.collection) {
            return;
        }
        enqueue_gallery_preview_with_retry(
            window.as_weak(),
            path,
            purpose,
            longest_edge,
            target,
            attempts_left - 1,
        );
    });
}

fn preview_collection_is_active(app: &AppWindow, collection: PreviewCollection) -> bool {
    let page = app.global::<AppState>().get_page();
    match collection {
        PreviewCollection::Assets => page.as_str() == "assets",
        PreviewCollection::Generations => page.as_str() == "generation",
        PreviewCollection::Inspiration => page.as_str() == "inspiration",
    }
}

pub(super) fn cancel_gallery_previews(collection: PreviewCollection) {
    if let Some(queue) = PREVIEW_WORK_QUEUE.get() {
        queue.cancel_collection(collection);
    }
}

pub(super) fn load_preview_image(path: &Path, purpose: PreviewPurpose) -> Result<Image> {
    load_preview_image_with_edge_and_purpose(
        path,
        purpose.longest_edge(),
        purpose.cache_name(),
    )
}

pub(super) fn prepare_preview_image(
    path: &Path,
    purpose: PreviewPurpose,
) -> Result<PreparedPreview> {
    prepare_preview_image_if(path, purpose, || true)?.ok_or_else(|| {
        anyhow::anyhow!("缩略图请求已取消")
    })
}

pub(super) fn prepare_preview_image_if(
    path: &Path,
    purpose: PreviewPurpose,
    should_continue: impl Fn() -> bool,
) -> Result<Option<PreparedPreview>> {
    if !should_continue() {
        return Ok(None);
    }
    let longest_edge = purpose.longest_edge();
    let key = preview_key(path, longest_edge)?;
    let Some(pixels) = load_or_create_preview_pixels_if(
        path,
        longest_edge,
        &key,
        purpose.cache_name(),
        PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire),
        &should_continue,
    )? else {
        return Ok(None);
    };
    Ok(Some(PreparedPreview { key, pixels }))
}

pub(super) fn prepare_original_image_if(
    path: &Path,
    should_continue: impl Fn() -> bool,
) -> Result<Option<PreparedPreview>> {
    if !should_continue() {
        return Ok(None);
    }
    let key = preview_key(path, u32::MAX)?;
    let decode_lock = SOURCE_DECODE_LOCK.get_or_init(|| Mutex::new(()));
    let _decode_guard = decode_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !should_continue() {
        return Ok(None);
    }

    let (decoded, _) = decode_image_file(path)?;
    let decoded_pixels = u64::from(decoded.width()).saturating_mul(u64::from(decoded.height()));
    if decoded_pixels == 0 || decoded_pixels > MAX_SOURCE_PIXELS {
        anyhow::bail!("图片尺寸过大，无法安全加载到画布");
    }
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = Arc::new(PreviewPixels {
        rgba: Arc::new(rgba.as_raw().clone()),
        width,
        height,
    });
    if should_continue() {
        Ok(Some(PreparedPreview { key, pixels }))
    } else {
        Ok(None)
    }
}

pub(super) fn materialize_prepared_preview(prepared: PreparedPreview) -> Image {
    PREVIEW_MEMORY_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(image) = cache.get(&prepared.key) {
            return image;
        }
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            prepared.pixels.rgba.as_slice(),
            prepared.pixels.width,
            prepared.pixels.height,
        );
        let image = Image::from_rgba8(buffer);
        cache.insert(
            prepared.key,
            image.clone(),
            prepared.pixels.bytes(),
        );
        image
    })
}

pub(super) fn inspect_image_dimensions(path: &Path) -> Result<(u32, u32)> {
    let (width, height) = image::image_dimensions(path)
        .with_context(|| format!("无法读取图片尺寸 {}", path.display()))?;
    let pixel_count = u64::from(width).saturating_mul(u64::from(height));
    if pixel_count == 0 || pixel_count > MAX_SOURCE_PIXELS {
        anyhow::bail!("图片尺寸过大，无法安全处理");
    }
    Ok((width, height))
}

pub(super) fn load_preview_image_with_edge(path: &Path, longest_edge: u32) -> Result<Image> {
    load_preview_image_with_edge_and_purpose(path, longest_edge, "custom")
}

fn load_preview_image_with_edge_and_purpose(
    path: &Path,
    longest_edge: u32,
    purpose: &str,
) -> Result<Image> {
    let longest_edge = longest_edge.clamp(64, PreviewPurpose::Viewer.longest_edge());
    let key = preview_key(path, longest_edge)?;

    if let Some(image) = PREVIEW_MEMORY_CACHE.with(|cache| cache.borrow_mut().get(&key)) {
        return Ok(image);
    }

    let pixels = load_or_create_preview_pixels(
        path,
        longest_edge,
        &key,
        purpose,
        PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire),
    )?;
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        pixels.rgba.as_slice(),
        pixels.width,
        pixels.height,
    );
    let image = Image::from_rgba8(buffer);
    PREVIEW_MEMORY_CACHE
        .with(|cache| cache.borrow_mut().insert(key, image.clone(), pixels.bytes()));
    Ok(image)
}

fn load_or_create_preview_pixels(
    path: &Path,
    longest_edge: u32,
    key: &str,
    purpose: &str,
    cache_epoch: u64,
) -> Result<Arc<PreviewPixels>> {
    load_or_create_preview_pixels_if(
        path,
        longest_edge,
        key,
        purpose,
        cache_epoch,
        &|| true,
    )?
    .ok_or_else(|| anyhow::anyhow!("缩略图请求已取消"))
}

fn load_or_create_preview_pixels_if(
    path: &Path,
    longest_edge: u32,
    key: &str,
    purpose: &str,
    cache_epoch: u64,
    should_continue: &dyn Fn() -> bool,
) -> Result<Option<Arc<PreviewPixels>>> {
    let cache_path = preview_cache_dir().join(format!("{key}.png"));
    if !should_continue() {
        return Ok(None);
    }
    if let Some(pixels) = try_load_cached_preview_pixels(
        path,
        &cache_path,
        purpose,
        longest_edge,
        cache_epoch,
    )? {
        return Ok(Some(pixels));
    }

    let decode_lock = SOURCE_DECODE_LOCK.get_or_init(|| Mutex::new(()));
    let _decode_guard = decode_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !should_continue()
        || cache_epoch != PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire)
    {
        return Ok(None);
    }
    // A different loader may have produced this thumbnail while this request was
    // waiting for the global decode slot. Rechecking here prevents stale canvas
    // refresh threads from decoding the same full-resolution source repeatedly.
    if let Some(pixels) = try_load_cached_preview_pixels(
        path,
        &cache_path,
        purpose,
        longest_edge,
        cache_epoch,
    )? {
        return Ok(Some(pixels));
    }
    let known_dimensions = match inspect_image_dimensions(path) {
        Ok(dimensions) => Some(dimensions),
        Err(_error) if macos_native_preview_source(path) => None,
        Err(error) => return Err(error),
    };
    if known_dimensions.is_some_and(|(width, height)| {
        u64::from(width).saturating_mul(u64::from(height)) > MAX_PREVIEW_SOURCE_PIXELS
    }) {
        anyhow::bail!("图片尺寸过大，无法安全生成缩略图");
    }
    let (decoded, _) = decode_image_file(path)?;
    let decoded_pixels = u64::from(decoded.width()).saturating_mul(u64::from(decoded.height()));
    if decoded_pixels == 0 || decoded_pixels > MAX_PREVIEW_SOURCE_PIXELS {
        anyhow::bail!("图片尺寸过大，无法安全生成缩略图");
    }
    let preview = if decoded.width().max(decoded.height()) > longest_edge {
        decoded.thumbnail(longest_edge, longest_edge)
    } else {
        decoded
    };
    let rgba = preview.to_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = Arc::new(PreviewPixels {
        rgba: Arc::new(rgba.as_raw().clone()),
        width,
        height,
    });

    if !should_continue()
        || cache_epoch != PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire)
    {
        return Ok(None);
    }

    if cache_path.parent().is_some_and(ensure_managed_subdirectory) {
        let encoded = encode_png_rgba(&rgba, width, height)?;
        let mut written_bytes = 0_u64;
        {
            let lock = DISK_CLEANUP_LOCK.get_or_init(|| Mutex::new(()));
            let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
            if should_continue()
                && cache_epoch == PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire)
                && atomic_write_file(&cache_path, &encoded).is_ok()
                && managed_preview_path(&cache_path).is_some()
            {
                written_bytes = encoded.len() as u64;
                index_preview_relation(path, &cache_path, purpose, longest_edge, "ready");
            }
        }
        if written_bytes > 0
            && DISK_WRITTEN_BYTES
                .fetch_add(written_bytes, AtomicOrdering::Relaxed)
                .saturating_add(written_bytes)
                >= DISK_CLEANUP_WRITE_BYTES
        {
            cleanup_preview_disk_cache();
        }
    }
    if should_continue()
        && cache_epoch == PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire)
    {
        Ok(Some(pixels))
    } else {
        Ok(None)
    }
}

fn try_load_cached_preview_pixels(
    source_path: &Path,
    cache_path: &Path,
    purpose: &str,
    longest_edge: u32,
    cache_epoch: u64,
) -> Result<Option<Arc<PreviewPixels>>> {
    if managed_preview_path(cache_path).is_none() {
        return Ok(None);
    }
    let lock = DISK_CLEANUP_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    if cache_epoch != PREVIEW_CACHE_EPOCH.load(AtomicOrdering::Acquire)
        || !cache_path.is_file()
    {
        return Ok(None);
    }
    match load_cached_preview_pixels(cache_path) {
        Ok(pixels) => {
            index_preview_relation(source_path, cache_path, purpose, longest_edge, "ready");
            Ok(Some(pixels))
        }
        Err(_) => {
            let _ = fs::remove_file(cache_path);
            remove_indexed_file(cache_path);
            Ok(None)
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_native_preview_source(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "heic" | "heif"))
}

#[cfg(not(target_os = "macos"))]
fn macos_native_preview_source(_path: &Path) -> bool {
    false
}

fn load_cached_preview_pixels(path: &Path) -> Result<Arc<PreviewPixels>> {
    let decoded = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0
        || height == 0
        || width > PreviewPurpose::Viewer.longest_edge()
        || height > PreviewPurpose::Viewer.longest_edge()
    {
        anyhow::bail!("缩略图缓存尺寸无效");
    }
    Ok(Arc::new(PreviewPixels {
        rgba: Arc::new(rgba.into_raw()),
        width,
        height,
    }))
}

fn index_preview_relation(
    source_path: &Path,
    cache_path: &Path,
    _purpose: &str,
    longest_edge: u32,
    status: &str,
) {
    let Some(index) = global_file_index() else {
        return;
    };
    let Ok(source) = index.register_file(&managed_file_registration(source_path)) else {
        return;
    };
    let Ok(preview) = index.register_file(&managed_file_registration(cache_path)) else {
        return;
    };
    let Ok(metadata) = fs::metadata(source_path) else {
        return;
    };
    let source_mtime_ns = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64;
    let key = PreviewKey {
        source_file_id: source.id,
        // The file bytes are keyed by source fingerprint and edge. Purposes that use the
        // same edge deliberately share one physical thumbnail and one relationship.
        purpose: "thumbnail".to_string(),
        longest_edge,
        cache_version: PREVIEW_CACHE_VERSION,
    };
    let previous = index.find_preview(&key).ok().flatten();
    let registration = PreviewRegistration {
        key,
        preview_file_id: preview.id,
        source_size: metadata.len(),
        source_mtime_ns,
        status: status.to_string(),
        last_accessed_at: preview_timestamp_millis(),
    };
    if index.upsert_preview(&registration).is_err() {
        return;
    }
    if let Some(previous) = previous.filter(|record| record.preview_file_id != preview.id) {
        if let Some(path) = managed_preview_path(&previous.preview_path) {
            let _ = fs::remove_file(path);
        }
        let _ = index.delete_file(previous.preview_file_id);
    }
}

fn preview_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

pub(super) fn invalidate_previews_for_source(source_path: &Path) {
    let keys = [
        PreviewPurpose::Reference.longest_edge(),
        PreviewPurpose::Gallery.longest_edge(),
        PreviewPurpose::Canvas.longest_edge(),
        PreviewPurpose::Viewer.longest_edge(),
    ]
    .into_iter()
    .filter_map(|edge| preview_key(source_path, edge).ok())
    .collect::<Vec<_>>();

    if let Some(index) = global_file_index() {
        if let Ok(Some(source)) = index.find_file_by_path(source_path) {
            if let Ok(records) = index.delete_previews_for_source(source.id) {
                for record in records {
                    if let Some(path) = managed_preview_path(&record.preview_path) {
                        let _ = fs::remove_file(path);
                    }
                    let _ = index.delete_file(record.preview_file_id);
                }
            }
        }
    }
    for key in &keys {
        let cache_path = preview_cache_dir().join(format!("{key}.png"));
        if let Some(path) = managed_preview_path(&cache_path) {
            let _ = fs::remove_file(&path);
            remove_indexed_file(&path);
        }
    }
    PREVIEW_MEMORY_CACHE.with(|cache| cache.borrow_mut().remove(&keys));
}

fn preview_cache_dir() -> PathBuf {
    preview_cache_root_dir().join(format!("v{PREVIEW_CACHE_VERSION}"))
}

fn preview_cache_root_dir() -> PathBuf {
    app_data_dir().join("cache").join("previews")
}

fn preview_request_key(path: &Path, longest_edge: u32) -> String {
    let normalized_path = path.to_string_lossy().to_string();
    #[cfg(windows)]
    let normalized_path = normalized_path.replace('\\', "/").to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(PREVIEW_CACHE_VERSION.to_le_bytes());
    hasher.update(normalized_path.as_bytes());
    hasher.update(longest_edge.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

fn preview_key(path: &Path, longest_edge: u32) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let metadata =
        fs::metadata(&canonical).with_context(|| format!("无法读取图片文件 {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("图片路径不是文件");
    }
    let modified = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let normalized_path = canonical.to_string_lossy().to_string();
    #[cfg(windows)]
    let normalized_path = normalized_path.replace('\\', "/").to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(PREVIEW_CACHE_VERSION.to_le_bytes());
    hasher.update(normalized_path.as_bytes());
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(modified.to_le_bytes());
    hasher.update(longest_edge.to_le_bytes());
    let mut file = fs::File::open(&canonical)
        .with_context(|| format!("无法读取图片文件 {}", path.display()))?;
    let mut sample = [0_u8; 4096];
    let first_len = file.read(&mut sample)?;
    hasher.update((first_len as u64).to_le_bytes());
    hasher.update(&sample[..first_len]);
    if metadata.len() > sample.len() as u64 {
        file.seek(SeekFrom::End(-(sample.len() as i64)))?;
        let last_len = file.read(&mut sample)?;
        hasher.update((last_len as u64).to_le_bytes());
        hasher.update(&sample[..last_len]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn cleanup_preview_disk_cache() {
    let lock = DISK_CLEANUP_LOCK.get_or_init(|| Mutex::new(()));
    let Ok(_guard) = lock.try_lock() else {
        return;
    };
    let directory = preview_cache_dir();
    if !safe_managed_subdirectory(&directory) {
        return;
    }
    let Ok(entries) = fs::read_dir(&directory) else {
        return;
    };
    let mut files = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = managed_preview_path(&entry.path())?;
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then_some((
                path,
                metadata.len(),
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            ))
        })
        .collect::<Vec<_>>();
    let mut total = files.iter().map(|(_, size, _)| *size).sum::<u64>();
    if total <= DISK_CACHE_LIMIT_BYTES {
        DISK_WRITTEN_BYTES.store(0, AtomicOrdering::Release);
        return;
    }
    let indexed_access = global_file_index()
        .and_then(|index| index.least_recently_used_previews(100_000).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|record| (preview_cache_path_identity(&record.preview_path), record.last_accessed_at))
        .collect::<BTreeMap<_, _>>();
    files.sort_by_key(|(path, _, modified)| {
        indexed_access
            .get(&preview_cache_path_identity(path))
            .copied()
            .unwrap_or_else(|| {
            modified
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(i64::MAX as u128) as i64
            })
    });
    for (path, size, _) in files {
        if total <= DISK_CACHE_TARGET_BYTES {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            remove_indexed_file(&path);
        }
    }
    DISK_WRITTEN_BYTES.store(0, AtomicOrdering::Release);
}

fn preview_cache_path_identity(path: &Path) -> String {
    let value = path.to_string_lossy().to_string();
    #[cfg(windows)]
    let value = value.replace('/', "\\").to_lowercase();
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_purposes_keep_ui_images_bounded() {
        assert_eq!(PreviewPurpose::Reference.longest_edge(), 256);
        assert_eq!(PreviewPurpose::Gallery.longest_edge(), 384);
        assert_eq!(PreviewPurpose::Canvas.longest_edge(), 1024);
        assert_eq!(PreviewPurpose::Viewer.longest_edge(), 2048);
    }

    #[test]
    fn original_image_preparation_preserves_source_dimensions() {
        let directory = tempfile::tempdir().expect("temporary original image directory");
        let path = directory.path().join("source.png");
        image::RgbaImage::from_pixel(73, 211, image::Rgba([12, 34, 56, 255]))
            .save(&path)
            .expect("save original image");

        let prepared = prepare_original_image_if(&path, || true)
            .expect("prepare original image")
            .expect("original image should be available");

        assert_eq!(prepared.pixels.width, 73);
        assert_eq!(prepared.pixels.height, 211);
    }

    #[test]
    fn memory_cache_evicts_old_entries_over_budget() {
        let mut cache = PreviewMemoryCache::default();
        cache.insert(
            "old".to_string(),
            Image::default(),
            MEMORY_CACHE_LIMIT_BYTES,
        );
        cache.insert("new".to_string(), Image::default(), 4);
        assert!(cache.get("old").is_none());
        assert!(cache.get("new").is_some());
        assert!(cache.bytes <= MEMORY_CACHE_LIMIT_BYTES);
    }

    #[test]
    fn preview_queue_is_bounded_and_deduplicates_equal_keys() {
        let queue = PreviewWorkQueue::default();
        let subscriber = || PreviewSubscriber {
            app: Weak::<AppWindow>::default(),
            target: PreviewTarget {
                collection: PreviewCollection::Assets,
                asset_id: "asset".to_string(),
                source_path: "source.png".to_string(),
                flat_row: None,
                group_row: None,
                group_item_row: None,
            },
        };
        assert!(queue.enqueue(
            PreviewJob {
                job_id: 0,
                queue_key: "same".to_string(),
                path: PathBuf::from("source.png"),
                longest_edge: 512,
                purpose: "gallery".to_string(),
                cache_epoch: 1,
            },
            subscriber(),
        ));
        assert!(queue.enqueue(
            PreviewJob {
                job_id: 0,
                queue_key: "same".to_string(),
                path: PathBuf::from("source.png"),
                longest_edge: 512,
                purpose: "gallery".to_string(),
                cache_epoch: 1,
            },
            subscriber(),
        ));
        let state = queue.state.lock().expect("queue");
        assert_eq!(state.queue.len(), 1);
        assert_eq!(state.subscribers["same"].subscribers.len(), 1);
    }

    #[test]
    fn cancelled_job_cannot_consume_new_subscribers_with_the_same_key() {
        let queue = PreviewWorkQueue::default();
        let subscriber = || PreviewSubscriber {
            app: Weak::<AppWindow>::default(),
            target: PreviewTarget {
                collection: PreviewCollection::Assets,
                asset_id: "asset".to_string(),
                source_path: "source.png".to_string(),
                flat_row: None,
                group_row: None,
                group_item_row: None,
            },
        };
        let job = || PreviewJob {
            job_id: 0,
            queue_key: "same".to_string(),
            path: PathBuf::from("source.png"),
            longest_edge: 512,
            purpose: "gallery".to_string(),
            cache_epoch: 1,
        };

        assert!(queue.enqueue(job(), subscriber()));
        let cancelled_job = queue.next_job();
        queue.cancel_collection(PreviewCollection::Assets);
        assert!(queue.enqueue(job(), subscriber()));
        assert!(queue
            .take_subscribers(&cancelled_job.queue_key, cancelled_job.job_id)
            .is_empty());

        let replacement_job = queue.next_job();
        assert_eq!(
            queue
                .take_subscribers(&replacement_job.queue_key, replacement_job.job_id)
                .len(),
            1
        );
    }
}
