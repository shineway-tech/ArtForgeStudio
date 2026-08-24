use super::*;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

pub(super) type UpdateCancellation = Arc<AtomicBool>;

const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

enum UpdateDownloadEvent {
    Progress(i32),
    Verifying,
    Ready(PathBuf),
    Cancelled,
    Failed(String),
}

struct UpdateDownloadRequest {
    url: reqwest::Url,
    expected_size: u64,
    expected_sha256: String,
}

pub(super) fn new_update_cancellation() -> UpdateCancellation {
    Arc::new(AtomicBool::new(false))
}

pub(super) fn cleanup_stale_update_dirs() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_update_temp_dir_name(name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed >= Duration::from_secs(24 * 60 * 60));
        if old_enough {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

pub(super) fn is_update_temp_dir_name(name: &str) -> bool {
    name.strip_prefix("artforge-update-")
        .is_some_and(|suffix| Uuid::parse_str(suffix).is_ok())
}

pub(super) fn begin_automatic_update(app: &AppWindow, cancellation: UpdateCancellation) {
    let state = app.global::<AppState>();
    if state.get_update_active() {
        return;
    }
    let request = match update_download_request(&state) {
        Ok(request) => request,
        Err(message) => {
            state.set_update_stage("manual".into());
            state.set_update_download_message(message.into());
            open_update_download(app);
            return;
        }
    };

    cancellation.store(false, AtomicOrdering::Release);
    state.set_update_result_open(true);
    state.set_update_stage("downloading".into());
    state.set_update_download_progress(0);
    state.set_update_download_message("正在下载安装包…".into());

    let (sender, receiver) = mpsc::channel();
    let worker_cancellation = cancellation.clone();
    std::thread::spawn(move || {
        download_update_package(request, worker_cancellation, sender);
    });
    poll_update_download(
        app.as_weak(),
        cancellation,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

pub(super) fn cancel_automatic_update(app: &AppWindow, cancellation: &UpdateCancellation) {
    let state = app.global::<AppState>();
    if matches!(
        state.get_update_stage().as_str(),
        "downloading" | "verifying"
    ) {
        cancellation.store(true, AtomicOrdering::Release);
        state.set_update_stage("cancelling".into());
        state.set_update_download_message("正在取消下载…".into());
    }
}

fn update_download_request(state: &AppState) -> std::result::Result<UpdateDownloadRequest, String> {
    let url = validated_update_download_url(&state.get_update_download_url().to_string())
        .map_err(|_| "更新地址无效，已改为使用浏览器下载。".to_string())?;
    let expected_size = state
        .get_update_download_size()
        .trim()
        .parse::<u64>()
        .map_err(|_| "更新清单缺少文件大小，已改为使用浏览器下载。".to_string())?;
    let expected_sha256 = state
        .get_update_download_sha256()
        .trim()
        .to_ascii_lowercase();
    if !valid_update_artifact_metadata(expected_size, &expected_sha256) {
        return Err("更新清单缺少完整校验信息，已改为使用浏览器下载。".to_string());
    }
    Ok(UpdateDownloadRequest {
        url,
        expected_size,
        expected_sha256,
    })
}

pub(super) fn valid_update_artifact_metadata(expected_size: u64, expected_sha256: &str) -> bool {
    expected_size > 0
        && expected_sha256.len() == 64
        && expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn download_update_package(
    request: UpdateDownloadRequest,
    cancellation: UpdateCancellation,
    sender: mpsc::Sender<UpdateDownloadEvent>,
) {
    let result = perform_update_download(&request, &cancellation, &sender);
    match result {
        Ok(Some(path)) => {
            let _ = sender.send(UpdateDownloadEvent::Ready(path));
        }
        Ok(None) => {
            let _ = sender.send(UpdateDownloadEvent::Cancelled);
        }
        Err(error) => {
            let _ = sender.send(UpdateDownloadEvent::Failed(error.to_string()));
        }
    }
}

fn perform_update_download(
    request: &UpdateDownloadRequest,
    cancellation: &UpdateCancellation,
    sender: &mpsc::Sender<UpdateDownloadEvent>,
) -> Result<Option<PathBuf>> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(UPDATE_DOWNLOAD_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("ElunviCanvas/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("无法创建更新下载请求")?;
    let mut response = client
        .get(request.url.clone())
        .send()
        .context("无法连接更新下载服务")?
        .error_for_status()
        .context("更新下载服务返回错误")?;
    if response.url().host_str() != Some(UPDATE_ASSET_HOST) {
        anyhow::bail!("更新下载被重定向到不受信任的地址");
    }
    if let Some(content_length) = response.content_length() {
        if content_length != request.expected_size {
            anyhow::bail!("安装包大小与更新清单不一致");
        }
    }

    let update_dir = std::env::temp_dir().join(format!("artforge-update-{}", Uuid::new_v4()));
    fs::create_dir_all(&update_dir).context("无法创建更新临时目录")?;
    let final_path = update_dir.join(update_package_file_name());
    let partial_path = update_dir.join(format!("{}.part", update_package_file_name()));
    let result = (|| -> Result<Option<PathBuf>> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial_path)
            .context("无法创建更新临时文件")?;
        let mut digest = Sha256::new();
        let mut downloaded = 0_u64;
        let mut last_progress = -1;
        let mut buffer = vec![0_u8; 128 * 1024];

        loop {
            if cancellation.load(AtomicOrdering::Acquire) {
                drop(file);
                let _ = fs::remove_dir_all(&update_dir);
                return Ok(None);
            }
            let count = response.read(&mut buffer).context("下载安装包失败")?;
            if count == 0 {
                break;
            }
            file.write_all(&buffer[..count]).context("写入安装包失败")?;
            digest.update(&buffer[..count]);
            downloaded = downloaded.saturating_add(count as u64);
            if downloaded > request.expected_size {
                anyhow::bail!("安装包大小超过更新清单声明");
            }
            let progress =
                ((downloaded.saturating_mul(100)) / request.expected_size).min(100) as i32;
            if progress != last_progress {
                last_progress = progress;
                let _ = sender.send(UpdateDownloadEvent::Progress(progress));
            }
        }
        file.sync_all().context("无法保存安装包")?;
        drop(file);

        let _ = sender.send(UpdateDownloadEvent::Verifying);
        let actual_sha256 = format!("{:x}", digest.finalize());
        if downloaded != request.expected_size
            || !actual_sha256.eq_ignore_ascii_case(&request.expected_sha256)
        {
            anyhow::bail!("安装包完整性校验失败");
        }
        fs::rename(&partial_path, &final_path).context("无法完成安装包写入")?;
        Ok(Some(final_path))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&update_dir);
    }
    result
}

fn poll_update_download(
    app_weak: Weak<AppWindow>,
    cancellation: UpdateCancellation,
    receiver: Rc<RefCell<Option<mpsc::Receiver<UpdateDownloadEvent>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let events = {
            let slot = receiver.borrow();
            let Some(channel) = slot.as_ref() else {
                return;
            };
            let mut events = Vec::new();
            loop {
                match channel.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        events.push(UpdateDownloadEvent::Failed(
                            "更新下载任务意外结束".to_string(),
                        ));
                        break;
                    }
                }
            }
            events
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let mut finished = false;
        for event in events {
            match event {
                UpdateDownloadEvent::Progress(progress) => {
                    let state = app.global::<AppState>();
                    state.set_update_download_progress(progress.clamp(0, 100));
                    state.set_update_download_message("正在下载安装包…".into());
                }
                UpdateDownloadEvent::Verifying => {
                    let state = app.global::<AppState>();
                    state.set_update_stage("verifying".into());
                    state.set_update_download_progress(100);
                    state.set_update_download_message("正在校验安装包…".into());
                }
                UpdateDownloadEvent::Ready(path) => {
                    finished = true;
                    receiver.borrow_mut().take();
                    if cancellation.load(AtomicOrdering::Acquire) {
                        if let Some(parent) = path.parent() {
                            let _ = fs::remove_dir_all(parent);
                        }
                        let state = app.global::<AppState>();
                        state.set_update_stage("idle".into());
                        state.set_update_download_message("更新已取消。".into());
                    } else {
                        handoff_update_to_installer(&app, &path);
                    }
                    break;
                }
                UpdateDownloadEvent::Cancelled => {
                    finished = true;
                    receiver.borrow_mut().take();
                    let state = app.global::<AppState>();
                    state.set_update_stage("idle".into());
                    state.set_update_download_progress(0);
                    state.set_update_download_message("更新已取消。".into());
                    break;
                }
                UpdateDownloadEvent::Failed(message) => {
                    finished = true;
                    receiver.borrow_mut().take();
                    let state = app.global::<AppState>();
                    state.set_update_stage("failed".into());
                    state.set_update_download_message(
                        format!("{message}，请重试或稍后更新。").into(),
                    );
                    break;
                }
            }
        }
        if !finished {
            poll_update_download(app.as_weak(), cancellation, receiver);
        }
    });
}

