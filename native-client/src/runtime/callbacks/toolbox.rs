use super::*;

const MAX_COMPRESSION_IMAGES: usize = 50;

pub(super) fn wire_toolbox_callbacks(app: &AppWindow) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        state.on_choose_compression_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(paths) = rfd::FileDialog::new()
                .add_filter("Images", &["jpg", "jpeg", "png", "webp", "bmp"])
                .pick_files()
            else {
                return;
            };
            add_compression_paths(&app, paths);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_add_compression_images_from_drag(move |mime_type, data| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            add_compression_from_drag_data(&app, mime_type.as_str(), data.as_str())
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
            let id = id.to_string();
            let images = state
                .get_compression_images()
                .iter()
                .filter(|item| item.id.as_str() != id)
                .collect::<Vec<_>>();
            set_compression_images(&state, images);
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
            set_compression_images(&state, Vec::new());
            state.set_compression_message("".into());
            state.set_compression_estimated_credits("--".into());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_start_compression(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_compression_images().row_count() == 0 {
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
            if state.get_compression_mode().as_str() == "size"
                && state
                    .get_compression_target_kb()
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .is_none()
            {
                state.set_compression_message(
                    if state.get_language().as_str() == "en" {
                        "Enter a valid target size"
                    } else {
                        "请输入有效的目标文件大小"
                    }
                    .into(),
                );
                return;
            }
            state.set_compression_processing(false);
            state.set_compression_message(
                if state.get_language().as_str() == "en" {
                    "Image compression is waiting for backend configuration"
                } else {
                    "图片压缩能力等待后端配置"
                }
                .into(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_choose_enhance_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
            else {
                return;
            };
            let state = app.global::<AppState>();
            match load_image(&path) {
                Ok(image) => {
                    let name = path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default()
                        .to_string();
                    state.set_enhance_source_path(path.display().to_string().into());
                    state.set_enhance_source_name(name.into());
                    state.set_enhance_source_image(image);
                    state.set_enhance_result_path("".into());
                    state.set_enhance_result_name("".into());
                    state.set_enhance_result_image(Image::default());
                    state.set_enhance_processing(false);
                    state.set_enhance_message("".into());
                }
                Err(_) => state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "The selected file is not a supported image"
                    } else {
                        "所选文件不是受支持的图片"
                    }
                    .into(),
                ),
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_start_enhance(move |quality| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_enhance_source_path().trim().is_empty() {
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Upload an image first"
                    } else {
                        "请先上传图片"
                    }
                    .into(),
                );
                return;
            }
            let quality = quality.to_ascii_uppercase();
            if !matches!(quality.as_str(), "1K" | "2K" | "4K") {
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Choose a valid quality"
                    } else {
                        "请选择有效的清晰度"
                    }
                    .into(),
                );
                return;
            }
            state.set_enhance_quality(quality.into());
            state.set_enhance_processing(false);
            state.set_enhance_message(
                if state.get_language().as_str() == "en" {
                    "Image enhancement is waiting for backend configuration"
                } else {
                    "图片变清晰能力等待后端配置"
                }
                .into(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reveal_enhance_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_enhance_result_path().to_string());
            if !path.is_file() {
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "No enhanced image is available yet"
                    } else {
                        "暂无可查看的清晰处理结果"
                    }
                    .into(),
                );
                return;
            }
            match reveal_path_in_file_manager(&path) {
                Ok(_) => state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Opened the image folder"
                    } else {
                        "已打开图片所在文件夹"
                    }
                    .into(),
                ),
                Err(error) => state.set_enhance_message(
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
        state.on_choose_watermark_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
            else {
                return;
            };
            let state = app.global::<AppState>();
            match load_image(&path) {
                Ok(image) => {
                    let name = path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default()
                        .to_string();
                    state.set_watermark_source_path(path.display().to_string().into());
                    state.set_watermark_source_name(name.into());
                    state.set_watermark_source_image(image);
                    state.set_watermark_result_path("".into());
                    state.set_watermark_result_name("".into());
                    state.set_watermark_result_image(Image::default());
                    state.set_watermark_processing(false);
                    state.set_watermark_progress(0);
                    state.set_watermark_message("".into());
                }
                Err(_) => state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "The selected file is not a supported image"
                    } else {
                        "所选文件不是受支持的图片"
                    }
                    .into(),
                ),
            }
        });
    }

    {
        let app_weak = app.as_weak();
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
            state.set_watermark_processing(false);
            state.set_watermark_progress(0);
            state.set_watermark_message(
                if state.get_language().as_str() == "en" {
                    "The watermark-removal service is waiting for backend configuration"
                } else {
                    "去水印服务等待后端配置，接口开放后将在右侧显示处理结果"
                }
                .into(),
            );
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
}

pub(super) fn add_compression_from_drag_data(
    app: &AppWindow,
    mime_type: &str,
    data: &str,
) -> bool {
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
        if !canonical.is_file()
            || !is_compression_image_path(&canonical)
            || !known_paths.insert(source_path.clone())
        {
            skipped += 1;
            continue;
        }
        let Ok(decoded) = image::open(&canonical) else {
            skipped += 1;
            continue;
        };
        let rgba = decoded.to_rgba8();
        let preview = slint_image_from_rgba(&rgba, rgba.width(), rgba.height());
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
        });
        added += 1;
    }

    set_compression_images(&state, images);
    state.set_compression_estimated_credits("--".into());
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
    if state.get_compression_images().row_count() >= MAX_COMPRESSION_IMAGES {
        state.set_compression_message(compression_limit_message(state.get_language().as_str() == "en").into());
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
        let directory = app_data_dir()
            .join("toolbox")
            .join("compression-inputs");
        if fs::create_dir_all(&directory).is_err() {
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
    state.set_compression_images(ModelRc::new(VecModel::from(images)));
}

fn is_compression_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "bmp"
            )
        })
        .unwrap_or(false)
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

fn compression_add_message(
    english: bool,
    added: usize,
    skipped: usize,
    total: usize,
) -> String {
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
