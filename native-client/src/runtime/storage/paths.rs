use super::*;

pub(super) fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn init_version_state(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_current_version(env!("CARGO_PKG_VERSION").into());
    refresh_update_state(app);
}

pub(super) fn refresh_update_state(app: &AppWindow) -> bool {
    let state = app.global::<AppState>();
    let current = env!("CARGO_PKG_VERSION");
    let latest = read_update_manifest_version().unwrap_or_else(|| current.to_string());
    let available = compare_versions(&latest, current).is_gt();
    state.set_latest_version(latest.clone().into());
    state.set_update_available(available);
    if available {
        state.set_update_message(format!("发现新版本 {latest}").into());
    } else if state.get_update_message().is_empty() {
        state.set_update_message("当前已经是最新版本".into());
    }
    available
}

pub(super) fn read_update_manifest_version() -> Option<String> {
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
            let version = manifest.version.trim();
            if !version.is_empty() {
                return Some(version.to_string());
            }
        }
    }
    None
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

pub(super) fn advance_update_progress(app_weak: Weak<AppWindow>) {
    slint::Timer::single_shot(Duration::from_millis(180), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !state.get_update_progress_open() || state.get_update_ready() {
            return;
        }
        let next = (state.get_update_progress() + 8).min(100);
        state.set_update_progress(next);
        if next >= 100 {
            state.set_update_ready(true);
            return;
        }
        advance_update_progress(app.as_weak());
    });
}

pub(super) fn relaunch_current_exe() -> Result<()> {
    let exe = std::env::current_exe().context("无法获取当前客户端路径")?;
    Command::new(exe).spawn().context("无法重启客户端")?;
    Ok(())
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
