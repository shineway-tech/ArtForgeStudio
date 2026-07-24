use super::*;

const DEFAULT_UPDATE_MANIFEST_URL: &str =
    "https://cdn.honeykid.cn/public/art_forge/update-manifest.json";
const DEFAULT_UPDATE_NOTES: &str = "本次更新包含功能优化与问题修复。";

pub(super) fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn init_version_state(app: &AppWindow) {
    let state = app.global::<AppState>();
    let current = env!("CARGO_PKG_VERSION");
    state.set_current_version(current.into());
    state.set_latest_version(current.into());
    state.set_update_download_url(default_update_download_url().into());
    state.set_update_check_failed(false);
    state.set_update_message("".into());
}

pub(super) fn begin_update_check(app: &AppWindow, manual: bool) {
    let state = app.global::<AppState>();
    if state.get_update_checking() {
        return;
    }
    state.set_update_checking(true);
    if manual {
        state.set_update_check_failed(false);
        state.set_update_message("".into());
        state.set_update_result_open(false);
    }

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = fetch_update_manifest().map_err(|error| error.to_string());
        let _ = sender.send(result);
    });
    poll_update_check(
        app.as_weak(),
        manual,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_update_check(
    app_weak: Weak<AppWindow>,
    manual: bool,
    receiver: Rc<
        RefCell<
            Option<
                mpsc::Receiver<std::result::Result<UpdateManifest, String>>,
            >,
        >,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(channel) = slot.as_ref() else {
                return;
            };
            match channel.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("版本服务暂时不可用".to_string()))
                }
            }
        };
        let Some(result) = result else {
            poll_update_check(app_weak, manual, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_update_checking(false);
        match result {
            Ok(manifest) => apply_update_manifest(&app, manifest, manual),
            Err(_) if manual => {
                state.set_update_available(false);
                state.set_update_required(false);
                state.set_update_check_failed(true);
                state.set_update_message("当前无法连接更新服务，请检查网络后重试。".into());
                state.set_update_result_open(true);
            }
            Err(_) => {}
        }
    });
}

fn fetch_update_manifest() -> Result<UpdateManifest> {
    match fetch_remote_update_manifest() {
        Ok(manifest) => Ok(manifest),
        Err(remote_error) => read_local_update_manifest().ok_or(remote_error),
    }
}