fn handoff_update_to_installer(app: &AppWindow, package_path: &Path) {
    let state = app.global::<AppState>();
    state.set_update_stage("installing".into());
    state.set_update_download_progress(100);
    state.set_update_download_message("安装包校验通过，正在启动安装…".into());
    let expected_version = state.get_latest_version().to_string();
    match launch_update_installer(package_path, &expected_version) {
        Ok(()) => {
            let _ = app.window().hide();
            let _ = slint::quit_event_loop();
        }
        Err(error) => {
            if let Some(parent) = package_path.parent() {
                let _ = fs::remove_dir_all(parent);
            }
            state.set_update_stage("failed".into());
            state.set_update_download_message(format!("无法启动自动安装：{error}").into());
        }
    }
}

fn update_package_file_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "ElunviCanvas-update.dmg"
    } else if cfg!(target_os = "windows") {
        "ElunviCanvas-update.exe"
    } else {
        "ElunviCanvas-update.bin"
    }
}

#[cfg(target_os = "windows")]
fn launch_update_installer(package_path: &Path, _expected_version: &str) -> Result<()> {
    Command::new(package_path)
        .args(windows_update_installer_args())
        .spawn()
        .context("无法启动 Windows 更新安装器")?;
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn windows_update_installer_args() -> &'static [&'static str] {
    &[
        "/SP-",
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NOCANCEL",
        "/NORESTART",
        "/CLOSEAPPLICATIONS",
    ]
}

