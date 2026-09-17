use super::*;

use sha2::{Digest, Sha256};
use std::io::{Read, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;

const VIDEO_STREAM_CHUNK: usize = 64 * 1024;
const VIDEO_REQUEST_HEADER_LIMIT: usize = 8 * 1024;
const VIDEO_REQUEST_DEADLINE: Duration = Duration::from_secs(3);
const VIDEO_SOCKET_TIMEOUT: Duration = Duration::from_millis(200);

struct OwnedVideoSource {
    authority: Arc<NamespaceStorageAuthority>,
    file: Mutex<NamespaceManagedFile>,
    size: u64,
    receipt: SavedVideoOutput,
}
impl OwnedVideoSource {
    fn read_chunk(&self, offset: u64, count: usize) -> Result<Vec<u8>> {
        anyhow::ensure!(count <= VIDEO_STREAM_CHUNK && offset <= self.size
            && count as u64 <= self.size - offset, "invalid video chunk");
        let mut file = self.file.lock().map_err(|_| anyhow!("video source unavailable"))?;
        let metadata = self.authority.inspect_regular(&file)?;
        anyhow::ensure!(metadata.link_count == 1 && metadata.byte_size == self.size, "video source changed");
        self.authority.with_regular_reader(&mut file, |reader| {
            reader.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0; count];
            reader.read_exact(&mut bytes)?;
            Ok(bytes)
        })
    }
}
struct PreparedVideoPlayback {
    url: reqwest::Url,
    player_url: reqwest::Url,
    cancel: Arc<AtomicBool>,
    source: Arc<OwnedVideoSource>,
}
impl Drop for PreparedVideoPlayback {
    fn drop(&mut self) { self.cancel.store(true, Ordering::SeqCst); }
}
struct PlayerWorker {
    lease: NamespaceLease,
    cancel: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}
#[derive(Default)]
struct PlayerWorkers { closing: bool, failed: bool, workers: Vec<PlayerWorker> }
fn player_workers() -> &'static Mutex<PlayerWorkers> {
    static WORKERS: std::sync::OnceLock<Mutex<PlayerWorkers>> = std::sync::OnceLock::new();
    WORKERS.get_or_init(|| Mutex::new(PlayerWorkers::default()))
}
fn spawn_video_player_worker(
    lease: NamespaceLease, cancel: Arc<AtomicBool>, work: impl FnOnce() + Send + 'static,
) -> Result<()> {
    let mut registry = player_workers().lock().map_err(|_| anyhow!("player worker registry unavailable"))?;
    anyhow::ensure!(!registry.closing, "player shutdown has started");
    let handle = std::thread::Builder::new().name("owned-video".into()).spawn(work)?;
    registry.workers.push(PlayerWorker { lease, cancel, handle });
    Ok(())
}
fn reap_video_player_workers() -> Result<()> {
    let finished = {
        let mut registry = player_workers().lock().map_err(|_| anyhow!("player worker registry unavailable"))?;
        let mut finished = Vec::new();
        let mut i = 0;
        while i < registry.workers.len() {
            if registry.workers[i].handle.is_finished() { finished.push(registry.workers.swap_remove(i)); }
            else { i += 1; }
        }
        finished
    };
    let mut failed = false;
    for worker in finished { failed |= worker.handle.join().is_err(); }
    if failed { player_workers().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).failed = true; }
    anyhow::ensure!(!failed, "owned video worker failed");
    Ok(())
}
fn cancel_video_player_workers(lease: &NamespaceLease) {
    let registry = player_workers().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for worker in &registry.workers {
        if &worker.lease == lease { worker.cancel.store(true, Ordering::SeqCst); }
    }
}
fn schedule_video_worker_reap() {
    slint::Timer::single_shot(Duration::from_millis(50), || {
        let _ = reap_video_player_workers();
        let pending = player_workers().lock().map(|registry| registry.workers.iter()
            .any(|worker| worker.cancel.load(Ordering::SeqCst))).unwrap_or(false);
        if pending { schedule_video_worker_reap(); }
    });
}
#[cfg(test)]
fn core_video_player_worker_count(lease: &NamespaceLease) -> usize {
    player_workers().lock().unwrap().workers.iter().filter(|worker| &worker.lease == lease).count()
}

/// Fixture-only drain: never closes the process-global registry or reopens it.
/// Joining outside its lock lets a cancelled parent register its final child;
/// repeat extraction until that original lease has no remaining real handles.
#[cfg(test)]
pub(super) fn drain_video_player_workers_for_lease_for_test(lease: &NamespaceLease) -> Result<()> {
    clear_active_video_player(Some(lease));
    loop {
        let workers = {
            let mut registry = player_workers().lock().map_err(|_| anyhow!("player worker registry unavailable"))?;
            let mut workers = Vec::new();
            let mut index = registry.workers.len();
            while index > 0 {
                index -= 1;
                if &registry.workers[index].lease == lease {
                    registry.workers[index].cancel.store(true, Ordering::SeqCst);
                    workers.push(registry.workers.swap_remove(index));
                }
            }
            workers
        };
        if workers.is_empty() { break; }
        let mut failed = false;
        for worker in workers { failed |= worker.handle.join().is_err(); }
        if failed {
            player_workers().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).failed = true;
        }
    }
    anyhow::ensure!(!player_workers().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).failed,
        "owned video worker failed during fixture drain");
    Ok(())
}

// Shutdown-only: owner calls after stopping player UI, outside ordinary/UI short locks.
// Every worker is registered before exposure. The closing flag forbids late child spawns.
pub(super) fn drain_video_player_workers_for_shutdown() -> Result<()> {
    clear_active_video_player(None);
    let workers = {
        let mut registry = player_workers().lock().map_err(|_| anyhow!("player worker registry unavailable"))?;
        registry.closing = true;
        for worker in &registry.workers { worker.cancel.store(true, Ordering::SeqCst); }
        std::mem::take(&mut registry.workers)
    };
    let mut failed = player_workers().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).failed;
    for worker in workers { failed |= worker.handle.join().is_err(); }
    anyhow::ensure!(!failed, "owned video worker failed during shutdown");
    Ok(())
}