fn fetch_remote_update_manifest() -> Result<UpdateManifest> {
    let configured = if cfg!(debug_assertions) {
        std::env::var("ARTFORGE_UPDATE_MANIFEST_URL")
            .unwrap_or_else(|_| DEFAULT_UPDATE_MANIFEST_URL.to_string())
    } else {
        DEFAULT_UPDATE_MANIFEST_URL.to_string()
    };
    let url = reqwest::Url::parse(configured.trim()).context("更新清单地址无效")?;
    if !cfg!(debug_assertions) && url.scheme() != "https" {
        anyhow::bail!("生产环境更新清单必须使用 HTTPS");
    }
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(format!("ArtForgeStudio/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("无法创建版本检查请求")?
        .get(url)
        .send()
        .context("无法连接版本服务")?
        .error_for_status()
        .context("版本服务返回错误")?;
    let manifest = response
        .json::<UpdateManifest>()
        .context("更新清单格式无效")?;
    if manifest.version.trim().is_empty() {
        anyhow::bail!("更新清单缺少版本号");
    }
    Ok(manifest)
}

fn read_local_update_manifest() -> Option<UpdateManifest> {
    for base in resource_base_dirs() {
        for path in [
            base.join("update-manifest.json"),
            base.join("data").join("update-manifest.json"),
        ] {
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<UpdateManifest>(&text) else {
                continue;
            };
            if !manifest.version.trim().is_empty() {
                return Some(manifest);
            }
        }
    }
    None
}

fn apply_update_manifest(app: &AppWindow, manifest: UpdateManifest, manual: bool) {
    let state = app.global::<AppState>();
    let current = env!("CARGO_PKG_VERSION");
    state.set_update_check_failed(false);
    let manifest_version = manifest.version.trim();
    let required = state.get_update_required();
    let required_version = state.get_latest_version().to_string();
    let latest = if required && compare_versions(&required_version, manifest_version).is_gt() {
        required_version
    } else {
        manifest_version.to_string()
    };
    let available = required || compare_versions(&latest, current).is_gt();
    state.set_latest_version(latest.clone().into());
    state.set_update_available(available);
    state.set_update_required(required);
    state.set_update_published_at(manifest.published_at.trim().into());
    state.set_update_release_notes(
        if manifest.notes.trim().is_empty() {
            DEFAULT_UPDATE_NOTES
        } else {
            manifest.notes.trim()
        }
        .into(),
    );
    let download_url = manifest_download_url(&manifest)
        .filter(|url| validated_update_download_url(url).is_ok())
        .unwrap_or_else(default_update_download_url);
    state.set_update_download_url(download_url.into());
    state.set_update_message(
        if required {
            format!("在线功能要求升级到 {latest}")
        } else if available {
            format!("发现新版本 {latest}")
        } else {
            "当前已经是最新版本".to_string()
        }
        .into(),
    );
    if available || manual {
        state.set_update_result_open(true);
    }
}

fn manifest_download_url(manifest: &UpdateManifest) -> Option<String> {
    let value = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        &manifest.downloads.macos_aarch64
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        &manifest.downloads.macos_x64
    } else if cfg!(target_os = "windows") {
        &manifest.downloads.windows_x64
    } else {
        ""
    };
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn default_update_download_url() -> String {
    let file_name = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "ArtForgeStudio_macos_aarch64.dmg"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "ArtForgeStudio_macos_x64.dmg"
    } else if cfg!(target_os = "windows") {
        "ArtForgeStudio_windows_x64_setup.exe"
    } else {
        return String::new();
    };
    format!("https://cdn.honeykid.cn/public/art_forge/{file_name}")
}

pub(super) fn validated_update_download_url(candidate: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(candidate.trim()).context("更新下载地址无效")?;
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() {
        anyhow::bail!("更新下载地址必须是安全的 HTTPS 地址");
    }
    Ok(url)
}

pub(super) fn open_update_download(app: &AppWindow) {
    let state = app.global::<AppState>();
    let candidate = state.get_update_download_url().to_string();
    let result = validated_update_download_url(&candidate).and_then(|url| {
        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(url.as_str())
                .spawn()
                .context("无法打开下载地址")?;
        }
        #[cfg(target_os = "windows")]
        {
            Command::new("rundll32")
                .arg("url.dll,FileProtocolHandler")
                .arg(url.as_str())
                .spawn()
                .context("无法打开下载地址")?;
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Command::new("xdg-open")
                .arg(url.as_str())
                .spawn()
                .context("无法打开下载地址")?;
        }
        Ok(())
    });
    if let Err(error) = result {
        state.set_update_release_notes(format!("无法打开下载地址：{error}").into());
    }
}

pub(super) fn show_required_update_prompt(app: &AppWindow, minimum_version: &str) {
    let state = app.global::<AppState>();
    let minimum = minimum_version.trim();
    let latest = state.get_latest_version().to_string();
    if latest.is_empty() || compare_versions(minimum, &latest).is_gt() {
        state.set_latest_version(minimum.into());
    }
    if state.get_update_download_url().is_empty() {
        state.set_update_download_url(default_update_download_url().into());
    }
    state.set_update_available(true);
    state.set_update_required(true);
    state.set_update_check_failed(false);
    state.set_update_message(format!("在线功能要求至少升级到 {minimum}").into());
    state.set_update_result_open(true);
}

pub(super) fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left_parts = version_parts(left);
    let right_parts = version_parts(right);
    let len = left_parts.len().max(right_parts.len());
    for index in 0..len {
        let left_value = *left_parts.get(index).unwrap_or(&0);
        let right_value = *right_parts.get(index).unwrap_or(&0);
        match left_value.cmp(&right_value) {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    std::cmp::Ordering::Equal
}

pub(super) fn version_parts(version: &str) -> Vec<i32> {
    version
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<i32>().unwrap_or(0))
        .collect()
}

