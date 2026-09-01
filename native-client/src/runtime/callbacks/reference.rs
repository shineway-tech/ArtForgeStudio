use super::*;
use crate::platform::{self, ExternalDropPosition, ExternalImageDrop};
use std::io::Read;

const MAX_DROPPED_IMAGE_BYTES: u64 = 100 * 1024 * 1024;

pub(super) fn wire_reference_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let app_weak = app.as_weak();
            let store = store.clone();
            drop(app);
            let _ = slint::spawn_local(async move {
                let Some(files) = rfd::AsyncFileDialog::new()
                    .add_filter("Images", crate::image_formats::picker_image_extensions())
                    .pick_files()
                    .await
                else {
                    return;
                };
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                for file in files {
                    add_reference_from_path(&app, &store, file.path());
                }
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_paste_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let canvas = state.get_page().as_str() == "canvas";
            let max_references = max_reference_images_for_category(&category);
            let Ok(mut clipboard) = arboard::Clipboard::new() else {
                return false;
            };
            let Ok(img) = clipboard.get_image() else {
                return false;
            };
            let source_path = match persist_clipboard_reference(&img) {
                Ok(path) => path.display().to_string(),
                Err(_) => {
                    state.set_generation_status("无法保存剪贴板参考图".into());
                    return true;
                }
            };
            let mut store = store.borrow_mut();
            let references = references_for_context_mut(&mut store, &category, canvas);
            if references.len() >= max_references {
                state.set_generation_status(reference_limit_message(max_references).into());
                return true;
            }
            references.push(ReferenceData {
                id: Uuid::new_v4().to_string(),
                source_path,
            });
            push_references_for_context(&app, &store, canvas);
            save_local_store(&app, &store);
            state.set_generation_status("已从剪贴板粘贴参考图".into());
            true
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference_from_transfer(move |transfer| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Ok(data) = transfer.plain_text() else {
                return false;
            };
            if let Some(url) = external_image_url(data.as_str()) {
                start_external_reference_import(&app, store.clone(), url);
                return true;
            }
            add_reference_from_drag_data(&app, &store, TEXT_PLAIN_MIME, data.as_str())
        });
    }

    state.on_start_thumbnail_drag_preview(move |data| {
        let Some(path) = drag_data_to_path(data.as_str()) else {
            return false;
        };
        drag_preview::start_thumbnail_drag_preview(path)
    });

    {
        let app_weak = app.as_weak();
        state.on_start_thumbnail_file_drag(move |data| {
            let Some(path) = drag_data_to_path(data.as_str()) else {
                return false;
            };
            let result = drag_preview::start_thumbnail_file_drag(path);
            if let Some(app) = app_weak.upgrade() {
                reset_pointer_after_native_drag(&app);
            }
            result
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_remove_reference(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let id = id.to_string();
                let state = app.global::<AppState>();
                let category = resolve_category(&state.get_asset_type().to_string(), "");
                let canvas = state.get_page().as_str() == "canvas";
                references_for_context_mut(&mut store.borrow_mut(), &category, canvas)
                    .retain(|r| r.id != id);
                push_references_for_context(&app, &store.borrow(), canvas);
                save_local_store(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_clear_references(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let canvas = state.get_page().as_str() == "canvas";
            references_for_context_mut(&mut store.borrow_mut(), &category, canvas).clear();
            push_references_for_context(&app, &store.borrow(), canvas);
            save_local_store(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_reference(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let canvas = state.get_page().as_str() == "canvas";
            let store_ref = store.borrow();
            let Some(item) = references_for_context(&store_ref, &category, canvas)
                .iter()
                .find(|r| r.id == id)
                .cloned()
            else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_viewer_id(item.id.into());
            state.set_viewer_source("reference".into());
            state.set_viewer_source_path(item.source_path.clone().into());
            state.set_viewer_image(
                load_preview_image(Path::new(&item.source_path), PreviewPurpose::Viewer)
                    .unwrap_or_default(),
            );
            state.set_viewer_title("参考图".into());
            state.set_viewer_prompt("".into());
            state.set_viewer_prompt_lines(1);
            state.set_viewer_time("".into());
            state.set_viewer_ratio("".into());
            state.set_viewer_quality("".into());
            state.set_viewer_model("".into());
            state.set_viewer_width(0);
            state.set_viewer_height(0);
            state.set_viewer_cutout_done(false);
            state.set_viewer_remove_black_done(false);
            state.set_viewer_upscale_done(false);
            state.set_viewer_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_process_external_image_drops(move || {
            if let Some(app) = app_weak.upgrade() {
                process_external_image_drops(&app, &store);
            }
        });
    }
}

fn push_references_for_context(app: &AppWindow, store: &Store, canvas: bool) {
    if canvas {
        push_canvas_references(app, store);
    } else {
        push_references(app, store);
    }
}

fn process_external_image_drops(app: &AppWindow, store: &Rc<RefCell<Store>>) {
    let drops = platform::take_external_image_drops();
    if app.global::<AppState>().get_directory_migration_open() { return; }
    let page = app.global::<AppState>().get_page();
    if matches!(page.as_str(), "generation" | "canvas") {
        for drop in drops {
            match drop {
                ExternalImageDrop::Paths(paths, position)
                    if external_drop_inside_reference_input(app, position.as_ref()) =>
                {
                    for path in paths {
                        add_reference_from_path(app, store, &path);
                    }
                }
                #[cfg(windows)]
                ExternalImageDrop::Text(data, position)
                    if external_drop_inside_reference_input(app, position.as_ref()) =>
                {
                    if let Some(url) = external_image_url(&data) {
                        start_external_reference_import(app, store.clone(), url);
                    } else {
                        add_reference_from_drag_data(app, store, TEXT_PLAIN_MIME, &data);
                    }
                }
                _ => {}
            }
        }
    } else if page.as_str() == "toolbox-enhance" {
        for drop in drops {
            match drop {
                ExternalImageDrop::Paths(paths, _) => {
                    image_enhancement_callbacks::add_enhancement_paths(app, paths);
                }
                #[cfg(windows)]
                ExternalImageDrop::Text(data, _) => {
                    image_enhancement_callbacks::add_enhancement_from_drag_data(
                        app,
                        TEXT_PLAIN_MIME,
                        &data,
                    );
                }
            }
        }
    } else if page.as_str() == "toolbox-watermark" {
        for drop in drops {
            match drop {
                ExternalImageDrop::Paths(paths, _) => {
                    toolbox_callbacks::add_watermark_paths(app, paths);
                }
                #[cfg(windows)]
                ExternalImageDrop::Text(data, _) => {
                    toolbox_callbacks::add_watermark_from_drag_data(
                        app,
                        TEXT_PLAIN_MIME,
                        &data,
                    );
                }
            }
        }
    } else if page.as_str() == "toolbox-colorize" {
        for drop in drops {
            match drop {
                ExternalImageDrop::Paths(paths, _) => {
                    toolbox_callbacks::add_colorization_paths(app, paths);
                }
                #[cfg(windows)]
                ExternalImageDrop::Text(data, _) => {
                    toolbox_callbacks::add_colorization_from_drag_data(
                        app,
                        TEXT_PLAIN_MIME,
                        &data,
                    );
                }
            }
        }
    } else if page.as_str() == "toolbox-compress" {
        for drop in drops {
            match drop {
                ExternalImageDrop::Paths(paths, _) => {
                    toolbox_callbacks::add_compression_paths(app, paths);
                }
                #[cfg(windows)]
                ExternalImageDrop::Text(data, _) => {
                    toolbox_callbacks::add_compression_from_drag_data(
                        app,
                        TEXT_PLAIN_MIME,
                        &data,
                    );
                }
            }
        }
    } else if page.as_str() == "toolbox-convert" {
        for drop in drops {
            match drop {
                ExternalImageDrop::Paths(paths, _) => {
                    toolbox_callbacks::add_conversion_paths(app, paths);
                }
                #[cfg(windows)]
                ExternalImageDrop::Text(data, _) => {
                    toolbox_callbacks::add_conversion_from_drag_data(
                        app,
                        TEXT_PLAIN_MIME,
                        &data,
                    );
                }
            }
        }
    } else if page.as_str() == "toolbox-crop" {
        for drop in drops {
            match drop {
                ExternalImageDrop::Paths(paths, _) => {
                    toolbox_callbacks::add_crop_paths(app, paths);
                }
                #[cfg(windows)]
                ExternalImageDrop::Text(data, _) => {
                    toolbox_callbacks::add_crop_from_drag_data(app, TEXT_PLAIN_MIME, &data);
                }
            }
        }
    }
}

fn external_drop_inside_reference_input(
    app: &AppWindow,
    position: Option<&ExternalDropPosition>,
) -> bool {
    let Some(position) = position else {
        return false;
    };
    let state = app.global::<AppState>();
    let scale = if position.physical {
        app.window().scale_factor().max(f32::EPSILON)
    } else {
        1.0
    };
    let x = position.x / scale;
    let y = position.y / scale;
    let left = state.get_reference_drop_x();
    let top = state.get_reference_drop_y();
    let width = state.get_reference_drop_width();
    let height = state.get_reference_drop_height();
    width > 0.0 && height > 0.0 && x >= left && x <= left + width && y >= top && y <= top + height
}

fn start_external_reference_import(app: &AppWindow, store: Rc<RefCell<Store>>, url: String) {
    let state = app.global::<AppState>();
    state.set_generation_status(
        if state.get_language().as_str() == "en" {
            "Importing the dropped image..."
        } else {
            "正在导入拖入的图片..."
        }
        .into(),
    );
    let (sender, receiver) = mpsc::channel::<std::result::Result<PathBuf, String>>();
    std::thread::spawn(move || {
        let _ = sender.send(download_external_reference(&url));
    });
    poll_external_reference_import(app.as_weak(), store, Rc::new(RefCell::new(Some(receiver))));
}

fn poll_external_reference_import(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
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
            poll_external_reference_import(app_weak, store, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match result {
            Ok(path) => {
                add_reference_from_path(&app, &store, &path);
                remove_managed_reference_import(&path);
            }
            Err(error) => app.global::<AppState>().set_generation_status(error.into()),
        }
    });
}

pub(super) fn download_external_reference(url: &str) -> std::result::Result<PathBuf, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("ElunviCanvas/1.0")
        .build()
        .map_err(|_| "无法创建图片下载请求".to_string())?;
    let response = client
        .get(url)
        .send()
        .map_err(|_| "无法下载拖入的网页图片".to_string())?
        .error_for_status()
        .map_err(|_| "网页图片地址不可访问".to_string())?;
    if response.content_length().unwrap_or(0) > MAX_DROPPED_IMAGE_BYTES {
        return Err("拖入的图片超过 100 MB 安全限制".to_string());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_DROPPED_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取网页图片失败".to_string())?;
    if bytes.len() as u64 > MAX_DROPPED_IMAGE_BYTES {
        return Err("拖入的图片超过 100 MB 安全限制".to_string());
    }
    let format = image::guess_format(&bytes).map_err(|_| "拖入的网址不是有效图片".to_string())?;
    image::load_from_memory(&bytes).map_err(|_| "拖入的网址不是受支持的图片".to_string())?;
    let extension = match format {
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::WebP => "webp",
        image::ImageFormat::Gif => "gif",
        image::ImageFormat::Bmp => "bmp",
        image::ImageFormat::Tiff => "tiff",
        _ => "png",
    };
    let directory = app_data_dir().join("references").join("imports");
    if !ensure_managed_subdirectory(&directory) {
        return Err("无法创建参考图目录".to_string());
    }
    let destination = directory.join(format!("dragged-{}.{}", Uuid::new_v4(), extension));
    atomic_write_file(&destination, &bytes).map_err(|_| "无法保存拖入的图片".to_string())?;
    Ok(destination)
}

const STALE_REFERENCE_IMPORT_AGE: Duration = Duration::from_secs(24 * 60 * 60);

fn reference_import_dir() -> PathBuf {
    app_data_dir().join("references").join("imports")
}

fn is_managed_reference_import_path(path: &Path) -> bool {
    path.parent().is_some_and(|parent| parent == reference_import_dir())
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("dragged-") && !name.ends_with(".part"))
}

fn remove_managed_reference_import(path: &Path) {
    if is_managed_reference_import_path(path)
        && safe_managed_subdirectory(&reference_import_dir())
        && fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
    {
        let _ = fs::remove_file(path);
    }
}

pub(super) fn cleanup_stale_reference_imports() {
    let now = std::time::SystemTime::now();
    let directory = reference_import_dir();
    if !safe_managed_subdirectory(&directory) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_managed_reference_import_path(&path) {
            continue;
        }
        let stale = fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_REFERENCE_IMPORT_AGE);
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod managed_import_tests {
    use super::*;

    #[test]
    fn managed_reference_import_filter_never_accepts_external_paths() {
        let managed = reference_import_dir().join("dragged-example.png");
        assert!(is_managed_reference_import_path(&managed));
        assert!(!is_managed_reference_import_path(
            &app_data_dir().join("out").join("dragged-example.png")
        ));
        assert!(!is_managed_reference_import_path(
            &reference_import_dir().join("unrelated.png")
        ));
        assert!(!is_managed_reference_import_path(
            &reference_import_dir().join("dragged-example.png.part")
        ));
    }
}