// This URL contains no filesystem name, identity, query or arbitrary proxy target.
fn validated_local_video_url(
    address: std::net::SocketAddr,
    token: &str,
    endpoint: &str,
) -> Result<reqwest::Url> {
    anyhow::ensure!(address.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        && address.port() != 0 && token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid owned player endpoint");
    anyhow::ensure!(matches!(endpoint, "media" | "player"), "invalid owned player route");
    Ok(reqwest::Url::parse(&format!(
        "http://127.0.0.1:{}/{endpoint}/{token}",
        address.port()
    ))?)
}

fn prepare_video_playback(persistence: PrivatePersistence, output: SavedVideoOutput) -> Result<PreparedVideoPlayback> {
    let cancel = Arc::new(AtomicBool::new(false));
    prepare_video_playback_with_cancel(persistence, output, cancel)
}
fn prepare_video_playback_with_cancel(
    persistence: PrivatePersistence, output: SavedVideoOutput, cancel: Arc<AtomicBool>,
) -> Result<PreparedVideoPlayback> {
    let activity = persistence.begin_activity()?;
    anyhow::ensure!(!cancel.load(Ordering::SeqCst) && !activity.is_quiescing(), "video preparation cancelled");
    let directory = persistence.lease().namespace.path(ManagedUserArea::Videos);
    let path = Path::new(&output.source_path);
    let relative = path.strip_prefix(&directory).map_err(|_| anyhow!("video is outside captured namespace"))?;
    let mime = match path.extension().and_then(|value| value.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp4") => "video/mp4", Some("webm") => "video/webm", Some("mov") => "video/quicktime",
        _ => return Err(anyhow!("unsupported local video")),
    };
    anyhow::ensure!(output.size_bytes > 0 && output.size_bytes <= i64::MAX as u64
        && output.sha256.len() == 64 && output.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid retained video receipt");
    let key = ManagedFileKey::new(ManagedUserArea::Videos, relative.to_str().ok_or_else(|| anyhow!("invalid video name"))?)?;
    let source = {
        let _effect = persistence.begin_effect()?;
        let authority = persistence.storage_authority()?;
        let file = authority.open_existing_regular(&key)?;
        let metadata = authority.inspect_regular(&file)?;
        anyhow::ensure!(metadata.link_count == 1 && metadata.byte_size == output.size_bytes, "video receipt size mismatch");
        Arc::new(OwnedVideoSource { authority, file: Mutex::new(file), size: output.size_bytes, receipt: output.clone() })
    };
    let mut digest = Sha256::new();
    let mut offset = 0;
    while offset < source.size {
        anyhow::ensure!(!cancel.load(Ordering::SeqCst) && !activity.is_quiescing(), "video preparation cancelled");
        let _effect = persistence.begin_effect()?;
        let count = (source.size - offset).min(VIDEO_STREAM_CHUNK as u64) as usize;
        digest.update(source.read_chunk(offset, count)?);
        offset += count as u64;
    }
    anyhow::ensure!(format!("{:x}", digest.finalize()).eq_ignore_ascii_case(&output.sha256), "video receipt digest mismatch");
    anyhow::ensure!(!cancel.load(Ordering::SeqCst) && !activity.is_quiescing() && persistence.is_current(), "video preparation cancelled");
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let token = Uuid::new_v4().simple().to_string();
    let address = listener.local_addr()?;
    let url = validated_local_video_url(address, &token, "media")?;
    let player_url = validated_local_video_url(address, &token, "player")?;
    let port = url.port().ok_or_else(|| anyhow!("missing player port"))?;
    let worker_source = source.clone();
    let worker_cancel = cancel.clone();
    let worker_persistence = persistence.clone();
    spawn_video_player_worker(persistence.lease().clone(), cancel.clone(), move || {
        run_video_listener(listener, worker_persistence, worker_source, worker_cancel, token, port, mime);
    })?;
    Ok(PreparedVideoPlayback { url, player_url, cancel, source })
}

fn run_video_listener(
    listener: TcpListener, persistence: PrivatePersistence, source: Arc<OwnedVideoSource>,
    cancel: Arc<AtomicBool>, token: String, port: u16, mime: &'static str,
) {
    // One accepted connection at a time: no unbounded per-request worker queue.
    while !cancel.load(Ordering::SeqCst) && persistence.is_current() {
        match listener.accept() {
            Ok((mut stream, address)) => {
                if !address.ip().is_loopback() { continue; }
                if stream.set_read_timeout(Some(VIDEO_SOCKET_TIMEOUT)).is_err()
                    || stream.set_write_timeout(Some(VIDEO_SOCKET_TIMEOUT)).is_err() { continue; }
                let _ = serve_video_connection(
                    &mut stream,
                    &persistence,
                    &source,
                    &cancel,
                    &token,
                    port,
                    mime,
                );
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break,
        }
    }
    cancel.store(true, Ordering::SeqCst);
}
fn read_video_request(stream: &mut TcpStream, cancel: &AtomicBool) -> Result<String> {
    let deadline = Instant::now() + VIDEO_REQUEST_DEADLINE;
    let mut headers = Vec::with_capacity(1024);
    let mut byte = [0; 1];
    while headers.len() < VIDEO_REQUEST_HEADER_LIMIT && Instant::now() < deadline {
        anyhow::ensure!(!cancel.load(Ordering::SeqCst), "player request cancelled");
        match stream.read(&mut byte) {
            Ok(0) => return Err(anyhow!("incomplete player request")),
            Ok(_) => {
                headers.push(byte[0]);
                if headers.ends_with(b"\r\n\r\n") {
                    anyhow::ensure!(headers.is_ascii(), "invalid player headers");
                    return Ok(String::from_utf8(headers)?);
                }
            }
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(anyhow!("player request headers exceeded bounds"))
}
fn video_range(value: Option<&str>, size: u64) -> Result<(u64, u64, bool)> {
    anyhow::ensure!(size > 0 && size <= i64::MAX as u64, "invalid video size");
    let Some(value) = value else { return Ok((0, size, false)); };
    let value = value.strip_prefix("bytes=").ok_or_else(|| anyhow!("invalid range"))?;
    anyhow::ensure!(!value.contains(','), "multiple ranges unsupported");
    let (start, end) = value.split_once('-').ok_or_else(|| anyhow!("invalid range"))?;
    let decimal = |value: &str| -> Result<u64> {
        anyhow::ensure!(!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()), "invalid range");
        Ok(value.parse()?)
    };
    let (start, end) = if start.is_empty() {
        let suffix = decimal(end)?;
        anyhow::ensure!(suffix > 0, "invalid suffix range");
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start = decimal(start)?;
        let end = if end.is_empty() { size - 1 } else { decimal(end)?.min(size - 1) };
        (start, end)
    };
    anyhow::ensure!(start < size && end >= start, "unsatisfiable range");
    let length = end.checked_sub(start).and_then(|value| value.checked_add(1)).ok_or_else(|| anyhow!("range overflow"))?;
    Ok((start, length, true))
}
fn write_video_bytes(stream: &mut TcpStream, bytes: &[u8], current: impl Fn() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut offset = 0;
    while offset < bytes.len() {
        anyhow::ensure!(current(), "video write cancelled");
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "video write deadline").into());
        }
        let written = stream.write(&bytes[offset..])?;
        anyhow::ensure!(written > 0, "video connection closed");
        offset += written;
    }
    Ok(())
}
fn empty_video_response(stream: &mut TcpStream, code: u16, size: Option<u64>) -> Result<()> {
    let extra = size.map(|size| format!("Content-Range: bytes */{size}\r\n")).unwrap_or_default();
    let headers = format!("HTTP/1.1 {code} Rejected\r\nContent-Type: application/octet-stream\r\nContent-Length: 0\r\nConnection: close\r\nCache-Control: no-store\r\n{extra}\r\n");
    write_video_bytes(stream, headers.as_bytes(), || true)?;
    Ok(())
}
fn serve_video_connection(
    stream: &mut TcpStream, persistence: &PrivatePersistence, source: &OwnedVideoSource,
    cancel: &AtomicBool, token: &str, port: u16, mime: &str,
) -> Result<()> {
    let request = read_video_request(stream, cancel)?;
    let mut lines = request.split("\r\n");
    let first = lines.next().unwrap_or_default().split(' ').collect::<Vec<_>>();
    let media_path = format!("/media/{token}");
    let player_path = format!("/player/{token}");
    if first.len() != 3
        || !matches!(first[0], "GET" | "HEAD")
        || !matches!(first[1], path if path == media_path || path == player_path)
        || first[2] != "HTTP/1.1"
    {
        return empty_video_response(stream, 404, None);
    }
    let mut host = None;
    let mut range = None;
    let mut count = 0;
    for line in lines.take_while(|line| !line.is_empty()) {
        count += 1;
        if count > 64 { return empty_video_response(stream, 400, None); }
        let Some((name, value)) = line.split_once(':') else { return empty_video_response(stream, 400, None); };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "host" if host.is_none() => host = Some(value),
            "range" if range.is_none() => range = Some(value),
            "host" | "range" | "transfer-encoding" => return empty_video_response(stream, 400, None),
            "content-length" if value != "0" => return empty_video_response(stream, 400, None),
            _ => {}
        }
    }
    if host != Some(format!("127.0.0.1:{port}").as_str()) { return empty_video_response(stream, 404, None); }
    if first[1] == player_path {
        if range.is_some() { return empty_video_response(stream, 400, None); }
        let media_url = validated_local_video_url(
            std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)),
            token,
            "media",
        )?;
        let html = player_html(&media_url)?;
        let body = html.as_bytes();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; media-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let (activity, _effect) = persistence.begin_effect()?;
        anyhow::ensure!(
            !cancel.load(Ordering::SeqCst) && !activity.is_quiescing(),
            "video player page cancelled"
        );
        write_video_bytes(stream, headers.as_bytes(), || {
            !cancel.load(Ordering::SeqCst) && !activity.is_quiescing()
        })?;
        if first[0] == "GET" {
            write_video_bytes(stream, body, || {
                !cancel.load(Ordering::SeqCst) && !activity.is_quiescing()
            })?;
        }
        return Ok(());
    }
    let (start, length, partial) = match video_range(range, source.size) {
        Ok(range) => range, Err(_) => return empty_video_response(stream, 416, Some(source.size)),
    };
    let end = start.checked_add(length).filter(|end| *end <= source.size).ok_or_else(|| anyhow!("range overflow"))?;
    {
        let (activity, _effect) = persistence.begin_effect()?;
        anyhow::ensure!(!cancel.load(Ordering::SeqCst) && !activity.is_quiescing(), "video stream cancelled");
        let range_header = if partial { format!("Content-Range: bytes {}-{}/{}\r\n", start, end.checked_sub(1).ok_or_else(|| anyhow!("range overflow"))?, source.size) } else { String::new() };
        let status = if partial { "206 Partial Content" } else { "200 OK" };
        let headers = format!("HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {length}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n{range_header}\r\n");
        write_video_bytes(stream, headers.as_bytes(), || !cancel.load(Ordering::SeqCst) && !activity.is_quiescing())?;
    }
    if first[0] == "HEAD" { return Ok(()); }
    let mut offset = start;
    while offset < end {
        let (activity, _effect) = persistence.begin_effect()?;
        anyhow::ensure!(!cancel.load(Ordering::SeqCst) && !activity.is_quiescing(), "video stream cancelled");
        let count = (end-offset).min(VIDEO_STREAM_CHUNK as u64) as usize;
        // The retained source lock/post-validation ends BEFORE any socket operation.
        let bytes = match source.read_chunk(offset, count) {
            Ok(bytes) => bytes,
            Err(error) => { cancel.store(true, Ordering::SeqCst); return Err(error); }
        };
        anyhow::ensure!(!cancel.load(Ordering::SeqCst) && !activity.is_quiescing(), "video stream cancelled");
        write_video_bytes(stream, &bytes, || !cancel.load(Ordering::SeqCst) && !activity.is_quiescing())?;
        offset += count as u64;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlayerCommand {
    Download,
    OpenFolder,
    Regenerate,
    Ready,
    PlaybackError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VideoPlayerBounds {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl VideoPlayerBounds {
    fn from_logical(x: f32, y: f32, width: f32, height: f32, scale_factor: f32) -> Option<Self> {
        if ![x, y, width, height, scale_factor]
            .iter()
            .all(|value| value.is_finite())
            || scale_factor <= 0.0
            || width < 2.0
            || height < 2.0
        {
            return None;
        }
        Some(Self {
            x: (x.max(0.0) * scale_factor).round() as u32,
            y: (y.max(0.0) * scale_factor).round() as u32,
            width: (width * scale_factor).round().max(1.0) as u32,
            height: (height * scale_factor).round().max(1.0) as u32,
        })
    }
}


fn parse_player_command(body: &str) -> Option<PlayerCommand> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.as_object()?.len() != 1 {
        return None;
    }
    match value.get("command")?.as_str()? {
        "download" => Some(PlayerCommand::Download),
        "open_folder" => Some(PlayerCommand::OpenFolder),
        "regenerate" => Some(PlayerCommand::Regenerate),
        "player_ready" => Some(PlayerCommand::Ready),
        "playback_error" => Some(PlayerCommand::PlaybackError),
        _ => None,
    }
}

fn player_html(video_url: &reqwest::Url) -> Result<String> {
    let encoded_url = serde_json::to_string(video_url.as_str()).context("视频地址编码失败")?;
    Ok(include_str!("video_player/player.html").replace("__VIDEO_SRC_JSON__", &encoded_url))
}


struct ActiveVideoSession {
    context: AppContext,
    persistence: PrivatePersistence,
    output: SavedVideoOutput,
    cancel: Arc<AtomicBool>,
    bounds: VideoPlayerBounds,
    playback: Option<PreparedVideoPlayback>,
}
thread_local! {
    static ACTIVE_VIDEO_SESSION: RefCell<Option<ActiveVideoSession>> = const { RefCell::new(None) };
}
fn player_output_is_current(context: &AppContext, persistence: &PrivatePersistence, output: &SavedVideoOutput) -> bool {
    let store = context.store.borrow();
    store.private_persistence.as_ref().is_some_and(|current| current.same_binding(persistence))
        && store.video_outputs.get(&format!("{}:{}", output.server_task_id, output.file_id)) == Some(output)
}
fn apply_video_player<R>(
    context: &AppContext, persistence: &PrivatePersistence, output: &SavedVideoOutput, apply: impl FnOnce() -> R,
) -> Option<R> {
    let _activity = persistence.begin_activity().ok()?;
    if !player_output_is_current(context, persistence, output) { return None; }
    context.apply_user_completion(persistence.lease(), apply).ok()
}
fn clear_active_video_player(lease: Option<&NamespaceLease>) {
    let removed = ACTIVE_VIDEO_SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|active| lease.map_or(true, |lease| active.persistence.lease() == lease)) {
            slot.take()
        } else { None }
    });
    if let Some(active) = removed {
        active.cancel.store(true, Ordering::SeqCst);
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        desktop_video_player::close_exact(&active.cancel);
    }
}
pub(super) fn close_video_player_for_retirement(lease: &NamespaceLease) {
    // Revocation cleanup is allowed AFTER ordinary admission has retired/tripped.
    // Matching the exact lease prevents late A cleanup from closing B.
    cancel_video_player_workers(lease);
    clear_active_video_player(Some(lease));
    schedule_video_worker_reap();
}
pub(super) fn close_video_player_for_required_upgrade() {
    let lease = ACTIVE_VIDEO_SESSION.with(|slot| slot.borrow().as_ref().map(|active| active.persistence.lease().clone()));
    if let Some(lease) = lease { close_video_player_for_retirement(&lease); }
}
pub(super) fn close_video_player() {
    let captured = ACTIVE_VIDEO_SESSION.with(|slot| slot.borrow().as_ref().map(|active|
        (active.context.clone(), active.persistence.clone(), active.output.clone())));
    if let Some((context, persistence, output)) = captured {
        let _ = apply_video_player(&context, &persistence, &output, || clear_active_video_player(Some(persistence.lease())));
        schedule_video_worker_reap();
    }
}
pub(super) fn set_video_player_visible(visible: bool) {
    let captured = ACTIVE_VIDEO_SESSION.with(|slot| slot.borrow().as_ref().map(|active|
        (active.context.clone(), active.persistence.clone(), active.output.clone(), active.cancel.clone())));
    if let Some((context, persistence, output, cancel)) = captured {
        let _ = apply_video_player(&context, &persistence, &output, || {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            desktop_video_player::set_visible_exact(&cancel, visible);
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            let _ = (cancel, visible);
        });
    }
}
pub(super) fn sync_video_player_captured(
    app: &AppWindow, context: &AppContext, persistence: PrivatePersistence,
    output: &SavedVideoOutput, logical_bounds: (f32, f32, f32, f32),
) -> Result<()> {
    let bounds = VideoPlayerBounds::from_logical(logical_bounds.0, logical_bounds.1, logical_bounds.2,
        logical_bounds.3, app.window().scale_factor()).ok_or_else(|| anyhow!("播放器区域无效"))?;
    let cancel = apply_video_player(context, &persistence, output, || {
        let reused = ACTIVE_VIDEO_SESSION.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(active) = slot.as_mut().filter(|active| active.persistence.lease() == persistence.lease()
                && &active.output == output && !active.cancel.load(Ordering::SeqCst)) {
                active.bounds = bounds;
                Some(active.cancel.clone())
            } else { None }
        });
        if let Some(cancel) = reused {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            desktop_video_player::resize_exact(&cancel, bounds);
            return None;
        }
        clear_active_video_player(None);
        let cancel = Arc::new(AtomicBool::new(false));
        ACTIVE_VIDEO_SESSION.with(|slot| *slot.borrow_mut() = Some(ActiveVideoSession {
            context: context.clone(), persistence: persistence.clone(), output: output.clone(),
            cancel: cancel.clone(), bounds, playback: None,
        }));
        Some(cancel)
    }).ok_or_else(|| anyhow!("播放器账号上下文已失效"))?;
    let Some(cancel) = cancel else { return Ok(()); };
    let worker_persistence = persistence.clone();
    let worker_output = output.clone();
    let worker_cancel = cancel.clone();
    let (sender, receiver) = mpsc::channel();
    if let Err(error) = spawn_video_player_worker(persistence.lease().clone(), cancel.clone(), move || {
        let result = prepare_video_playback_with_cancel(worker_persistence, worker_output, worker_cancel)
            .map_err(|_| "视频文件校验或播放器准备失败".to_string());
        let _ = sender.send(result);
    }) {
        clear_active_video_player(Some(persistence.lease()));
        return Err(error);
    }
    poll_video_player_preparation(app.as_weak(), context.clone(), persistence, output.clone(), cancel, receiver);
    Ok(())
}
fn poll_video_player_preparation(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence, output: SavedVideoOutput,
    cancel: Arc<AtomicBool>, receiver: mpsc::Receiver<std::result::Result<PreparedVideoPlayback, String>>,
) {
    slint::Timer::single_shot(Duration::from_millis(30), move || {
        let _ = reap_video_player_workers();
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                poll_video_player_preparation(weak, context, persistence, output, cancel, receiver);
                return;
            }
            Err(TryRecvError::Disconnected) => Err("播放器准备未完成".into()),
        };
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        let mut result = Some(result);
        let applied = apply_video_player(&context, &persistence, &output, || {
            let bounds = ACTIVE_VIDEO_SESSION.with(|slot| slot.borrow().as_ref()
                .filter(|active| Arc::ptr_eq(&active.cancel, &cancel)).map(|active| active.bounds));
            let Some(bounds) = bounds.filter(|_| !cancel.load(Ordering::SeqCst)) else { return false };
            match result.take().expect("single playback result") {
                Ok(playback) => {
                    #[cfg(any(target_os = "windows", target_os = "macos"))]
                    let opened = desktop_video_player::sync(&app, context.clone(), persistence.clone(),
                        output.clone(), &playback.player_url, &playback.url, cancel.clone(), bounds);
                    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                    let opened: Result<()> = { let _ = bounds; Err(anyhow!("当前平台不支持应用内视频播放器")) };
                    match opened {
                        Ok(()) => ACTIVE_VIDEO_SESSION.with(|slot| {
                            if let Some(active) = slot.borrow_mut().as_mut() { active.playback = Some(playback); }
                        }),
                        Err(_) => {
                            cancel.store(true, Ordering::SeqCst);
                            app.global::<AppState>().set_video_status("应用内视频播放器初始化失败，文件仍保留".into());
                        }
                    }
                }
                Err(_) => {
                    cancel.store(true, Ordering::SeqCst);
                    app.global::<AppState>().set_video_status("视频文件校验或播放器准备失败，文件仍保留".into());
                }
            }
            true
        }).unwrap_or(false);
        if !applied { cancel.store(true, Ordering::SeqCst); }
        drop(result);
        schedule_video_worker_reap();
    });
}
fn handle_player_command(
    app: &AppWindow, context: &AppContext, persistence: &PrivatePersistence,
    output: &SavedVideoOutput, command: PlayerCommand,
) {
    match command {
        PlayerCommand::Regenerate => {
            let changed = apply_video_player(context, persistence, output, || {
                let state = app.global::<AppState>();
                if state.get_video_result_path().as_str() != output.source_path { return false; }
                clear_active_video_player(Some(persistence.lease()));
                state.set_video_result_path("".into());
                state.set_video_quote_ready(false);
                state.set_video_status("正在更新服务端报价...".into());
                true
            }).unwrap_or(false);
            if changed {
                schedule_video_worker_reap();
                // Actual quote producer captures billing again; never call it under the latch.
                if player_output_is_current(context, persistence, output) {
                    let state = app.global::<AppState>();
                    state.invoke_request_video_quote(state.get_video_aspect_ratio(), state.get_video_resolution(), state.get_video_duration_seconds());
                }
            }
        }
        PlayerCommand::OpenFolder => start_captured_video_reveal(app, context, persistence, output),
        PlayerCommand::Download => start_captured_video_export(app, context, persistence, output),
        PlayerCommand::Ready => {
            let _ = apply_video_player(context, persistence, output, || {
                app.global::<AppState>().set_video_status("视频已加载".into());
            });
        }
        PlayerCommand::PlaybackError => {
            let _ = apply_video_player(context, persistence, output, || {
                app.global::<AppState>().set_video_status(
                    "视频无法解码，请下载后使用系统播放器查看".into(),
                );
            });
        }
    }
}