#[cfg(target_os = "macos")]
fn launch_update_installer(package_path: &Path, expected_version: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let update_dir = package_path
        .parent()
        .ok_or_else(|| anyhow!("更新临时目录无效"))?;
    let helper_path = update_dir.join("install-update.sh");
    let mut helper = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&helper_path)
        .context("无法创建 macOS 更新辅助程序")?;
    helper
        .write_all(MACOS_UPDATE_HELPER.as_bytes())
        .context("无法写入 macOS 更新辅助程序")?;
    helper.sync_all().context("无法保存 macOS 更新辅助程序")?;
    drop(helper);
    fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o700))
        .context("无法设置 macOS 更新辅助程序权限")?;

    let current_bundle = macos_current_bundle();
    let destination = macos_update_destination(current_bundle.as_deref());
    let fallback_bundle = current_bundle
        .clone()
        .unwrap_or_else(|| destination.clone());
    let app_pid = std::process::id();
    let shell_command = [
        shell_quote(helper_path.as_os_str().to_string_lossy().as_ref()),
        app_pid.to_string(),
        shell_quote(package_path.as_os_str().to_string_lossy().as_ref()),
        shell_quote(destination.as_os_str().to_string_lossy().as_ref()),
        shell_quote(expected_version),
    ]
    .join(" ");
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        apple_script_string(&shell_command)
    );
    let recovery_message =
        "display alert \"Elunvi Canvas 更新失败\" message \"已保留原版本，请稍后重试。\"";
    let recovery_command = format!(
        "/usr/bin/osascript -e {} || {{ \
         wait_count=0; \
         while /bin/kill -0 {app_pid} 2>/dev/null && [ \"$wait_count\" -lt 20 ]; do \
         /bin/sleep 0.5; wait_count=$((wait_count + 1)); done; \
         recovery_app={}; \
         if [ ! -d \"$recovery_app\" ]; then recovery_app={}; fi; \
         if [ -d \"$recovery_app\" ]; then /usr/bin/open \"$recovery_app\"; fi; \
         /usr/bin/osascript -e {}; \
         }}",
        shell_quote(&apple_script),
        shell_quote(destination.as_os_str().to_string_lossy().as_ref()),
        shell_quote(fallback_bundle.as_os_str().to_string_lossy().as_ref()),
        shell_quote(recovery_message),
    );
    Command::new("/bin/sh")
        .arg("-c")
        .arg(recovery_command)
        .spawn()
        .context("无法启动 macOS 更新授权程序")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_current_bundle() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|executable| {
        executable
            .ancestors()
            .find(|path| path.extension().and_then(|value| value.to_str()) == Some("app"))
            .map(Path::to_path_buf)
    })
}

