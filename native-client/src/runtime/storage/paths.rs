use super::*;

const DEFAULT_UPDATE_MANIFEST_URL: &str =
    "https://static.honeykid.cn/public/art_forge/update-manifest.json";
const DEFAULT_UPDATE_NOTES: &str = "本次更新包含功能优化与问题修复。";
pub(super) const UPDATE_ASSET_HOST: &str = "static.honeykid.cn";
const LEGACY_UPDATE_ASSET_HOST: &str = "cdn.honeykid.cn";

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
    state.set_update_download_sha256("".into());
    state.set_update_download_size("0".into());
    state.set_update_stage("idle".into());
    state.set_update_download_progress(0);
    state.set_update_download_message("".into());
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
    poll_update_check(app.as_weak(), manual, Rc::new(RefCell::new(Some(receiver))));
}

fn poll_update_check(
    app_weak: Weak<AppWindow>,
    manual: bool,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<UpdateManifest, String>>>>>,
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
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("ElunviCanvas/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("无法创建版本检查请求")?
        .get(url)
        .send()
        .context("无法连接版本服务")?;
    if !cfg!(debug_assertions) && response.url().host_str() != Some(UPDATE_ASSET_HOST) {
        anyhow::bail!("更新清单被重定向到不受信任的地址");
    }
    let response = response.error_for_status().context("版本服务返回错误")?;
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
    let manifest_matches_latest = compare_versions(manifest_version, &latest).is_eq();
    let download_url = if manifest_matches_latest {
        manifest_download_url(&manifest)
            .filter(|url| validated_update_download_url(url).is_ok())
            .unwrap_or_else(default_update_download_url)
    } else {
        default_update_download_url()
    };
    let artifact = if manifest_matches_latest {
        manifest_update_artifact(&manifest)
    } else {
        UpdateArtifact::default()
    };
    state.set_update_download_url(download_url.into());
    state.set_update_download_sha256(artifact.sha256.trim().into());
    state.set_update_download_size(artifact.size_bytes.to_string().into());
    state.set_update_stage("idle".into());
    state.set_update_download_progress(0);
    state.set_update_download_message("".into());
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
    canonical_update_download_url(value)
}

fn manifest_update_artifact(manifest: &UpdateManifest) -> UpdateArtifact {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        manifest.artifacts.macos_aarch64.clone()
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        manifest.artifacts.macos_x64.clone()
    } else if cfg!(target_os = "windows") {
        manifest.artifacts.windows_x64.clone()
    } else {
        UpdateArtifact::default()
    }
}

pub(super) fn canonical_update_download_url(candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return None;
    }
    let mut url = reqwest::Url::parse(candidate).ok()?;
    if url.host_str() == Some(LEGACY_UPDATE_ASSET_HOST) {
        url.set_host(Some(UPDATE_ASSET_HOST)).ok()?;
    }
    Some(url.to_string())
}

fn default_update_download_url() -> String {
    let file_name = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "ElunviCanvas_macos_aarch64.dmg"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "ElunviCanvas_macos_x64.dmg"
    } else if cfg!(target_os = "windows") {
        "ElunviCanvas_windows_x64_setup.exe"
    } else {
        return String::new();
    };
    format!("https://{UPDATE_ASSET_HOST}/public/art_forge/{file_name}")
}

pub(super) fn validated_update_download_url(candidate: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(candidate.trim()).context("更新下载地址无效")?;
    if url.scheme() != "https"
        || url.host_str() != Some(UPDATE_ASSET_HOST)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        anyhow::bail!("更新下载地址必须使用受信任的 HTTPS 内容域名");
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
    let minimum_exceeds_known_release =
        latest.is_empty() || compare_versions(minimum, &latest).is_gt();
    if minimum_exceeds_known_release {
        state.set_latest_version(minimum.into());
        state.set_update_download_url(default_update_download_url().into());
        state.set_update_download_sha256("".into());
        state.set_update_download_size("0".into());
    }
    if state.get_update_download_url().is_empty() {
        state.set_update_download_url(default_update_download_url().into());
    }
    state.set_update_available(true);
    state.set_update_required(true);
    state.set_update_stage("idle".into());
    state.set_update_download_progress(0);
    state.set_update_download_message("".into());
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
            PathBuf::from(r"C:\Users\deyx1\Documents\ElunviCanvas"),
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
                .join("ElunviCanvas")
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
    let delivery_staging_dir = data_dir.join("delivery-staging");

    fs::create_dir_all(&data_dir)?;
    let data_metadata = fs::symlink_metadata(&data_dir)?;
    if !data_metadata.file_type().is_dir() || data_metadata.file_type().is_symlink() {
        return Err(anyhow!("应用数据目录不安全，无法启动"));
    }
    for directory in [&input_dir, &output_dir, &prompt_dir, &delivery_staging_dir] {
        if !ensure_managed_subdirectory(directory) {
            return Err(anyhow!("无法创建安全的应用数据子目录"));
        }
    }

    let state = app.global::<AppState>();
    state.set_input_dir(input_dir.display().to_string().into());
    state.set_output_dir(output_dir.display().to_string().into());
    state.set_prompt_dir(prompt_dir.display().to_string().into());
    cleanup_generation_transients_at_startup(app);
    Ok(())
}