struct VideoExportReader {
    source: Arc<OwnedVideoSource>, persistence: PrivatePersistence, cancel: Arc<AtomicBool>, position: u64,
}
impl Read for VideoExportReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let result = (|| -> Result<usize> {
            anyhow::ensure!(!self.cancel.load(Ordering::SeqCst) && self.persistence.is_current(), "video export cancelled");
            if self.position == self.source.size || buffer.is_empty() { return Ok(0); }
            let count = buffer.len().min((self.source.size-self.position).min(VIDEO_STREAM_CHUNK as u64) as usize);
            let bytes = self.source.read_chunk(self.position, count)?;
            buffer[..count].copy_from_slice(&bytes);
            self.position += count as u64;
            Ok(count)
        })();
        result.map_err(|_| std::io::Error::other("video export cancelled or source unavailable"))
    }
}
fn start_captured_video_export(
    app: &AppWindow, context: &AppContext, persistence: &PrivatePersistence, output: &SavedVideoOutput,
) {
    let source = ACTIVE_VIDEO_SESSION.with(|slot| slot.borrow().as_ref()
        .filter(|active| active.persistence.lease() == persistence.lease() && &active.output == output)
        .and_then(|active| active.playback.as_ref()).map(|playback| (playback.source.clone(), playback.cancel.clone())));
    let Some((source, cancel)) = source else { return; };
    let Some(root) = context.data_root_capability.clone() else { return; };
    let Ok((activity, effect)) = persistence.begin_effect() else { return; };
    if !player_output_is_current(context, persistence, output) { return; }
    let name = Path::new(&output.source_path).file_name().and_then(|name| name.to_str()).unwrap_or("elunvi-video.mp4");
    let chosen = rfd::FileDialog::new().set_title("保存视频").set_file_name(name).save_file();
    if activity.is_quiescing() || !persistence.is_current() || !player_output_is_current(context, persistence, output) { return; }
    let Some(chosen) = chosen else { return; };
    let Some(parent) = chosen.parent().map(Path::to_path_buf) else { return; };
    let Some(name) = chosen.file_name().and_then(|name| name.to_str()).map(str::to_owned) else { return; };
    let captured = persistence.clone();
    let worker_cancel = cancel.clone();
    let (sender, receiver) = mpsc::channel();
    let spawned = spawn_video_player_worker(persistence.lease().clone(), cancel.clone(), move || {
        let _effect = effect;
        let result = (|| -> Result<()> {
            anyhow::ensure!(!activity.is_quiescing() && !worker_cancel.load(Ordering::SeqCst), "video export cancelled");
            let destination = ExternalExportDestination::open(&root, &parent)?;
            let mut stream = VideoExportReader { source, persistence: captured, cancel: worker_cancel, position: 0 };
            destination.write_new_file(&name, &mut stream)?;
            Ok(())
        })();
        let _ = sender.send(result.is_ok());
        drop(activity);
    });
    if spawned.is_err() {
        let _ = apply_video_player(context, persistence, output, || app.global::<AppState>().set_video_status("视频未保存，原文件仍保留".into()));
        return;
    }
    poll_video_export(app.as_weak(), context.clone(), persistence.clone(), output.clone(), cancel, receiver);
}
fn poll_video_export(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence, output: SavedVideoOutput,
    cancel: Arc<AtomicBool>, receiver: mpsc::Receiver<bool>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let _ = reap_video_player_workers();
        let succeeded = match receiver.try_recv() {
            Ok(value) => value,
            Err(TryRecvError::Empty) => {
                poll_video_export(weak, context, persistence, output, cancel, receiver);
                return;
            }
            Err(TryRecvError::Disconnected) => false,
        };
        let Some(app) = weak.upgrade() else { return; };
        if cancel.load(Ordering::SeqCst) { return; }
        let _ = apply_video_player(&context, &persistence, &output, || {
            app.global::<AppState>().set_video_status(if succeeded {
                "视频已保存"
            } else { "视频未保存，原文件仍保留；目标已存在时请选择新文件名" }.into());
        });
    });
}