#[cfg(target_os = "macos")]
fn macos_update_destination(current_bundle: Option<&Path>) -> PathBuf {
    match current_bundle {
        Some(path) if !path.starts_with("/Volumes") => path.to_path_buf(),
        _ => PathBuf::from("/Applications/ElunviCanvas.app"),
    }
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "macos")]
fn apple_script_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
const MACOS_UPDATE_HELPER: &str = r#"#!/bin/sh
set -eu

app_pid="$1"
dmg_path="$2"
destination="$3"
expected_version="$4"
wait_count=0
while kill -0 "$app_pid" 2>/dev/null; do
  if [ "$wait_count" -ge 240 ]; then
    exit 1
  fi
  sleep 0.5
  wait_count=$((wait_count + 1))
done

mount_dir="$(mktemp -d /tmp/artforge-update-mount.XXXXXX)"
backup="${destination}.artforge-update-backup.$$"
mounted=0
backup_created=0
cleanup() {
  if [ "$backup_created" -eq 1 ] && [ -d "$backup" ]; then
    /bin/rm -rf "$destination"
    /bin/mv "$backup" "$destination" || true
  fi
  if [ "$mounted" -eq 1 ]; then
    /usr/bin/hdiutil detach "$mount_dir" -quiet || true
  fi
  /bin/rm -rf "$mount_dir"
}
trap cleanup EXIT

/usr/bin/hdiutil verify "$dmg_path" >/dev/null
/usr/bin/hdiutil attach "$dmg_path" -nobrowse -readonly -mountpoint "$mount_dir" >/dev/null
mounted=1
source_app="$mount_dir/ElunviCanvas.app"
test -d "$source_app"
installed_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$source_app/Contents/Info.plist")"
test "$installed_version" = "$expected_version"
/usr/bin/codesign --verify --deep --strict "$source_app"
/usr/sbin/spctl --assess --type execute "$source_app"

if [ -d "$destination" ]; then
  /bin/mv "$destination" "$backup"
  backup_created=1
fi
console_user="$(/usr/bin/stat -f '%Su' /dev/console)"
if /usr/bin/ditto "$source_app" "$destination" \
  && /usr/bin/codesign --verify --deep --strict "$destination"; then
  /bin/rm -rf "$backup"
  backup_created=0
  /usr/bin/sudo -u "$console_user" /usr/bin/open "$destination"
  /bin/rm -rf "$(dirname "$dmg_path")"
else
  /bin/rm -rf "$destination"
  if [ -d "$backup" ]; then
    /bin/mv "$backup" "$destination"
    backup_created=0
    /usr/bin/sudo -u "$console_user" /usr/bin/open "$destination"
  fi
  exit 1
fi
"#;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn launch_update_installer(_package_path: &Path, _expected_version: &str) -> Result<()> {
    anyhow::bail!("当前平台暂不支持自动安装")
}