const STALE_PART_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const STALE_REFERENCE_UPLOAD_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const STALE_DELIVERY_STAGING_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn transient_path_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn cleanup_stale_files_in_directory(
    directory: &Path,
    retained: &BTreeSet<PathBuf>,
    now: std::time::SystemTime,
    maximum_age: Duration,
    managed_name: impl Fn(&Path) -> bool,
) {
    if !safe_transient_directory(directory) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !managed_name(&path) || retained.contains(&transient_path_identity(&path)) {
            continue;
        }
        let stale = fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= maximum_age);
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}

fn safe_transient_directory(directory: &Path) -> bool {
    let data = app_data_dir();
    if directory == data {
        return fs::symlink_metadata(directory).is_ok_and(|metadata| {
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink()
        });
    }
    if directory.starts_with(&data) {
        return safe_managed_subdirectory(directory);
    }
    let Ok(metadata) = fs::symlink_metadata(directory) else {
        return false;
    };
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(directory) = directory.canonicalize() else {
        return false;
    };
    let Ok(temp) = std::env::temp_dir().canonicalize() else {
        return false;
    };
    directory != temp && directory.starts_with(temp)
}

fn is_partial_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.ends_with(".part"))
}

fn is_atomic_temporary_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(uuid) = name
        .strip_prefix(".artforge-tmp-")
        .and_then(|value| value.strip_suffix(".part"))
    else {
        return false;
    };
    Uuid::parse_str(uuid).is_ok()
}

fn is_generated_output_partial(path: &Path) -> bool {
    if is_atomic_temporary_file(path) {
        return true;
    }
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(timestamp) = name.get(..17) else {
        return false;
    };
    timestamp.bytes().all(|value| value.is_ascii_digit())
        && name.as_bytes().get(17) == Some(&b'-')
        && name.ends_with(".part")
}

fn is_reference_upload_temp(path: &Path) -> bool {
    if is_atomic_temporary_file(path) {
        return true;
    }
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| {
            name.starts_with("reference-")
                && (name.ends_with(".png")
                    || name.ends_with(".jpg")
                    || name.ends_with(".png.part")
                    || name.ends_with(".jpg.part"))
        })
}

fn is_delivery_staging_file(path: &Path) -> bool {
    if is_atomic_temporary_file(path) {
        return true;
    }
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| {
            name.ends_with(".png")
                || name.ends_with(".jpg")
                || name.ends_with(".webp")
                || name.ends_with(".download.part")
        })
}

pub(super) fn cleanup_stale_generation_transients(retained: Option<&BTreeSet<PathBuf>>) {
    let now = std::time::SystemTime::now();
    let empty = BTreeSet::new();
    let reference_upload_dir = std::env::temp_dir()
        .join("ElunviCanvas")
        .join("reference-uploads");
    cleanup_stale_files_in_directory(
        &reference_upload_dir,
        &empty,
        now,
        STALE_REFERENCE_UPLOAD_AGE,
        is_reference_upload_temp,
    );

    let Some(retained) = retained else {
        return;
    };
    let data_dir = app_data_dir();
    cleanup_stale_files_in_directory(
        &data_dir,
        retained,
        now,
        STALE_PART_AGE,
        is_atomic_temporary_file,
    );
    cleanup_stale_files_in_directory(
        &data_dir.join("delivery-staging"),
        retained,
        now,
        STALE_DELIVERY_STAGING_AGE,
        is_delivery_staging_file,
    );
    for directory in [
        data_dir.join("out").join("image-edit-inputs"),
        data_dir.join("out").join("upscale-references"),
        data_dir.join("references").join("imports"),
    ] {
        cleanup_stale_files_in_directory(
            &directory,
            retained,
            now,
            STALE_PART_AGE,
            is_partial_file,
        );
    }
    cleanup_stale_files_in_directory(
        &data_dir.join("out"),
        retained,
        now,
        STALE_PART_AGE,
        is_generated_output_partial,
    );
}

fn remove_delivery_staging_file_in(path: &Path, directory: &Path) {
    if safe_transient_directory(directory)
        && path.parent() == Some(directory)
        && is_delivery_staging_file(path)
        && fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
    {
        let _ = fs::remove_file(path);
    }
}

pub(super) fn cleanup_failed_delivery_staging(path: &Path) {
    remove_delivery_staging_file_in(path, &app_data_dir().join("delivery-staging"));
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

pub(super) fn save_generated_file(app: &AppWindow, source: &Path, prompt: &str) -> Result<String> {
    if !source.is_file() {
        anyhow::bail!("生成文件尚未完整保存");
    }
    let dir = output_dir_path(app);
    fs::create_dir_all(&dir)?;
    let stem = sanitize_filename(&short_text(prompt, 18));
    let ext = source
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            )
        })
        .unwrap_or("png");
    let path = unique_path(dir.join(format!(
        "{}-{}.{}",
        Local::now().format("%Y%m%d%H%M%S%3f"),
        stem,
        ext
    )));
    let (mut output, temporary) = create_atomic_temporary_file(&path)?;
    let copy_result = (|| -> Result<()> {
        let mut input = fs::File::open(source)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        drop(output);
        fs::rename(&temporary, &path)?;
        sync_parent_directory(&path)?;
        Ok(())
    })();
    if let Err(error) = copy_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let _ = fs::remove_file(source);
    Ok(path.display().to_string())
}