pub(super) fn reveal_saved_video_folder(
    context: &AppContext,
    persistence: PrivatePersistence,
    output: SavedVideoOutput,
) -> Result<()> {
    anyhow::ensure!(
        player_output_is_current(context, &persistence, &output),
        "video owner changed"
    );
    let (activity, effect) = persistence.begin_effect()?;
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    spawn_video_player_worker(persistence.lease().clone(), cancel, move || {
        let _effect = effect;
        let result = (|| -> Result<()> {
            let authority = persistence.storage_authority()?;
            let directory = persistence.lease().namespace.path(ManagedUserArea::Videos);
            let path = Path::new(&output.source_path);
            let relative = path.strip_prefix(directory)?;
            let key = ManagedFileKey::new(
                ManagedUserArea::Videos,
                relative
                    .to_str()
                    .ok_or_else(|| anyhow!("invalid video path"))?,
            )?;
            let file = authority.open_existing_regular(&key)?;
            let metadata = authority.inspect_regular(&file)?;
            anyhow::ensure!(
                metadata.link_count == 1 && metadata.byte_size == output.size_bytes,
                "video file changed"
            );
            anyhow::ensure!(
                persistence.is_current()
                    && !activity.is_quiescing()
                    && !worker_cancel.load(Ordering::SeqCst),
                "video owner changed"
            );
            reveal_path_in_file_manager(path)
        })();
        drop(result);
    })?;
    schedule_video_worker_reap();
    Ok(())
}