pub(super) fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

pub(super) fn macos_resources_dir() -> Option<PathBuf> {
    let exe_dir = app_dir();
    let contents_dir = exe_dir.parent()?;
    if exe_dir.file_name().and_then(|value| value.to_str()) == Some("MacOS")
        && contents_dir.file_name().and_then(|value| value.to_str()) == Some("Contents")
    {
        Some(contents_dir.join("Resources"))
    } else {
        None
    }
}

pub(super) fn resource_base_dirs() -> Vec<PathBuf> {
    let exe_dir = app_dir();
    let mut bases = Vec::new();
    push_unique_path(&mut bases, exe_dir.clone());
    if let Some(resources_dir) = macos_resources_dir() {
        push_unique_path(&mut bases, resources_dir);
    }
    if let Some(parent) = exe_dir.parent() {
        push_unique_path(&mut bases, parent.to_path_buf());
    }
    if let Ok(current_dir) = std::env::current_dir() {
        push_unique_path(&mut bases, current_dir.clone());
        if let Some(parent) = current_dir.parent() {
            push_unique_path(&mut bases, parent.join("local-preview").join("static"));
        }
    }
    #[cfg(windows)]
    {
        push_unique_path(
            &mut bases,
            PathBuf::from(r"C:\Users\deyx1\Documents\ArtForgeStudio"),
        );
    }
    bases
}

pub(super) fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("ArtForgeStudio")
                .join("data");
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(resources_dir) = macos_resources_dir() {
            return resources_dir.join("data");
        }
    }

    macos_resources_dir().unwrap_or_else(app_dir).join("data")
}

pub(super) fn init_portable_dirs(app: &AppWindow) -> Result<()> {
    let data_dir = app_data_dir();
    let input_dir = data_dir.join("input");
    let output_dir = data_dir.join("out");
    let prompt_dir = data_dir.join("prompt");

    fs::create_dir_all(&input_dir)?;
    fs::create_dir_all(&output_dir)?;
    fs::create_dir_all(&prompt_dir)?;

    let state = app.global::<AppState>();
    state.set_input_dir(input_dir.display().to_string().into());
    state.set_output_dir(output_dir.display().to_string().into());
    state.set_prompt_dir(prompt_dir.display().to_string().into());
    Ok(())
}

pub(super) fn output_dir_path(app: &AppWindow) -> PathBuf {
    let value = app.global::<AppState>().get_output_dir().to_string();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return app_data_dir().join("out");
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        path
    } else {
        app_dir().join(path)
    }
}

pub(super) fn save_generated_bytes(app: &AppWindow, bytes: &[u8], prompt: &str) -> Result<String> {
    let dir = output_dir_path(app);
    fs::create_dir_all(&dir)?;
    let stem = sanitize_filename(&short_text(prompt, 18));
    let ext = image_extension(bytes);
    let path = unique_path(dir.join(format!(
        "{}-{}.{}",
        Local::now().format("%Y%m%d%H%M%S%3f"),
        stem,
        ext
    )));
    atomic_write_file(&path, bytes)?;
    Ok(path.display().to_string())
}

pub(super) fn atomic_write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let ext = path.extension().and_then(|value| value.to_str()).unwrap_or("bin");
    let temporary = path.with_extension(format!("{ext}.part"));
    fs::write(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

pub(super) fn image_extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "webp"
    } else {
        "png"
    }
}

pub(super) fn sanitize_filename(value: &str) -> String {
    let text = value
        .chars()
        .map(|ch| {
            if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control()
            {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    let trimmed = text.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "image".to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

pub(super) fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
    for index in 1..1000 {
        let name = if ext.is_empty() {
            format!("{stem}-{index}")
        } else {
            format!("{stem}-{index}.{ext}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}
