use super::*;

const MAX_COMPRESSION_IMAGES: usize = 50;
const MAX_CONVERSION_IMAGES: usize = 50;
const COLORIZATION_MAX_INPUT_BYTES: u64 = 10 * 1024 * 1024;
const COLORIZATION_MAX_EDGE_EXCLUSIVE: u32 = 3000;
const TOOLBOX_TEMP_FILE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

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

fn managed_toolbox_directory(
    data_directory: &Path,
    directory: ManagedToolboxDirectory,
) -> PathBuf {
    data_directory.join("toolbox").join(directory.name())
}

fn resolve_safe_managed_toolbox_directory(
    data_directory: &Path,
    directory: ManagedToolboxDirectory,
) -> Option<PathBuf> {
    let toolbox_directory = data_directory.join("toolbox");
    let managed_directory = managed_toolbox_directory(data_directory, directory);
    for candidate in [data_directory, toolbox_directory.as_path(), managed_directory.as_path()] {
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

pub(super) fn wire_toolbox_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        state.on_choose_compression_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_compression_processing() || state.get_compression_saving() {
                return;
            }
            let Some(paths) = rfd::FileDialog::new()
                .add_filter("Images", crate::image_formats::picker_image_extensions())
                .pick_files()
            else {
                return;
            };
            add_compression_paths(&app, paths);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_add_compression_images_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_compression_from_drag_data(&app, TEXT_PLAIN_MIME, data.as_str())
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_paste_compression_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            paste_compression_image(&app)
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_remove_compression_image(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_compression_processing() || state.get_compression_saving() {
                return;
            }
            let id = id.to_string();
            let (removed, images): (Vec<_>, Vec<_>) = state
                .get_compression_images()
                .iter()
                .partition(|item| item.id.as_str() == id);
            set_compression_images(&state, images);
            let data_directory = app_data_dir();
            for item in &removed {
                remove_toolbox_item_files(
                    &data_directory,
                    item,
                    ManagedToolboxDirectory::CompressionInputs,
                    ManagedToolboxDirectory::CompressionResults,
                );
            }
            state.set_compression_message("".into());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_clear_compression_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_compression_processing() || state.get_compression_saving() {
                return;
            }
            let removed = state.get_compression_images().iter().collect::<Vec<_>>();
            set_compression_images(&state, Vec::new());
            let data_directory = app_data_dir();
            for item in &removed {
                remove_toolbox_item_files(
                    &data_directory,
                    item,
                    ManagedToolboxDirectory::CompressionInputs,
                    ManagedToolboxDirectory::CompressionResults,
                );
            }
            state.set_compression_message("".into());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_save_compression_result(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
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
            if !result_path.is_file() {
                state.set_compression_message(
                    if state.get_language().as_str() == "en" {
                        "No compressed image is available to save"
                    } else {
                        "暂无可保存的压缩结果"
                    }
                    .into(),
                );
                return;
            }
            start_compression_result_save(&app, result_path, source_name);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_update_compression_target_preview(move |value| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
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
            state.set_compression_target_mb(preview.into());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_start_compression(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_local_compression(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_choose_conversion_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_conversion_processing() || state.get_conversion_saving() {
                return;
            }
            let Some(paths) = rfd::FileDialog::new()
                .add_filter("Images", crate::image_formats::picker_image_extensions())
                .pick_files()
            else {
                return;
            };
            add_conversion_paths(&app, paths);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_add_conversion_images_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_conversion_from_drag_data(&app, TEXT_PLAIN_MIME, data.as_str())
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_paste_conversion_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            paste_conversion_image(&app)
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_remove_conversion_image(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_conversion_processing() || state.get_conversion_saving() {
                return;
            }
            let id = id.to_string();
            let (removed, images): (Vec<_>, Vec<_>) = state
                .get_conversion_images()
                .iter()
                .partition(|item| item.id.as_str() == id);
            set_conversion_images(&state, images);
            let data_directory = app_data_dir();
            for item in &removed {
                remove_toolbox_item_files(
                    &data_directory,
                    item,
                    ManagedToolboxDirectory::ConversionInputs,
                    ManagedToolboxDirectory::ConversionResults,
                );
            }
            state.set_conversion_message("".into());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_clear_conversion_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_conversion_processing() || state.get_conversion_saving() {
                return;
            }
            let removed = state.get_conversion_images().iter().collect::<Vec<_>>();
            set_conversion_images(&state, Vec::new());
            let data_directory = app_data_dir();
            for item in &removed {
                remove_toolbox_item_files(
                    &data_directory,
                    item,
                    ManagedToolboxDirectory::ConversionInputs,
                    ManagedToolboxDirectory::ConversionResults,
                );
            }
            state.set_conversion_message("".into());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_save_conversion_result(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
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
            if !path.is_file() {
                state.set_conversion_message(
                    if state.get_language().as_str() == "en" {
                        "No converted image is available yet"
                    } else {
                        "暂无可保存的转换结果"
                    }
                    .into(),
                );
                return;
            }
            start_conversion_result_save(&app, path, source_name);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_start_conversion(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_local_conversion(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_choose_crop_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if app.global::<AppState>().get_crop_processing() {
                return;
            }
            let app_weak = app.as_weak();
            drop(app);
            let _ = slint::spawn_local(async move {
                let Some(file) = rfd::AsyncFileDialog::new()
                    .add_filter("Images", crate::image_formats::picker_image_extensions())
                    .pick_file()
                    .await
                else {
                    return;
                };
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                add_crop_paths(&app, vec![file.path().to_path_buf()]);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_add_crop_source_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_crop_from_drag_data(&app, TEXT_PLAIN_MIME, data.as_str())
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_paste_crop_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            paste_crop_image(&app)
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_crop_ratio(move |ratio| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            set_crop_ratio(&app, ratio.as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_update_crop_rect(move |action, dx, dy, x, y, width, height| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            update_crop_rect(&app, action.as_str(), dx, dy, x, y, width, height);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_transform_crop_source(move |action| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Err(error) = transform_crop_source(&app, action.as_str()) {
                set_crop_error(&app, &error);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reset_crop_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Err(error) = reset_crop_source(&app) {
                set_crop_error(&app, &error);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = context.store.clone();
        state.on_save_crop_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_crop_save(&app, store.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_add_watermark_source_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_watermark_from_drag_data(&app, TEXT_PLAIN_MIME, data.as_str())
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_choose_watermark_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["jpg", "jpeg", "png", "bmp"])
                .pick_file()
            else {
                return;
            };
            if !set_watermark_source_from_path(&app, &path) {
                set_watermark_unsupported_message(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_start_watermark_removal(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_watermark_source_path().trim().is_empty() {
                state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "Upload an image first"
                    } else {
                        "请先上传图片"
                    }
                    .into(),
                );
                return;
            }
            start_watermark_removal(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_start_remove_black_tool(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_remove_black_tool(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reveal_watermark_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_watermark_result_path().to_string());
            if !path.is_file() {
                state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "No processed image is available yet"
                    } else {
                        "暂无可查看的处理结果"
                    }
                    .into(),
                );
                return;
            }
            match reveal_path_in_file_manager(&path) {
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
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_add_colorize_source_from_drag(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            add_colorization_from_drag_data(&app, TEXT_PLAIN_MIME, data.as_str())
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_choose_colorize_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["jpg", "jpeg", "png", "bmp"])
                .pick_file()
            else {
                return;
            };
            if let Err(error) = set_colorization_source_from_path(&app, &path) {
                set_colorization_source_error(&app, &error);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_start_colorize(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_colorize_source_path().trim().is_empty() {
                state.set_colorize_message(
                    if state.get_language().as_str() == "en" {
                        "Upload an image first"
                    } else {
                        "请先上传图片"
                    }
                    .into(),
                );
                return;
            }
            start_image_colorization(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reveal_colorize_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_colorize_result_path().to_string());
            if !path.is_file() {
                state.set_colorize_message(
                    if state.get_language().as_str() == "en" {
                        "No colorized image is available yet"
                    } else {
                        "暂无可查看的上色结果"
                    }
                    .into(),
                );
                return;
            }
            match reveal_path_in_file_manager(&path) {
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
            }
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

fn start_remove_black_tool(app: &AppWindow) {
    let state = app.global::<AppState>();
    let source = PathBuf::from(state.get_watermark_source_path().to_string());
    if !source.is_file() {
        state.set_watermark_message("请先上传图片".into());
        return;
    }
    state.set_watermark_processing(true);
    state.set_watermark_progress(12);
    state.set_watermark_message("正在去黑...".into());

    let result = (|| -> Result<(PathBuf, String, Image)> {
        let decoded = image::open(&source).context("无法读取图片")?;
        let mut rgba = decoded.to_rgba8();
        remove_black_pixels(rgba.as_mut());
        let bytes = encode_png_rgba(&rgba, rgba.width(), rgba.height())?;
        let dir = output_dir_path(app);
        fs::create_dir_all(&dir)?;
        let stem = source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("image");
        let path = unique_path(dir.join(format!(
            "{}-{}-remove-black.png",
            Local::now().format("%Y%m%d%H%M%S%3f"),
            sanitize_filename(stem)
        )));
        fs::write(&path, bytes)?;
        let preview = load_preview_image(&path, PreviewPurpose::Canvas)?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        Ok((path, name, preview))
    })();

    match result {
        Ok((path, name, preview)) => {
            state.set_watermark_result_path(path.display().to_string().into());
            state.set_watermark_result_name(name.into());
            state.set_watermark_result_image(preview);
            state.set_watermark_progress(100);
            state.set_watermark_message("去黑完成".into());
        }
        Err(error) => {
            state.set_watermark_progress(0);
            state.set_watermark_message(format!("去黑失败：{error}").into());
        }
    }
    state.set_watermark_processing(false);
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

fn paste_compression_image(app: &AppWindow) -> bool {
    let state = app.global::<AppState>();
    if state.get_compression_processing() || state.get_compression_saving() {
        return true;
    }
    if state.get_compression_images().row_count() >= MAX_COMPRESSION_IMAGES {
        state.set_compression_message(
            compression_limit_message(state.get_language().as_str() == "en").into(),
        );
        return true;
    }
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return false;
    };
    if let Ok(image) = clipboard.get_image() {
        let Some(rgba) = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.into_owned(),
        ) else {
            return false;
        };
        let Ok(bytes) = encode_png_rgba(&rgba, rgba.width(), rgba.height()) else {
            return false;
        };
        let directory = app_data_dir().join("toolbox").join("compression-inputs");
        if !ensure_managed_subdirectory(&directory) {
            return false;
        }
        let path = directory.join(format!("pasted-{}.png", Uuid::new_v4()));
        if atomic_write_file(&path, &bytes).is_err() {
            return false;
        }
        add_compression_paths(app, vec![path]);
        return true;
    }
    let Ok(text) = clipboard.get_text() else {
        return false;
    };
    add_compression_from_drag_data(app, TEXT_PLAIN_MIME, &text)
}

fn set_compression_images(state: &AppState, images: Vec<CompressionImageItem>) {
    let has_results = images
        .iter()
        .any(|item| item.status.as_str() == "completed" && !item.result_path.is_empty());
    state.set_compression_images(ModelRc::new(VecModel::from(images)));
    state.set_compression_has_results(has_results);
}

fn start_local_compression(app: &AppWindow) {
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
                state.set_compression_message(
                    if state.get_language().as_str() == "en" {
                        "Enter a valid target size"
                    } else {
                        "请输入有效的目标文件大小"
                    }
                    .into(),
                );
                return;
            };
            ImageCompressionMode::TargetBytes(target_bytes)
        }
        _ => {
            state.set_compression_message(
                if state.get_language().as_str() == "en" {
                    "Choose a supported compression mode"
                } else {
                    "请选择受支持的压缩方式"
                }
                .into(),
            );
            return;
        }
    };

    let mut images = state.get_compression_images().iter().collect::<Vec<_>>();
    if images.is_empty() {
        state.set_compression_message(
            if state.get_language().as_str() == "en" {
                "Add at least one image first"
            } else {
                "请先添加需要压缩的图片"
            }
            .into(),
        );
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
    set_compression_images(&state, images);
    let data_directory = app_data_dir();
    for result_path in abandoned_results {
        let _ = remove_managed_toolbox_file(
            &data_directory,
            ManagedToolboxDirectory::CompressionResults,
            &result_path,
        );
    }
    state.set_compression_processing(true);
    state.set_compression_message(
        if state.get_language().as_str() == "en" {
            "Compressing images locally..."
        } else {
            "正在压缩中..."
        }
        .into(),
    );

    let output_dir = app_data_dir().join("toolbox").join("compression-results");
    if !ensure_managed_subdirectory(&output_dir) {
        state.set_compression_processing(false);
        state.set_compression_message("无法创建安全的压缩缓存目录".into());
        return;
    }
    let (sender, receiver) = mpsc::channel::<CompressionOutcome>();
    std::thread::spawn(move || {
        run_local_compression_worker(inputs, mode, output_dir, sender);
    });
    poll_local_compression(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
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
    receiver: Rc<RefCell<Option<mpsc::Receiver<CompressionOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(CompressionOutcome::Interrupted)
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_local_compression(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        match outcome {
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
                receiver.borrow_mut().take();
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
        }
        if keep_polling {
            poll_local_compression(app_weak, receiver);
        }
    });
}

fn start_compression_result_save(app: &AppWindow, result_path: PathBuf, source_name: String) {
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
    let _ = slint::spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title("保存压缩结果")
            .set_file_name(default_name)
            .add_filter(filter_name, &[extension.as_str()])
            .save_file()
            .await
        else {
            return;
        };
        let destination = normalize_compression_destination(file.path(), &extension);
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_compression_saving(true);
        state.set_compression_message(
            if state.get_language().as_str() == "en" {
                "Saving the compressed image..."
            } else {
                "正在保存压缩结果..."
            }
            .into(),
        );
        let (sender, receiver) = mpsc::channel::<CompressionSaveOutcome>();
        let data_directory = app_data_dir();
        std::thread::spawn(move || {
            let outcome = copy_and_release_managed_toolbox_result(
                &result_path,
                &destination,
                &data_directory,
                ManagedToolboxDirectory::CompressionResults,
            )
            .map(|released| CompressionSaveOutcome::Saved {
                destination,
                released_result_path: released.then_some(result_path),
            })
            .unwrap_or(CompressionSaveOutcome::Failed);
            let _ = sender.send(outcome);
        });
        poll_compression_result_save(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
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
    receiver: Rc<RefCell<Option<mpsc::Receiver<CompressionSaveOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
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
                    Some(CompressionSaveOutcome::Failed)
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_compression_result_save(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
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

fn paste_conversion_image(app: &AppWindow) -> bool {
    let state = app.global::<AppState>();
    if state.get_conversion_processing() || state.get_conversion_saving() {
        return true;
    }
    if state.get_conversion_images().row_count() >= MAX_CONVERSION_IMAGES {
        state.set_conversion_message(
            compression_limit_message(state.get_language().as_str() == "en").into(),
        );
        return true;
    }
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return false;
    };
    if let Ok(image) = clipboard.get_image() {
        let Some(rgba) = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.into_owned(),
        ) else {
            return false;
        };
        let Ok(bytes) = encode_png_rgba(&rgba, rgba.width(), rgba.height()) else {
            return false;
        };
        let directory = app_data_dir().join("toolbox").join("conversion-inputs");
        if !ensure_managed_subdirectory(&directory) {
            return false;
        }
        let path = directory.join(format!("pasted-{}.png", Uuid::new_v4()));
        if atomic_write_file(&path, &bytes).is_err() {
            return false;
        }
        add_conversion_paths(app, vec![path]);
        return true;
    }
    let Ok(text) = clipboard.get_text() else {
        return false;
    };
    add_conversion_from_drag_data(app, TEXT_PLAIN_MIME, &text)
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

fn start_local_conversion(app: &AppWindow) {
    let state = app.global::<AppState>();
    if state.get_conversion_processing() || state.get_conversion_saving() {
        return;
    }
    let target_format = state.get_conversion_target_format().to_string();
    if conversion_format_extension(&target_format).is_none() {
        state.set_conversion_message(
            if state.get_language().as_str() == "en" {
                "Choose a supported output format"
            } else {
                "请选择受支持的输出格式"
            }
            .into(),
        );
        return;
    }

    let mut images = state.get_conversion_images().iter().collect::<Vec<_>>();
    if images.is_empty() {
        state.set_conversion_message(
            if state.get_language().as_str() == "en" {
                "Add at least one image first"
            } else {
                "请先添加需要转换的图片"
            }
            .into(),
        );
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
    set_conversion_images(&state, images);
    let data_directory = app_data_dir();
    for result_path in abandoned_results {
        let _ = remove_managed_toolbox_file(
            &data_directory,
            ManagedToolboxDirectory::ConversionResults,
            &result_path,
        );
    }
    state.set_conversion_processing(true);
    state.set_conversion_message(
        if state.get_language().as_str() == "en" {
            "Converting images locally..."
        } else {
            "正在转换中..."
        }
        .into(),
    );

    let output_dir = app_data_dir().join("toolbox").join("conversion-results");
    if !ensure_managed_subdirectory(&output_dir) {
        state.set_conversion_processing(false);
        state.set_conversion_message("无法创建安全的转换缓存目录".into());
        return;
    }
    let (sender, receiver) = mpsc::channel::<ConversionOutcome>();
    std::thread::spawn(move || {
        run_local_conversion_worker(inputs, target_format, output_dir, sender);
    });
    poll_local_conversion(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
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
    receiver: Rc<RefCell<Option<mpsc::Receiver<ConversionOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(ConversionOutcome::Interrupted)
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_local_conversion(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        match outcome {
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
                receiver.borrow_mut().take();
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
        }
        if keep_polling {
            poll_local_conversion(app_weak, receiver);
        }
    });
}

fn start_conversion_result_save(app: &AppWindow, result_path: PathBuf, source_name: String) {
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
    let _ = slint::spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title("保存转换结果")
            .set_file_name(default_name)
            .add_filter(filter_name, &[extension.as_str()])
            .save_file()
            .await
        else {
            return;
        };
        let destination = normalize_conversion_destination(file.path(), &extension);
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_conversion_saving(true);
        state.set_conversion_message(
            if state.get_language().as_str() == "en" {
                "Saving the converted image..."
            } else {
                "正在保存转换结果..."
            }
            .into(),
        );
        let (sender, receiver) = mpsc::channel::<ConversionSaveOutcome>();
        let data_directory = app_data_dir();
        std::thread::spawn(move || {
            let outcome = copy_and_release_managed_toolbox_result(
                &result_path,
                &destination,
                &data_directory,
                ManagedToolboxDirectory::ConversionResults,
            )
            .map(|released| ConversionSaveOutcome::Saved {
                destination,
                released_result_path: released.then_some(result_path),
            })
            .unwrap_or(ConversionSaveOutcome::Failed);
            let _ = sender.send(outcome);
        });
        poll_conversion_result_save(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
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
    receiver: Rc<RefCell<Option<mpsc::Receiver<ConversionSaveOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
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
                    Some(ConversionSaveOutcome::Failed)
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_conversion_result_save(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
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

fn paste_crop_image(app: &AppWindow) -> bool {
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
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return false;
    };
    if let Ok(image) = clipboard.get_image() {
        let Some(rgba) = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.into_owned(),
        ) else {
            return false;
        };
        let Ok(bytes) = encode_png_rgba(&rgba, rgba.width(), rgba.height()) else {
            return false;
        };
        let directory = app_data_dir().join("toolbox").join("crop-inputs");
        if !ensure_managed_subdirectory(&directory) {
            return false;
        }
        let path = directory.join(format!("pasted-{}.png", Uuid::new_v4()));
        if atomic_write_file(&path, &bytes).is_err() {
            return false;
        }
        add_crop_paths(app, vec![path]);
        return true;
    }
    let Ok(text) = clipboard.get_text() else {
        return false;
    };
    add_crop_from_drag_data(app, TEXT_PLAIN_MIME, &text)
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
    Success { bytes: Vec<u8>, source_path: String },
    Failure,
}

fn start_crop_save(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();
    if state.get_crop_processing() {
        return;
    }
    let source_path = state.get_crop_source_path().to_string();
    if source_path.trim().is_empty() {
        state.set_crop_message(
            if state.get_language().as_str() == "en" {
                "Upload an image first"
            } else {
                "请先上传图片"
            }
            .into(),
        );
        return;
    }
    let steps = state.get_crop_transform_steps().to_string();
    let crop_rect = (
        state.get_crop_x(),
        state.get_crop_y(),
        state.get_crop_width(),
        state.get_crop_height(),
    );
    state.set_crop_processing(true);
    state.set_crop_message(
        if state.get_language().as_str() == "en" {
            "Saving the cropped image..."
        } else {
            "正在保存裁剪结果..."
        }
        .into(),
    );

    let (sender, receiver) = mpsc::channel::<CropSaveOutcome>();
    std::thread::spawn(move || {
        let outcome = process_crop_result(Path::new(&source_path), &steps, crop_rect)
            .map(|bytes| CropSaveOutcome::Success { bytes, source_path })
            .unwrap_or(CropSaveOutcome::Failure);
        let _ = sender.send(outcome);
    });
    poll_crop_save(app.as_weak(), store, Rc::new(RefCell::new(Some(receiver))));
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

fn poll_crop_save(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<CropSaveOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(60), move || {
        let outcome = {
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
                    Some(CropSaveOutcome::Failure)
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_crop_save(app_weak, store, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_crop_processing(false);
        match outcome {
            CropSaveOutcome::Success { bytes, source_path } => {
                if save_crop_asset(&app, &store, &source_path, &bytes).is_ok() {
                    state.set_crop_message(
                        if state.get_language().as_str() == "en" {
                            "Saved to My Assets"
                        } else {
                            "已保存到我的资产"
                        }
                        .into(),
                    );
                } else {
                    state.set_crop_message(
                        if state.get_language().as_str() == "en" {
                            "The cropped image could not be saved"
                        } else {
                            "裁剪结果保存失败，请重试"
                        }
                        .into(),
                    );
                }
            }
            CropSaveOutcome::Failure => state.set_crop_message(
                if state.get_language().as_str() == "en" {
                    "The image could not be processed"
                } else {
                    "图片处理失败，请更换图片后重试"
                }
                .into(),
            ),
        }
    });
}

fn save_crop_asset(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    source_path: &str,
    bytes: &[u8],
) -> Result<()> {
    let (width, height) = generated_image_dimensions(bytes)?;
    let source_title = Path::new(source_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("图片");
    let title = format!("{} 裁剪", short_text(source_title, 18));
    let result_path = save_generated_bytes(app, bytes, &title)?;
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
        source_path: result_path,
        reference_paths: vec![source_path.to_string()],
        cutout_done: false,
        remove_black_done: false,
        upscale_done: false,
        is_new: false,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let mut store = store.borrow_mut();
    store.assets.insert(0, item);
    store.notifications.insert(
        0,
        NotificationData {
            id: Uuid::new_v4().to_string(),
            title: format!("图片裁剪完成：{title}"),
            model: "图片裁剪".to_string(),
            time: now,
            reason: String::new(),
            success: true,
            read: false,
        },
    );
    save_local_store(app, &store);
    push_all(app, &store);
    Ok(())
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
    let state = app.global::<AppState>();
    if state.get_session_state().as_str() != "online" {
        state.set_auth_open(true);
        state.set_watermark_message(
            if state.get_language().as_str() == "en" {
                "Sign in and connect to the service before removing a watermark"
            } else {
                "请先登录并连接服务后再去水印"
            }
            .into(),
        );
        return;
    }
    if context.backend.is_none() || state.get_watermark_processing() {
        return;
    }
    let Some(session_scope) = current_generation_session_scope(&context) else {
        state.set_watermark_message("登录状态已变化，请重新发起去水印".into());
        return;
    };

    let source = PathBuf::from(state.get_watermark_source_path().to_string());
    let persisted_source = match persist_reference_source(&source) {
        Ok(path) => path,
        Err(_) => {
            state.set_watermark_message(
                if state.get_language().as_str() == "en" {
                    "The selected image could not be prepared"
                } else {
                    "无法处理所选图片，请更换图片后重试"
                }
                .into(),
            );
            return;
        }
    };
    state.set_watermark_source_path(persisted_source.display().to_string().into());
    let client_request_id = Uuid::new_v4().simple().to_string();
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints(std::slice::from_ref(&persisted_source)) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                state.set_watermark_message(format!("原图校验失败：{error}").into());
                return;
            }
        };
    let record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id,
        owner_user_id: session_scope.owner_user_id.clone(),
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
    if upsert_pending_generation_scoped(
        record.clone(),
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    )
    .is_err()
    {
        state.set_watermark_message(
            if state.get_language().as_str() == "en" {
                "The task could not be saved locally"
            } else {
                "任务准备失败，请重试"
            }
            .into(),
        );
        return;
    }
    launch_watermark_removal(app, context, record, false);
}

pub(super) fn resume_pending_watermark_removal(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
) {
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !generation_scope_matches_context(&context, &session_scope) {
        return;
    }
    if app.global::<AppState>().get_watermark_processing() {
        return;
    }
    if let Some(source_path) = record.reference_paths.first() {
        let path = PathBuf::from(source_path);
        if path.is_file() {
            let state = app.global::<AppState>();
            state.set_watermark_source_path(source_path.clone().into());
            state.set_watermark_source_name(
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .into(),
            );
            if let Ok(image) = load_preview_image(&path, PreviewPurpose::Canvas) {
                state.set_watermark_source_image(image);
            }
        }
    }
    launch_watermark_removal(app, context, record, true);
}

fn launch_watermark_removal(
    app: &AppWindow,
    context: AppContext,
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
    let state = app.global::<AppState>();
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

    let source_path = record.reference_paths.first().cloned().unwrap_or_default();
    let (sender, receiver) = mpsc::channel::<WatermarkOutcome>();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || run_watermark_worker(backend, worker_scope, record, sender));
    poll_watermark_outcomes(
        app.as_weak(),
        context,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
        source_path,
    );
}

fn run_watermark_worker(
    backend: Arc<BackendRuntime>,
    session_scope: SessionScope,
    mut record: PendingGenerationRecord,
    sender: mpsc::Sender<WatermarkOutcome>,
) {
    if record.owner_user_id != session_scope.owner_user_id
        || record.auth_epoch != session_scope.auth_epoch
        || !backend_generation_scope_active(&backend, &session_scope)
    {
        return;
    }
    let api = GenerationApi::new(backend.api.clone());
    let verified_delivery_file_ids = match sanitize_recovered_delivery_paths(&mut record) {
        Ok(file_ids) => file_ids,
        Err(_) => {
            let _ = sender.send(WatermarkOutcome::Failure {
                reason: "本地去水印恢复记录无法安全更新，已暂停交付，请重启后重试"
                    .to_string(),
            });
            return;
        }
    };
    if record.terminal {
        if let Some(saved) = record
            .deliveries
            .iter()
            .find(|delivery| verified_delivery_file_ids.contains(&delivery.file_id))
        {
            let delivery = (!saved.acknowledged).then(|| DeliveryConfirmation {
                client_request_id: record.client_request_id.clone(),
                item_index: saved.item_index,
                task_id: record.server_task_id.clone(),
                file_id: saved.file_id.clone(),
                sha256: saved.sha256.clone(),
                size_bytes: saved.size_bytes,
                failed_asset_id: None,
            });
            let _ = sender.send(WatermarkOutcome::Recovered {
                local_path: saved.local_path.clone(),
                delivery,
            });
            return;
        }
    }

    let mut uploaded = record.uploaded_file_ids.clone();
    if uploaded.is_empty() && record.server_task_id.is_empty() {
        if !generation_references_match(&record) {
            let _ = sender.send(WatermarkOutcome::Failure {
                reason: "原图内容已变化，恢复任务已暂停，请重新发起".to_string(),
            });
            return;
        }
        let Some(path) = record.reference_paths.first() else {
            let _ = sender.send(WatermarkOutcome::Failure {
                reason: "找不到待处理的原图，请重新上传".to_string(),
            });
            return;
        };
        match api.upload_reference_scoped(Path::new(path), &session_scope) {
            Ok(file_id) => {
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                if !matches!(
                    update_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                        |item| item.uploaded_file_ids = snapshot,
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
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                }
                let _ = sender.send(WatermarkOutcome::Failure {
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
        match api.create_watermark_removal_scoped(&request, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                    let _ = sender.send(WatermarkOutcome::CreditInsufficient {
                        message: "本次去水印需要 20 积分，请先充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                }
                let _ = sender.send(WatermarkOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    } else {
        match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(WatermarkOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    };

    record.server_task_id = detail.id.clone();
    let server_task_id = detail.id.clone();
    let server_id_snapshot = server_task_id.clone();
    let uploaded_snapshot = uploaded.clone();
    if !matches!(
        update_pending_generation_scoped(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            &record.client_request_id,
            |item| {
                item.server_task_id = server_id_snapshot;
                item.uploaded_file_ids = uploaded_snapshot;
            },
        ),
        Ok(true)
    ) {
        return;
    }
    let _ = sender.send(WatermarkOutcome::Accepted {
        task_id: server_task_id.clone(),
    });

    loop {
        if !backend_generation_scope_active(&backend, &session_scope) {
            return;
        }
        let _ = sender.send(WatermarkOutcome::Progress {
            percent: detail.progress_percent,
        });
        if let Some(item) = detail.items.iter().find(|item| item.status == "succeeded") {
            if let Some(file) = item.file.as_ref() {
                match api.download_verified_scoped(file, &session_scope) {
                    Ok(bytes) => {
                        if !matches!(
                            update_pending_generation_scoped(
                                &session_scope.owner_user_id,
                                session_scope.auth_epoch,
                                &record.client_request_id,
                                |pending| {
                                    pending.terminal = true;
                                    pending.expected_success_count = 1;
                                },
                            ),
                            Ok(true)
                        ) {
                            return;
                        }
                        let _ = sender.send(WatermarkOutcome::Success {
                            bytes,
                            delivery: DeliveryConfirmation {
                                client_request_id: record.client_request_id.clone(),
                                item_index: item.index,
                                task_id: server_task_id,
                                file_id: file.id.clone(),
                                sha256: file.sha256.clone(),
                                size_bytes: file.size_bytes.parse().unwrap_or(0),
                                failed_asset_id: None,
                            },
                        });
                        return;
                    }
                    Err(error) if detail.terminal() => {
                        let _ = sender.send(WatermarkOutcome::Failure {
                            reason: error.generation_message(),
                        });
                        return;
                    }
                    Err(_) => {}
                }
            }
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
                update_pending_generation_scoped(
                    &session_scope.owner_user_id,
                    session_scope.auth_epoch,
                    &record.client_request_id,
                    |pending| {
                        pending.terminal = true;
                        pending.expected_success_count = 0;
                    },
                ),
                Ok(true)
            ) {
                return;
            }
            let _ = sender.send(WatermarkOutcome::Failure { reason });
            return;
        }
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        detail = match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(WatermarkOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        };
    }
}

fn poll_watermark_outcomes(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<WatermarkOutcome>>>>,
    source_path: String,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(WatermarkOutcome::Failure {
                        reason: "去水印任务已中断，请重试".to_string(),
                    })
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_watermark_outcomes(app_weak, context, session_scope, receiver, source_path);
            return;
        };
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        match outcome {
            WatermarkOutcome::Accepted { task_id } => {
                state.set_watermark_progress(state.get_watermark_progress().max(8));
                state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        format!("Task {task_id} is queued")
                    } else {
                        "任务已提交，正在排队处理...".to_string()
                    }
                    .into(),
                );
            }
            WatermarkOutcome::Progress { percent } => {
                state.set_watermark_progress(percent.clamp(1, 99));
                state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "Removing the watermark..."
                    } else {
                        "正在智能修复水印区域..."
                    }
                    .into(),
                );
            }
            WatermarkOutcome::Success { bytes, delivery } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                match save_watermark_asset(&app, &context.store, &source_path, &bytes) {
                    Ok((result_path, result_image)) => {
                        let result_name = Path::new(&result_path)
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("去水印结果")
                            .to_string();
                        state.set_watermark_result_path(result_path.clone().into());
                        state.set_watermark_result_name(result_name.into());
                        state.set_watermark_result_image(result_image);
                        state.set_watermark_progress(100);
                        state.set_watermark_processing(false);
                        state.set_watermark_message(
                            if state.get_language().as_str() == "en" {
                                "Watermark removed and saved to My Assets / Other"
                            } else {
                                "处理完成，已保存到“我的资产 / 其他”"
                            }
                            .into(),
                        );
                        let saved = pending_delivery_saved(
                            &session_scope.owner_user_id,
                            session_scope.auth_epoch,
                            &delivery.client_request_id,
                            &delivery,
                            &result_path,
                        );
                        if matches!(saved, Ok(true)) {
                            acknowledge_delivery_after_local_save(
                                app.as_weak(),
                                context.clone(),
                                session_scope.clone(),
                                delivery,
                            );
                        }
                    }
                    Err(error) => {
                        state.set_watermark_processing(false);
                        state.set_watermark_message(
                            format!("处理结果保存失败：{}", zh_error(&error.to_string())).into(),
                        );
                    }
                }
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            WatermarkOutcome::Recovered {
                local_path,
                delivery,
            } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let path = PathBuf::from(&local_path);
                let locally_verified = delivery.as_ref().map_or(true, |delivery| {
                    recovered_delivery_path_matches(
                        &local_path,
                        &delivery.sha256,
                        delivery.size_bytes,
                    )
                });
                match (
                    locally_verified,
                    load_preview_image(&path, PreviewPurpose::Canvas),
                ) {
                    (true, Ok(image)) => {
                        state.set_watermark_result_path(local_path.clone().into());
                        state.set_watermark_result_name(
                            path.file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or("去水印结果")
                                .into(),
                        );
                        state.set_watermark_result_image(image);
                        state.set_watermark_progress(100);
                        state.set_watermark_processing(false);
                        state.set_watermark_message("已恢复上次完成的去水印结果".into());
                        if let Some(delivery) = delivery {
                            acknowledge_delivery_after_local_save(
                                app.as_weak(),
                                context.clone(),
                                session_scope.clone(),
                                delivery,
                            );
                        }
                    }
                    _ => {
                        let retrying = delivery.as_ref().is_some_and(|delivery| {
                            matches!(
                                clear_recovered_delivery_local_path(
                                    &session_scope,
                                    &delivery.client_request_id,
                                    &delivery.file_id,
                                ),
                                Ok(true)
                            )
                        });
                        if retrying {
                            state.set_watermark_processing(false);
                            state.set_watermark_message(
                                "本地去水印结果校验失败，正在从服务端重新下载...".into(),
                            );
                            recover_pending_generations(&app, context.clone());
                        } else {
                            state.set_watermark_processing(false);
                            state.set_watermark_message(
                                "本地去水印结果已损坏，且暂时无法恢复，请重启后重试".into(),
                            );
                        }
                    }
                }
            }
            WatermarkOutcome::CreditInsufficient { message } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                state.set_watermark_processing(false);
                state.set_watermark_progress(0);
                state.set_watermark_message(message.clone().into());
                state.set_credit_insufficient_message(message.into());
                state.set_credit_insufficient_open(true);
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            WatermarkOutcome::Failure { reason } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                state.set_watermark_processing(false);
                state.set_watermark_progress(0);
                state.set_watermark_message(reason.into());
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
        }
        if keep_polling {
            poll_watermark_outcomes(app_weak, context, session_scope, receiver, source_path);
        }
    });
}

fn save_watermark_asset(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    source_path: &str,
    bytes: &[u8],
) -> Result<(String, Image)> {
    let (width, height) = generated_image_dimensions(bytes)?;
    let source = Path::new(source_path);
    let source_title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("图片");
    let title = format!("{} 去水印", short_text(source_title, 18));
    let result_path = save_generated_bytes(app, bytes, &title)?;
    let image = load_preview_image(Path::new(&result_path), PreviewPurpose::Canvas)?;
    let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: String::new(),
        title: title.clone(),
        category: "other".to_string(),
        kind: "game".to_string(),
        time: now.clone(),
        prompt: "去除图片水印".to_string(),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: "1K".to_string(),
        model: "去水印".to_string(),
        origin: "watermark_removal".to_string(),
        width,
        height,
        source_path: result_path.clone(),
        reference_paths: (!source_path.is_empty())
            .then(|| source_path.to_string())
            .into_iter()
            .collect(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done: false,
        is_new: false,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let notification = NotificationData {
        id: Uuid::new_v4().to_string(),
        title: format!("去水印处理完成：{title}"),
        model: "去水印".to_string(),
        time: now,
        reason: String::new(),
        success: true,
        read: false,
    };
    let mut store = store.borrow_mut();
    persist_generated_asset_checked(app, &mut store, item, notification, false, None)?;
    push_all(app, &store);
    Ok((result_path, image))
}

fn start_image_colorization(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if state.get_session_state().as_str() != "online" {
        state.set_auth_open(true);
        state.set_colorize_message(
            if state.get_language().as_str() == "en" {
                "Sign in and connect to the service before colorizing a photo"
            } else {
                "请先登录并连接服务后再进行老照片上色"
            }
            .into(),
        );
        return;
    }
    if context.backend.is_none() || state.get_colorize_processing() {
        return;
    }
    let Some(session_scope) = current_generation_session_scope(&context) else {
        state.set_colorize_message("登录状态已变化，请重新发起老照片上色".into());
        return;
    };

    let source = PathBuf::from(state.get_colorize_source_path().to_string());
    if let Err(error) = set_colorization_source_from_path(app, &source) {
        set_colorization_source_error(app, &error);
        return;
    }
    let persisted_source = match persist_colorization_source(&source) {
        Ok(path) => path,
        Err(_) => {
            state.set_colorize_message(
                if state.get_language().as_str() == "en" {
                    "The selected image could not be prepared"
                } else {
                    "无法处理所选图片，请更换图片后重试"
                }
                .into(),
            );
            return;
        }
    };
    state.set_colorize_source_path(persisted_source.display().to_string().into());
    let client_request_id = Uuid::new_v4().simple().to_string();
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints(std::slice::from_ref(&persisted_source)) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                state.set_colorize_message(format!("原图校验失败：{error}").into());
                return;
            }
        };
    let record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id,
        owner_user_id: session_scope.owner_user_id.clone(),
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
    if upsert_pending_generation_scoped(
        record.clone(),
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    )
    .is_err()
    {
        state.set_colorize_message(
            if state.get_language().as_str() == "en" {
                "The task could not be saved locally"
            } else {
                "任务准备失败，请重试"
            }
            .into(),
        );
        return;
    }
    launch_image_colorization(app, context, record, false);
}

pub(super) fn resume_pending_image_colorization(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
) {
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !generation_scope_matches_context(&context, &session_scope) {
        return;
    }
    if app.global::<AppState>().get_colorize_processing() {
        return;
    }
    if let Some(source_path) = record.reference_paths.first() {
        let path = PathBuf::from(source_path);
        if path.is_file() {
            let state = app.global::<AppState>();
            state.set_colorize_source_path(source_path.clone().into());
            state.set_colorize_source_name(
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .into(),
            );
            if let Ok(image) = load_preview_image(&path, PreviewPurpose::Canvas) {
                state.set_colorize_source_image(image);
            }
        }
    }
    launch_image_colorization(app, context, record, true);
}

fn launch_image_colorization(
    app: &AppWindow,
    context: AppContext,
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
    let state = app.global::<AppState>();
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

    let source_path = record.reference_paths.first().cloned().unwrap_or_default();
    let (sender, receiver) = mpsc::channel::<ImageColorizationOutcome>();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        run_image_colorization_worker(backend, worker_scope, record, sender)
    });
    poll_image_colorization_outcomes(
        app.as_weak(),
        context,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
        source_path,
    );
}

fn run_image_colorization_worker(
    backend: Arc<BackendRuntime>,
    session_scope: SessionScope,
    mut record: PendingGenerationRecord,
    sender: mpsc::Sender<ImageColorizationOutcome>,
) {
    if record.owner_user_id != session_scope.owner_user_id
        || record.auth_epoch != session_scope.auth_epoch
        || !backend_generation_scope_active(&backend, &session_scope)
    {
        return;
    }
    let api = GenerationApi::new(backend.api.clone());
    let verified_delivery_file_ids = match sanitize_recovered_delivery_paths(&mut record) {
        Ok(file_ids) => file_ids,
        Err(_) => {
            let _ = sender.send(ImageColorizationOutcome::Failure {
                reason: "本地上色恢复记录无法安全更新，已暂停交付，请重启后重试"
                    .to_string(),
            });
            return;
        }
    };
    if record.terminal {
        if let Some(saved) = record
            .deliveries
            .iter()
            .find(|delivery| verified_delivery_file_ids.contains(&delivery.file_id))
        {
            let delivery = (!saved.acknowledged).then(|| DeliveryConfirmation {
                client_request_id: record.client_request_id.clone(),
                item_index: saved.item_index,
                task_id: record.server_task_id.clone(),
                file_id: saved.file_id.clone(),
                sha256: saved.sha256.clone(),
                size_bytes: saved.size_bytes,
                failed_asset_id: None,
            });
            let _ = sender.send(ImageColorizationOutcome::Recovered {
                local_path: saved.local_path.clone(),
                delivery,
            });
            return;
        }
    }

    let mut uploaded = record.uploaded_file_ids.clone();
    if uploaded.is_empty() && record.server_task_id.is_empty() {
        if !generation_references_match(&record) {
            let _ = sender.send(ImageColorizationOutcome::Failure {
                reason: "原图内容已变化，恢复任务已暂停，请重新发起".to_string(),
            });
            return;
        }
        let Some(path) = record.reference_paths.first() else {
            let _ = sender.send(ImageColorizationOutcome::Failure {
                reason: "找不到待上色的原图，请重新上传".to_string(),
            });
            return;
        };
        match api.upload_reference_scoped(Path::new(path), &session_scope) {
            Ok(file_id) => {
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                if !matches!(
                    update_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                        |item| item.uploaded_file_ids = snapshot,
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
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                }
                let _ = sender.send(ImageColorizationOutcome::Failure {
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
        match api.create_image_colorization_scoped(&request, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                    let _ = sender.send(ImageColorizationOutcome::CreditInsufficient {
                        message: "本次老照片上色需要 20 积分，请先充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                }
                let _ = sender.send(ImageColorizationOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    } else {
        match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ImageColorizationOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    };

    record.server_task_id = detail.id.clone();
    let server_task_id = detail.id.clone();
    let server_id_snapshot = server_task_id.clone();
    let uploaded_snapshot = uploaded.clone();
    if !matches!(
        update_pending_generation_scoped(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            &record.client_request_id,
            |item| {
                item.server_task_id = server_id_snapshot;
                item.uploaded_file_ids = uploaded_snapshot;
            },
        ),
        Ok(true)
    ) {
        return;
    }
    let _ = sender.send(ImageColorizationOutcome::Accepted {
        task_id: server_task_id.clone(),
    });

    loop {
        if !backend_generation_scope_active(&backend, &session_scope) {
            return;
        }
        let _ = sender.send(ImageColorizationOutcome::Progress {
            percent: detail.progress_percent,
        });
        if let Some(item) = detail.items.iter().find(|item| item.status == "succeeded") {
            if let Some(file) = item.file.as_ref() {
                match api.download_verified_scoped(file, &session_scope) {
                    Ok(bytes) => {
                        if !matches!(
                            update_pending_generation_scoped(
                                &session_scope.owner_user_id,
                                session_scope.auth_epoch,
                                &record.client_request_id,
                                |pending| {
                                    pending.terminal = true;
                                    pending.expected_success_count = 1;
                                },
                            ),
                            Ok(true)
                        ) {
                            return;
                        }
                        let _ = sender.send(ImageColorizationOutcome::Success {
                            bytes,
                            delivery: DeliveryConfirmation {
                                client_request_id: record.client_request_id.clone(),
                                item_index: item.index,
                                task_id: server_task_id,
                                file_id: file.id.clone(),
                                sha256: file.sha256.clone(),
                                size_bytes: file.size_bytes.parse().unwrap_or(0),
                                failed_asset_id: None,
                            },
                        });
                        return;
                    }
                    Err(error) if detail.terminal() => {
                        let _ = sender.send(ImageColorizationOutcome::Failure {
                            reason: error.generation_message(),
                        });
                        return;
                    }
                    Err(_) => {}
                }
            }
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
                update_pending_generation_scoped(
                    &session_scope.owner_user_id,
                    session_scope.auth_epoch,
                    &record.client_request_id,
                    |pending| {
                        pending.terminal = true;
                        pending.expected_success_count = 0;
                    },
                ),
                Ok(true)
            ) {
                return;
            }
            let _ = sender.send(ImageColorizationOutcome::Failure { reason });
            return;
        }
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        detail = match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ImageColorizationOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        };
    }
}

fn poll_image_colorization_outcomes(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<ImageColorizationOutcome>>>>,
    source_path: String,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(ImageColorizationOutcome::Failure {
                        reason: "老照片上色任务已中断，请重试".to_string(),
                    })
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_image_colorization_outcomes(
                app_weak,
                context,
                session_scope,
                receiver,
                source_path,
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
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        match outcome {
            ImageColorizationOutcome::Accepted { task_id } => {
                state.set_colorize_progress(state.get_colorize_progress().max(8));
                state.set_colorize_message(
                    if state.get_language().as_str() == "en" {
                        format!("Task {task_id} is queued")
                    } else {
                        "任务已提交，正在排队处理...".to_string()
                    }
                    .into(),
                );
            }
            ImageColorizationOutcome::Progress { percent } => {
                state.set_colorize_progress(percent.clamp(1, 99));
                state.set_colorize_message(
                    if state.get_language().as_str() == "en" {
                        "Colorizing the photo..."
                    } else {
                        "正在为老照片还原自然色彩..."
                    }
                    .into(),
                );
            }
            ImageColorizationOutcome::Success { bytes, delivery } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                match save_image_colorization_asset(&app, &context.store, &source_path, &bytes) {
                    Ok((result_path, result_image)) => {
                        let result_name = Path::new(&result_path)
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("上色结果")
                            .to_string();
                        state.set_colorize_result_path(result_path.clone().into());
                        state.set_colorize_result_name(result_name.into());
                        state.set_colorize_result_image(result_image);
                        state.set_colorize_progress(100);
                        state.set_colorize_processing(false);
                        state.set_colorize_message(
                            if state.get_language().as_str() == "en" {
                                "Photo colorized and saved to My Assets / Other"
                            } else {
                                "上色完成，已保存到“我的资产 / 其他”"
                            }
                            .into(),
                        );
                        let saved = pending_delivery_saved(
                            &session_scope.owner_user_id,
                            session_scope.auth_epoch,
                            &delivery.client_request_id,
                            &delivery,
                            &result_path,
                        );
                        if matches!(saved, Ok(true)) {
                            acknowledge_delivery_after_local_save(
                                app.as_weak(),
                                context.clone(),
                                session_scope.clone(),
                                delivery,
                            );
                        }
                    }
                    Err(error) => {
                        state.set_colorize_processing(false);
                        state.set_colorize_message(
                            format!("上色结果保存失败：{}", zh_error(&error.to_string())).into(),
                        );
                    }
                }
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            ImageColorizationOutcome::Recovered {
                local_path,
                delivery,
            } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let path = PathBuf::from(&local_path);
                let locally_verified = delivery.as_ref().map_or(true, |delivery| {
                    recovered_delivery_path_matches(
                        &local_path,
                        &delivery.sha256,
                        delivery.size_bytes,
                    )
                });
                match (
                    locally_verified,
                    load_preview_image(&path, PreviewPurpose::Canvas),
                ) {
                    (true, Ok(image)) => {
                        state.set_colorize_result_path(local_path.clone().into());
                        state.set_colorize_result_name(
                            path.file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or("上色结果")
                                .into(),
                        );
                        state.set_colorize_result_image(image);
                        state.set_colorize_progress(100);
                        state.set_colorize_processing(false);
                        state.set_colorize_message("已恢复上次完成的老照片上色结果".into());
                        if let Some(delivery) = delivery {
                            acknowledge_delivery_after_local_save(
                                app.as_weak(),
                                context.clone(),
                                session_scope.clone(),
                                delivery,
                            );
                        }
                    }
                    _ => {
                        let retrying = delivery.as_ref().is_some_and(|delivery| {
                            matches!(
                                clear_recovered_delivery_local_path(
                                    &session_scope,
                                    &delivery.client_request_id,
                                    &delivery.file_id,
                                ),
                                Ok(true)
                            )
                        });
                        if retrying {
                            state.set_colorize_processing(false);
                            state.set_colorize_message(
                                "本地上色结果校验失败，正在从服务端重新下载...".into(),
                            );
                            recover_pending_generations(&app, context.clone());
                        } else {
                            state.set_colorize_processing(false);
                            state.set_colorize_message(
                                "本地上色结果已损坏，且暂时无法恢复，请重启后重试".into(),
                            );
                        }
                    }
                }
            }
            ImageColorizationOutcome::CreditInsufficient { message } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                state.set_colorize_processing(false);
                state.set_colorize_progress(0);
                state.set_colorize_message(message.clone().into());
                state.set_credit_insufficient_message(message.into());
                state.set_credit_insufficient_open(true);
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            ImageColorizationOutcome::Failure { reason } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                state.set_colorize_processing(false);
                state.set_colorize_progress(0);
                state.set_colorize_message(reason.into());
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
        }
        if keep_polling {
            poll_image_colorization_outcomes(
                app_weak,
                context,
                session_scope,
                receiver,
                source_path,
            );
        }
    });
}

fn save_image_colorization_asset(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    source_path: &str,
    bytes: &[u8],
) -> Result<(String, Image)> {
    let (width, height) = generated_image_dimensions(bytes)?;
    let source = Path::new(source_path);
    let source_title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("老照片");
    let title = format!("{} 上色", short_text(source_title, 18));
    let result_path = save_generated_bytes(app, bytes, &title)?;
    let image = load_preview_image(Path::new(&result_path), PreviewPurpose::Canvas)?;
    let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: String::new(),
        title: title.clone(),
        category: "other".to_string(),
        kind: "game".to_string(),
        time: now.clone(),
        prompt: "老照片上色".to_string(),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality_from_actual_dimensions(width, height),
        model: "老照片上色".to_string(),
        origin: "image_colorization".to_string(),
        width,
        height,
        source_path: result_path.clone(),
        reference_paths: (!source_path.is_empty())
            .then(|| source_path.to_string())
            .into_iter()
            .collect(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done: false,
        is_new: false,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let notification = NotificationData {
        id: Uuid::new_v4().to_string(),
        title: format!("老照片上色完成：{title}"),
        model: "老照片上色".to_string(),
        time: now,
        reason: String::new(),
        success: true,
        read: false,
    };
    let mut store = store.borrow_mut();
    persist_generated_asset_checked(app, &mut store, item, notification, false, None)?;
    push_all(app, &store);
    Ok((result_path, image))
}

#[cfg(test)]
mod local_image_tests {
    use super::*;

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
        let managed_directory = managed_toolbox_directory(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
        );
        let other_managed_directory = managed_toolbox_directory(
            &test_root,
            ManagedToolboxDirectory::ConversionInputs,
        );
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
        let linked_directory = managed_toolbox_directory(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
        );
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
        let input_directory = managed_toolbox_directory(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
        );
        let result_directory = managed_toolbox_directory(
            &test_root,
            ManagedToolboxDirectory::CompressionResults,
        );
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
        let result_directory = managed_toolbox_directory(
            &test_root,
            ManagedToolboxDirectory::ConversionResults,
        );
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
        let compression_inputs = managed_toolbox_directory(
            &test_root,
            ManagedToolboxDirectory::CompressionInputs,
        );
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