fn start_captured_video_reveal(
    app: &AppWindow, context: &AppContext, persistence: &PrivatePersistence, output: &SavedVideoOutput,
) {
    let captured = ACTIVE_VIDEO_SESSION.with(|slot| slot.borrow().as_ref()
        .filter(|active| active.persistence.lease() == persistence.lease() && &active.output == output)
        .and_then(|active| active.playback.as_ref()).map(|playback| (playback.source.clone(), playback.cancel.clone())));
    let Some((source, cancel)) = captured else { return; };
    let _ = start_captured_video_reveal_with(app, context, persistence, output, source, cancel,
        reveal_path_in_file_manager);
}

// The injected final effect is the same boundary used by production, not a
// replacement state machine. Tests supply a no-OS closure at this last boundary.
fn start_captured_video_reveal_with(
    app: &AppWindow, context: &AppContext, persistence: &PrivatePersistence, output: &SavedVideoOutput,
    source: Arc<OwnedVideoSource>, cancel: Arc<AtomicBool>,
    reveal: impl FnOnce(&Path) -> Result<()> + Send + 'static,
) -> bool {
    let Ok((activity, effect)) = persistence.begin_effect() else { return false; };
    if cancel.load(Ordering::SeqCst) || source.authority.lease() != persistence.lease()
        || source.receipt != *output
        || apply_video_player(context, persistence, output, || ()).is_none() {
        return false;
    }
    let captured = persistence.clone();
    let captured_output = output.clone();
    let worker_cancel = cancel.clone();
    let (sender, receiver) = mpsc::channel();
    let spawned = spawn_video_player_worker(persistence.lease().clone(), cancel.clone(), move || {
        let _effect = effect;
        let result = (|| -> Result<()> {
            anyhow::ensure!(!activity.is_quiescing() && !worker_cancel.load(Ordering::SeqCst)
                && captured.is_current(), "video reveal cancelled");
            // Worker-only held identity/size validation; the reader completes its
            // post-validation before the path-based external OS helper runs.
            source.read_chunk(0, 0)?;
            anyhow::ensure!(!activity.is_quiescing() && !worker_cancel.load(Ordering::SeqCst)
                && captured.is_current(), "video reveal cancelled");
            reveal(Path::new(&captured_output.source_path))
        })();
        let _ = sender.send(result.is_ok());
        drop(_effect);
        drop(activity);
    });
    if spawned.is_err() {
        let _ = apply_video_player(context, persistence, output, || {
            app.global::<AppState>().set_video_status("打开文件夹失败，文件仍保留".into());
        });
        return false;
    }
    poll_video_reveal(app.as_weak(), context.clone(), persistence.clone(), output.clone(), cancel, receiver);
    true
}
fn poll_video_reveal(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence, output: SavedVideoOutput,
    cancel: Arc<AtomicBool>, receiver: mpsc::Receiver<bool>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let _ = reap_video_player_workers();
        let succeeded = match receiver.try_recv() {
            Ok(value) => value,
            Err(TryRecvError::Empty) => {
                poll_video_reveal(weak, context, persistence, output, cancel, receiver);
                return;
            }
            Err(TryRecvError::Disconnected) => false,
        };
        let Some(app) = weak.upgrade() else { return; };
        if cancel.load(Ordering::SeqCst) { return; }
        let _ = apply_video_player(&context, &persistence, &output, || {
            if app.global::<AppState>().get_video_result_path().as_str() == output.source_path {
                app.global::<AppState>().set_video_status(if succeeded {
                    "已打开视频所在文件夹"
                } else { "打开文件夹失败，文件仍保留" }.into());
            }
        });
    });
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod desktop_video_player {
    use super::*;
    use wry::dpi::{PhysicalPosition, PhysicalSize};
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder};

    struct NativePlayer { cancel: Arc<AtomicBool>, webview: WebView }
    thread_local! {
        static VIDEO_WEBVIEW: RefCell<Option<NativePlayer>> = const { RefCell::new(None) };
    }
    fn rect(bounds: VideoPlayerBounds) -> Rect {
        Rect {
            position: PhysicalPosition::new(bounds.x, bounds.y).into(),
            size: PhysicalSize::new(bounds.width, bounds.height).into(),
        }
    }
    pub(super) fn sync(
        app: &AppWindow, context: AppContext, persistence: PrivatePersistence, output: SavedVideoOutput,
        player_url: &reqwest::Url, media_url: &reqwest::Url, cancel: Arc<AtomicBool>, bounds: VideoPlayerBounds,
    ) -> Result<()> {
        anyhow::ensure!(player_url.origin() == media_url.origin(), "播放器地址来源不一致");
        let weak = app.as_weak();
        let command_cancel = cancel.clone();
        let allowed_player_url = player_url.as_str().to_string();
        let window_handle = app.window().window_handle();
        let webview = WebViewBuilder::new()
            .with_url(player_url.as_str())
            .with_bounds(rect(bounds))
            .with_devtools(false)
            .with_clipboard(false)
            .with_navigation_handler(move |candidate| {
                candidate == allowed_player_url || candidate == "about:blank"
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_ipc_handler(move |request| {
                let current = ACTIVE_VIDEO_SESSION.with(|slot| slot.borrow().as_ref().is_some_and(|active|
                    Arc::ptr_eq(&active.cancel, &command_cancel) && !command_cancel.load(Ordering::SeqCst)
                    && active.persistence.lease() == persistence.lease() && active.output == output));
                if !current { return; }
                let Some(command) = parse_player_command(request.body()) else { return; };
                let Some(app) = weak.upgrade() else { return; };
                handle_player_command(&app, &context, &persistence, &output, command);
            })
            .build_as_child(&window_handle)
            .context("应用内视频播放器初始化失败")?;
        VIDEO_WEBVIEW.with(|slot| *slot.borrow_mut() = Some(NativePlayer { cancel, webview }));
        Ok(())
    }
    pub(super) fn close_exact(cancel: &Arc<AtomicBool>) {
        VIDEO_WEBVIEW.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|active| Arc::ptr_eq(&active.cancel, cancel)) { slot.take(); }
        });
    }
    pub(super) fn resize_exact(cancel: &Arc<AtomicBool>, bounds: VideoPlayerBounds) {
        VIDEO_WEBVIEW.with(|slot| {
            if let Some(active) = slot.borrow().as_ref().filter(|active| Arc::ptr_eq(&active.cancel, cancel)) {
                let _ = active.webview.set_bounds(rect(bounds));
                let _ = active.webview.set_visible(true);
            }
        });
    }
    pub(super) fn set_visible_exact(cancel: &Arc<AtomicBool>, visible: bool) {
        VIDEO_WEBVIEW.with(|slot| {
            if let Some(active) = slot.borrow().as_ref().filter(|active| Arc::ptr_eq(&active.cancel, cancel)) {
                let _ = active.webview.set_visible(visible);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_commands_are_a_strict_allowlist() {
        assert_eq!(
            parse_player_command(r#"{"command":"download"}"#),
            Some(PlayerCommand::Download)
        );
        assert_eq!(
            parse_player_command(r#"{"command":"open_folder"}"#),
            Some(PlayerCommand::OpenFolder)
        );
        assert_eq!(
            parse_player_command(r#"{"command":"regenerate"}"#),
            Some(PlayerCommand::Regenerate)
        );
        assert_eq!(
            parse_player_command(r#"{"command":"player_ready"}"#),
            Some(PlayerCommand::Ready)
        );
        assert_eq!(
            parse_player_command(r#"{"command":"playback_error"}"#),
            Some(PlayerCommand::PlaybackError)
        );
        for body in [
            r#"{"command":"open_url"}"#,
            r#"{"command":"seek","value":999999}"#,
            r#"{"command":"download","path":"C:\\secret.txt"}"#,
            "not-json",
        ] {
            assert_eq!(parse_player_command(body), None);
        }
    }

    #[test]
    fn player_bounds_reject_invalid_or_zero_areas() {
        assert_eq!(
            VideoPlayerBounds::from_logical(10.0, 20.0, 640.0, 360.0, 1.5),
            Some(VideoPlayerBounds {
                x: 15,
                y: 30,
                width: 960,
                height: 540
            })
        );
        assert!(VideoPlayerBounds::from_logical(0.0, 0.0, 0.0, 360.0, 1.0).is_none());
        assert!(VideoPlayerBounds::from_logical(0.0, 0.0, 640.0, f32::NAN, 1.0).is_none());
    }
}

#[cfg(test)]
mod core_player_tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};

    struct Fixture {
        context: AppContext,
        persistence: PrivatePersistence,
        lease: NamespaceLease,
        authority: Arc<NamespaceStorageAuthority>,
        writer: client_state::tests::Fixture,
        _index: tempfile::TempDir,
    }
    impl Fixture {
        fn new(user: &str) -> Self {
            let writer = client_state::tests::Fixture::new(false, false);
            let session = Arc::new(SessionManager::new(Arc::new(crate::runtime::test_support::MemoryRefreshTokenStore::default())));
            let scope = session.install_tokens_for_user(&TokenSet {
                access_token: "player-test".into(), access_expires_in_seconds: 1800,
                refresh_token: "player-refresh".into(), refresh_expires_at: "2099-01-01T00:00:00Z".into(),
                token_type: "X-Token".into(),
            }, user).unwrap();
            let lease = writer.lease(user, scope.auth_epoch, 1);
            writer.activate(lease.clone()).unwrap();
            let index_root = tempfile::tempdir().unwrap();
            let index = FileIndex::initialize(index_root.path().join("index.sqlite3")).unwrap();
            let api = ApiClient::new(ApiClientConfig {
                base_url: reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),
                app_version: "999.0.0".into(), timeout: Duration::from_millis(50),
            }, DeviceIdentity { id: Uuid::new_v4().to_string(), name: "player-fixture".into(), platform: "macos".into() }, session).unwrap();
            let root = writer.data_root_capability_arc();
            let context = AppContext {
                backend: Some(Arc::new(BackendRuntime { api })),
                data_root_capability: Some(root.clone()), file_index: Some(index.clone()),
                current_user_id: Arc::new(Mutex::new(Some(user.into()))), ..Default::default()
            };
            context.user_activity.activate(lease.clone()).unwrap();
            *context.active_namespace.lock().unwrap() = Some(lease.clone());
            context.backend.as_ref().unwrap().api.bind_user_work(UserWorkAdmission::new(
                context.active_namespace.clone(), context.user_activity.clone(),
            )).unwrap();
            let persistence = PrivatePersistence::for_test_with_storage(
                (*writer).clone(), lease.clone(), context.user_activity.clone(),
                context.backend.as_ref().unwrap().api.upgrade_latch().clone(), root,
                context.backend.as_ref().unwrap().api.clone(), index,
            );
            context.store.borrow_mut().private_persistence = Some(persistence.clone());
            let authority = persistence.storage_authority().unwrap();
            Self { context, persistence, lease, authority, writer, _index: index_root }
        }
        fn output(&self, bytes: &[u8]) -> SavedVideoOutput {
            let key = ManagedFileKey::new(ManagedUserArea::Videos, "owned.mp4").unwrap();
            let authority = self.authority.clone();
            std::thread::scope(|threads| threads.spawn(move || {
                let mut file = authority.create_new_regular(&key).unwrap();
                authority.write_new_regular_from(&mut file, &mut &bytes[..]).unwrap();
                authority.sync_regular(&mut file).unwrap();
            }).join().unwrap());
            let output = SavedVideoOutput { model: String::new(), resolution: String::new(), duration_secs: 0,
                source_asset_id:String::new(),prompt:String::new(),
                client_request_id: "44444444-4444-4444-8444-444444444444".into(), server_task_id: "55555555-5555-4555-8555-555555555555".into(),
                file_id: "66666666-6666-4666-8666-666666666666".into(), billing_account_group_id: "33333333-3333-4333-8333-333333333333".into(),
                sha256: format!("{:x}", Sha256::digest(bytes)), size_bytes: bytes.len() as u64,
                source_path: self.lease.namespace.path(ManagedUserArea::Videos).join("owned.mp4").to_string_lossy().into_owned(),
                title: "owned video".into(), created_at: "2026-09-08T00:00:00Z".into(),
            };
            self.context.store.borrow_mut().video_outputs.insert("55555555-5555-4555-8555-555555555555:66666666-6666-4666-8666-666666666666".into(), output.clone());
            output
        }
        fn prepare(&self, output: SavedVideoOutput) -> PreparedVideoPlayback {
            let persistence = self.persistence.clone();
            std::thread::scope(|threads| threads.spawn(move || prepare_video_playback(persistence, output))
                .join().unwrap()).expect("owned namespace/Videos must prepare the real playback path")
        }
        fn stop(&self) {
            close_video_player_for_retirement(&self.lease);
            let deadline = Instant::now() + Duration::from_secs(6);
            while core_video_player_worker_count(&self.lease) != 0 && Instant::now() < deadline {
                reap_video_player_workers().unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(core_video_player_worker_count(&self.lease), 0, "every owned player worker must be joined");
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            close_video_player_for_retirement(&self.lease);
            let deadline = Instant::now() + Duration::from_secs(6);
            while core_video_player_worker_count(&self.lease) != 0 && Instant::now() < deadline {
                let _ = reap_video_player_workers();
                std::thread::sleep(Duration::from_millis(5));
            }
            if !std::thread::panicking() { assert_eq!(core_video_player_worker_count(&self.lease), 0); }
        }
    }
    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn core_shared_upgrade_observer_stops_captured_player_when_ordinary_result_is_dropped() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = Fixture::new(A);
        let output = fixture.output(b"private buffered video");
        let playback = fixture.prepare(output.clone());
        let cancelled = playback.cancel.clone();
        let app = AppWindow::new().unwrap();
        crate::runtime::app::wire_callbacks(&app, fixture.context.clone());
        app.global::<AppState>().set_session_state("online".into());
        app.global::<AppState>().set_video_result_path(output.source_path.clone().into());
        ACTIVE_VIDEO_SESSION.with(|slot| *slot.borrow_mut() = Some(ActiveVideoSession {
            context: fixture.context.clone(), persistence: fixture.persistence.clone(),
            output, cancel: cancelled.clone(), bounds: VideoPlayerBounds { x:0,y:0,width:10,height:10 },
            playback: Some(playback),
        }));
        // No ordinary HTTP completion delivers the error to the UI.
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        assert!(app.global::<AppState>().get_update_required());
        assert_eq!(app.global::<AppState>().get_session_state(), "update_required");
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(ACTIVE_VIDEO_SESSION.with(|slot| slot.borrow().is_none()));
        fixture.stop();
    }

    fn request(url: &reqwest::Url, method: &str, path: &str, extra: &str) -> (String, Vec<u8>) {
        let mut stream = std::net::TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{extra}Connection: close\r\n\r\n", url.port().unwrap()).unwrap();
        // Test response collection is bounded independently; production streams in chunks.
        let mut response = Vec::new();
        stream.take(3 * 1024 * 1024).read_to_end(&mut response).unwrap();
        let boundary = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
        (String::from_utf8(response[..boundary].to_vec()).unwrap(), response[boundary+4..].to_vec())
    }

    #[test]
    fn core_video_player_no_range_streams_more_than_one_megabyte_completely() {
        let fixture = Fixture::new(A);
        let bytes: Vec<u8> = (0..(2 * 1024 * 1024 + 17)).map(|n| (n % 251) as u8).collect();
        let output = fixture.output(&bytes);
        let playback = fixture.prepare(output);
        assert_eq!(playback.url.scheme(), "http");
        assert_eq!(playback.url.host_str(), Some("127.0.0.1"));
        let (headers, body) = request(&playback.url, "GET", playback.url.path(), "");
        assert!(headers.starts_with("HTTP/1.1 200 "));
        assert_eq!(body, bytes);
        fixture.stop();
    }

    #[test]
    fn core_video_player_serves_the_player_and_media_from_one_local_origin() {
        let fixture = Fixture::new(A);
        let output = fixture.output(b"private video");
        let playback = fixture.prepare(output);
        assert_eq!(playback.player_url.origin(), playback.url.origin());
        let (headers, body) = request(
            &playback.player_url,
            "GET",
            playback.player_url.path(),
            "",
        );
        let html = String::from_utf8(body).unwrap();
        assert!(headers.starts_with("HTTP/1.1 200 "));
        assert!(headers.contains("Content-Type: text/html; charset=utf-8"));
        assert!(headers.contains("Content-Security-Policy:"));
        assert!(html.contains(playback.url.as_str()));
        assert!(html.contains("player_ready"));
        assert!(html.contains("playback_error"));
        fixture.stop();
    }

    #[test]
    fn core_video_player_range_and_head_keep_exact_lengths() {
        let fixture = Fixture::new(A);
        let output = fixture.output(b"0123456789abcdef");
        let playback = fixture.prepare(output);
        let (headers, body) = request(&playback.url, "GET", playback.url.path(), "Range: bytes=3-8\r\n");
        assert!(headers.starts_with("HTTP/1.1 206 "));
        assert!(headers.contains("Content-Range: bytes 3-8/16"));
        assert_eq!(body, b"345678");
        for (range, expected) in [("bytes=-4", &b"cdef"[..]), ("bytes=12-", &b"cdef"[..])] {
            let (headers, body) = request(&playback.url, "GET", playback.url.path(), &format!("Range: {range}\r\n"));
            assert!(headers.starts_with("HTTP/1.1 206 "));
            assert!(headers.contains("Content-Range: bytes 12-15/16"));
            assert_eq!(body, expected);
        }
        let (headers, body) = request(&playback.url, "HEAD", playback.url.path(), "");
        assert!(headers.starts_with("HTTP/1.1 200 "));
        assert!(headers.contains("Content-Length: 16"));
        assert!(body.is_empty());
        let (headers, _) = request(&playback.url, "GET", playback.url.path(), "Range: bytes=19-20\r\n");
        assert!(headers.starts_with("HTTP/1.1 416 "));
        fixture.stop();
    }

    #[test]
    fn core_video_player_rejects_wrong_token_paths_and_methods() {
        let fixture = Fixture::new(A);
        let output = fixture.output(b"private");
        let playback = fixture.prepare(output);
        for (method, path) in [("GET", "/wrong-token"), ("GET", "/../../etc/passwd"), ("POST", playback.url.path())] {
            let (headers, body) = request(&playback.url, method, path, "");
            assert!(!headers.starts_with("HTTP/1.1 200 ") && !headers.starts_with("HTTP/1.1 206 "));
            assert!(body.is_empty());
        }
        for extra in ["Range: bytes=0-1\r\nRange: bytes=2-3\r\n", "Host: foreign.invalid\r\n"] {
            let (headers, body) = request(&playback.url, "GET", playback.url.path(), extra);
            assert!(headers.starts_with("HTTP/1.1 400 "));
            assert!(body.is_empty());
        }
        let (headers, body) = request(&playback.url, "GET", &format!("{}?path=private", playback.url.path()), "");
        assert!(headers.starts_with("HTTP/1.1 404 "));
        assert!(body.is_empty());
        fixture.stop();
    }

    #[test]
    fn core_video_player_retirement_cancels_owned_stream_and_stale_ipc() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = Fixture::new(A);
        let output = fixture.output(b"private");
        let playback = fixture.prepare(output.clone());
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_video_result_path(output.source_path.clone().into());
        app.global::<AppState>().set_video_status("retired".into());
        fixture.context.user_activity.begin_quiesce(&fixture.lease).unwrap().retire();
        close_video_player_for_retirement(&fixture.lease);
        handle_player_command(&app, &fixture.context, &fixture.persistence, &output, PlayerCommand::Regenerate);
        assert_eq!(app.global::<AppState>().get_video_result_path().as_str(), output.source_path);
        assert_eq!(app.global::<AppState>().get_video_status(), "retired");
        assert!(playback.cancel.load(Ordering::SeqCst));
        fixture.stop();
    }

    #[test]
    fn core_video_player_old_owner_close_does_not_cancel_new_owner() {
        let a = Fixture::new(A);
        let b = Fixture::new(B);
        let old = a.prepare(a.output(b"A"));
        let current = b.prepare(b.output(b"B"));
        close_video_player_for_retirement(&a.lease);
        assert!(old.cancel.load(Ordering::SeqCst));
        assert!(!current.cancel.load(Ordering::SeqCst));
        let (headers, body) = request(&current.url, "GET", current.url.path(), "");
        assert!(headers.starts_with("HTTP/1.1 200 "));
        assert_eq!(body, b"B");
        a.stop(); b.stop();
    }

    #[test]
    fn core_video_player_exact_upgrade_denies_regenerate_without_quote_reentry() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = Fixture::new(A);
        let output = fixture.output(b"private");
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_video_result_path(output.source_path.clone().into());
        app.global::<AppState>().set_video_status("upgrade".into());
        app.global::<AppState>().on_request_video_quote(|_, _, _| panic!("426 must deny quote"));
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: None });
        handle_player_command(&app, &fixture.context, &fixture.persistence, &output, PlayerCommand::Regenerate);
        assert_eq!(app.global::<AppState>().get_video_result_path().as_str(), output.source_path);
        assert_eq!(app.global::<AppState>().get_video_status(), "upgrade");
        fixture.stop();
    }

    #[test]
    fn core_video_player_replaced_source_never_serves_replacement_bytes() {
        let fixture = Fixture::new(A);
        let output = fixture.output(b"original");
        let playback = fixture.prepare(output.clone());
        let source = Path::new(&output.source_path);
        fs::rename(source, source.with_file_name("retained-original.mp4")).unwrap();
        fs::write(source, b"foreign!").unwrap();
        let (_, body) = request(&playback.url, "GET", playback.url.path(), "");
        assert!(body.is_empty(), "post-validation failure must prevent the first body chunk");
        fixture.stop();
    }

    #[test]
    fn core_video_player_reveal_denies_upgrade_and_retired_authority_before_effect() {
        i_slint_backend_testing::init_no_event_loop();
        for upgraded in [false, true] {
            let fixture = Fixture::new(A);
            let output = fixture.output(b"private");
            let playback = fixture.prepare(output.clone());
            let app = AppWindow::new().unwrap();
            app.global::<AppState>().set_video_status("untouched".into());
            if upgraded {
                fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: None });
            } else {
                fixture.context.user_activity.begin_quiesce(&fixture.lease).unwrap().retire();
            }
            let invoked = Arc::new(AtomicBool::new(false));
            let observed = invoked.clone();
            assert!(!start_captured_video_reveal_with(
                &app, &fixture.context, &fixture.persistence, &output,
                playback.source.clone(), playback.cancel.clone(),
                move |_| { observed.store(true, Ordering::SeqCst); Ok(()) },
            ));
            assert!(!invoked.load(Ordering::SeqCst));
            assert_eq!(app.global::<AppState>().get_video_status(), "untouched");
            fixture.stop();
        }
    }

    struct JoinedPlayerFixtureThread(Option<std::thread::JoinHandle<()>>);
    impl JoinedPlayerFixtureThread {
        fn finish(mut self) { self.0.take().unwrap().join().unwrap(); }
    }
    impl Drop for JoinedPlayerFixtureThread {
        fn drop(&mut self) { if let Some(worker) = self.0.take() { let _ = worker.join(); } }
    }

    #[test]
    fn core_video_player_reveal_runs_outside_latch_and_late_completion_cannot_enter_b() {
        i_slint_backend_testing::init_no_event_loop();
        let a = Fixture::new(A);
        let b = Fixture::new(B);
        let output = a.output(b"private");
        let playback = a.prepare(output.clone());
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_video_result_path(output.source_path.clone().into());
        let (entered, observed) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let latch = a.persistence.upgrade_latch();
        let expected_path = output.source_path.clone();
        assert!(start_captured_video_reveal_with(
            &app, &a.context, &a.persistence, &output,
            playback.source.clone(), playback.cancel.clone(),
            move |path| {
                assert!(latch.snapshot().is_none(), "external effect must not own the short latch");
                assert_eq!(path, Path::new(&expected_path));
                entered.send(()).unwrap();
                wait_release.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(())
            },
        ));
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        let activity = a.context.user_activity.clone();
        let lease = a.lease.clone();
        let retirement = JoinedPlayerFixtureThread(Some(std::thread::spawn(move || {
            activity.begin_quiesce(&lease).unwrap().retire();
        })));
        release.send(()).unwrap();
        retirement.finish();
        *a.context.active_namespace.lock().unwrap() = Some(b.lease.clone());
        a.context.store.borrow_mut().private_persistence = Some(b.persistence.clone());
        app.global::<AppState>().set_video_status("B status".into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(app.global::<AppState>().get_video_status(), "B status");
        a.stop(); b.stop();
    }
}
