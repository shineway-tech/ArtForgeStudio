use super::*;
use std::io::Read;

const MAX_DROPPED_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

pub(super) fn wire_reference_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(files) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_files()
            {
                let category =
                    resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
                let max_references = max_reference_images_for_category(&category);
                let mut store = store.borrow_mut();
                let references = references_for_category_mut(&mut store.references, &category);
                if references.len() >= max_references {
                    app.global::<AppState>()
                        .set_generation_status(reference_limit_message(max_references).into());
                    return;
                }
                for path in files {
                    if references.len() >= max_references {
                        break;
                    }
                    if let Ok(image) = load_image(&path) {
                        references.push(ReferenceData {
                            id: Uuid::new_v4().to_string(),
                            image,
                            source_path: path.display().to_string(),
                        });
                    }
                }
                if references.len() >= max_references {
                    app.global::<AppState>()
                        .set_generation_status(reference_limit_message(max_references).into());
                }
                push_references(&app, &store);
            }
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
            let max_references = max_reference_images_for_category(&category);
            let Ok(mut clipboard) = arboard::Clipboard::new() else {
                return false;
            };
            let Ok(img) = clipboard.get_image() else {
                return false;
            };
            let mut store = store.borrow_mut();
            let references = references_for_category_mut(&mut store.references, &category);
            if references.len() >= max_references {
                state.set_generation_status(reference_limit_message(max_references).into());
                return true;
            }
            let image = image_from_clipboard(img);
            references.push(ReferenceData {
                id: Uuid::new_v4().to_string(),
                image,
                source_path: String::new(),
            });
            push_references(&app, &store);
            state.set_generation_status("已从剪贴板粘贴参考图".into());
            true
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference_from_drag(move |mime_type, data| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            if let Some(url) = external_image_url(data.as_str()) {
                start_external_reference_import(&app, store.clone(), url);
                return true;
            }
            add_reference_from_drag_data(&app, &store, mime_type.as_str(), data.as_str())
        });
    }

    state.on_start_thumbnail_drag_preview(move |data| {
        let Some(path) = drag_data_to_path(data.as_str()) else {
            return false;
        };
        drag_preview::start_thumbnail_drag_preview(path)
    });

    state.on_start_thumbnail_file_drag(move |data| {
        let Some(path) = drag_data_to_path(data.as_str()) else {
            return false;
        };
        drag_preview::start_thumbnail_file_drag(path)
    });

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_remove_reference(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let id = id.to_string();
                let category =
                    resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
                references_for_category_mut(&mut store.borrow_mut().references, &category)
                    .retain(|r| r.id != id);
                push_references(&app, &store.borrow());
            }
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
            let category =
                resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
            let store_ref = store.borrow();
            let Some(item) = references_for_category(&store_ref.references, &category)
                .iter()
                .find(|r| r.id == id)
                .cloned()
            else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_viewer_id(item.id.into());
            state.set_viewer_source("reference".into());
            state.set_viewer_image(item.image);
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
}

fn start_external_reference_import(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    url: String,
) {
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
    poll_external_reference_import(
        app.as_weak(),
        store,
        Rc::new(RefCell::new(Some(receiver))),
    );
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
            }
            Err(error) => app
                .global::<AppState>()
                .set_generation_status(error.into()),
        }
    });
}

fn download_external_reference(url: &str) -> std::result::Result<PathBuf, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("ArtForgeStudio/1.0")
        .build()
        .map_err(|_| "无法创建图片下载请求".to_string())?;
    let response = client
        .get(url)
        .send()
        .map_err(|_| "无法下载拖入的网页图片".to_string())?
        .error_for_status()
        .map_err(|_| "网页图片地址不可访问".to_string())?;
    if response.content_length().unwrap_or(0) > MAX_DROPPED_IMAGE_BYTES {
        return Err("拖入的图片超过 25 MB 限制".to_string());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_DROPPED_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取网页图片失败".to_string())?;
    if bytes.len() as u64 > MAX_DROPPED_IMAGE_BYTES {
        return Err("拖入的图片超过 25 MB 限制".to_string());
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
    fs::create_dir_all(&directory).map_err(|_| "无法创建参考图目录".to_string())?;
    let destination = directory.join(format!("dragged-{}.{}", Uuid::new_v4(), extension));
    atomic_write_file(&destination, &bytes).map_err(|_| "无法保存拖入的图片".to_string())?;
    Ok(destination)
}