pub(super) fn atomic_write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;

    let (mut output, temporary) = create_atomic_temporary_file(path)?;
    if let Err(error) = output.write_all(bytes).and_then(|_| output.sync_all()) {
        drop(output);
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    drop(output);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    sync_parent_directory(path)?;
    Ok(())
}

pub(super) fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "durable path has no parent directory",
            )
        })?;
        fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub(super) fn create_atomic_temporary_file(
    destination: &Path,
) -> std::io::Result<(fs::File, PathBuf)> {
    let parent = destination.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic destination has no parent directory",
        )
    })?;
    for _ in 0..16 {
        let temporary = parent.join(format!(".artforge-tmp-{}.part", Uuid::new_v4()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((file, temporary)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "unable to allocate a unique atomic temporary file",
    ))
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

#[cfg(test)]
mod transient_cleanup_tests {
    use super::*;

    #[test]
    fn atomic_temporary_names_require_the_reserved_prefix_and_valid_uuid() {
        let valid = Path::new(".artforge-tmp-550e8400-e29b-41d4-a716-446655440000.part");
        assert!(is_atomic_temporary_file(valid));
        assert!(is_generated_output_partial(valid));
        assert!(is_reference_upload_temp(valid));
        assert!(is_delivery_staging_file(valid));
        assert!(!is_atomic_temporary_file(Path::new(".artforge-tmp-invalid.part")));
        assert!(!is_atomic_temporary_file(Path::new("user-work.part")));
        assert!(!is_atomic_temporary_file(Path::new(
            ".artforge-tmp-550e8400-e29b-41d4-a716-446655440000.png"
        )));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_never_follows_a_predictable_legacy_temp_symlink() {
        use std::os::unix::fs::symlink;

        let directory = std::env::temp_dir().join(format!(
            "artforge-atomic-symlink-test-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("create atomic test directory");
        let destination = directory.join("preview.png");
        let legacy_temporary = directory.join("preview.png.part");
        let external = directory.join("external-user-file");
        fs::write(&external, b"keep me").expect("write external target");
        symlink(&external, &legacy_temporary).expect("create legacy temp symlink");

        atomic_write_file(&destination, b"new preview").expect("atomic write");

        assert_eq!(fs::read(&external).expect("read external target"), b"keep me");
        assert_eq!(fs::read(&destination).expect("read destination"), b"new preview");
        assert!(legacy_temporary.exists());
        fs::remove_file(&legacy_temporary).expect("remove legacy symlink");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn stale_partial_cleanup_preserves_referenced_and_regular_files() {
        let directory =
            std::env::temp_dir().join(format!("artforge-stale-part-cleanup-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create partial cleanup directory");
        let orphan = directory.join("orphan.png.part");
        let retained = directory.join("retained.png.part");
        let regular = directory.join("work.png");
        fs::write(&orphan, b"orphan").expect("write orphan partial");
        fs::write(&retained, b"retained").expect("write retained partial");
        fs::write(&regular, b"regular").expect("write regular file");

        cleanup_stale_files_in_directory(
            &directory,
            &BTreeSet::from([transient_path_identity(&retained)]),
            std::time::SystemTime::now() + STALE_PART_AGE,
            STALE_PART_AGE,
            is_partial_file,
        );

        assert!(!orphan.exists());
        assert!(retained.is_file());
        assert!(regular.is_file());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_delivery_cleanup_is_limited_to_exact_staging_directory() {
        let root = std::env::temp_dir().join(format!(
            "artforge-delivery-staging-cleanup-{}",
            Uuid::new_v4()
        ));
        let staging = root.join("delivery-staging");
        fs::create_dir_all(&staging).expect("create delivery staging directory");
        let managed = staging.join("request-0-file.png");
        let unrelated = staging.join("notes.txt");
        let outside = root.join("request-0-file.png");
        fs::write(&managed, b"managed").expect("write managed staging file");
        fs::write(&unrelated, b"unrelated").expect("write unrelated staging file");
        fs::write(&outside, b"outside").expect("write outside file");

        remove_delivery_staging_file_in(&managed, &staging);
        remove_delivery_staging_file_in(&unrelated, &staging);
        remove_delivery_staging_file_in(&outside, &staging);

        assert!(!managed.exists());
        assert!(unrelated.is_file());
        assert!(outside.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reference_upload_cleanup_accepts_only_managed_prefix() {
        assert!(is_reference_upload_temp(Path::new("reference-1234.png")));
        assert!(is_reference_upload_temp(Path::new(
            "reference-1234.png.part"
        )));
        assert!(!is_reference_upload_temp(Path::new("portrait.png")));
    }
}
