use super::*;

const MAX_VIDEO_IMAGE_BYTES: u64 = 100 * 1024 * 1024;

struct PreparedVideoImage {
    id: String,
    title: String,
    path: String,
    preview: preview::PreparedPreview,
}

struct PreparedVideoImages {
    images: Vec<PreparedVideoImage>,
    skipped: usize,
}

fn video_image_key(path: &Path) -> String {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

fn video_asset_candidates(store: &Store) -> Vec<(String, PathBuf)> {
    store
        .assets
        .iter()
        .filter(|asset| !asset.source_path.is_empty() && asset.source_path != "failed")
        .map(|asset| (asset.title.clone(), PathBuf::from(&asset.source_path)))
        .collect()
}

// Decode off the UI thread. Both import routes share validation and canonical-path deduplication.
fn prepare_video_images(
    candidates: Vec<(String, PathBuf)>,
    should_continue: impl Fn() -> bool,
) -> PreparedVideoImages {
    let mut images = Vec::new();
    let mut skipped = 0;
    let mut seen = BTreeSet::new();
    for (title, path) in candidates {
        if !should_continue() {
            break;
        }
        if !fs::metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_VIDEO_IMAGE_BYTES)
        {
            skipped += 1;
            continue;
        }
        let id = video_image_key(&path);
        if !seen.insert(id.clone()) {
            continue;
        }
        match prepare_preview_image_if(&path, PreviewPurpose::Gallery, &should_continue) {
            Ok(Some(preview)) => images.push(PreparedVideoImage {
                id,
                title,
                path: path.to_string_lossy().into_owned(),
                preview,
            }),
            Ok(None) => break,
            Err(_) => skipped += 1,
        }
    }
    PreparedVideoImages { images, skipped }
}

fn materialize_video_images(prepared: PreparedVideoImages) -> (Vec<VideoImageItem>, usize) {
    let rows = prepared
        .images
        .into_iter()
        .map(|image| VideoImageItem {
            id: image.id.into(),
            title: image.title.into(),
            source_path: image.path.into(),
            image: materialize_prepared_preview(image.preview),
            selected: false,
            added: false,
        })
        .collect();
    (rows, prepared.skipped)
}

pub(super) fn reset_video_images(state: &AppState, epoch: &AtomicU64) {
    cancel_video_image_work(state, epoch);
    let path = state.get_viewer_source_path();
    let rows = if path.trim().is_empty() {
        vec![]
    } else {
        vec![VideoImageItem {
            id: video_image_key(Path::new(path.as_str())).into(),
            title: state.get_viewer_title(),
            source_path: path,
            image: state.get_viewer_image(),
            selected: false,
            added: false,
        }]
    };
    state.set_video_images(ModelRc::new(VecModel::from(rows)));
    state.set_video_images_status("可继续添加图片；移除仅从本次列表移除，不删除原文件".into());
}

pub(super) fn cancel_video_image_work(state: &AppState, epoch: &AtomicU64) {
    epoch.fetch_add(1, Ordering::SeqCst);
    state.set_video_images_loading(false);
    state.set_video_image_dialog("".into());
    state.set_video_asset_choices(ModelRc::default());
    state.set_video_asset_selected_count(0);
}

pub(super) fn video_image_generation_error(state: &AppState) -> Option<&'static str> {
    if state.get_video_images_loading() {
        Some("图片仍在加载，请稍候")
    } else {
        match state.get_video_images().row_count() {
            0 => Some("请先添加图片"),
            1 => None,
            _ => Some("当前视频接口仅支持单图，多图生成尚未开放"),
        }
    }
}

fn update_video_image_source(
    state: &AppState,
    quote_epoch: &AtomicU64,
    request_id: &Mutex<String>,
) {
    // Any collection change invalidates both an in-flight quote and its idempotency binding.
    quote_epoch.fetch_add(1, Ordering::SeqCst);
    request_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clear();
    state.set_video_quote_loading(false);
    state.set_video_quote_ready(false);
    state.set_video_quote_id("".into());
    state.set_video_credit_cost("".into());
    state.set_video_source_file_id("".into());
    let only = (state.get_video_images().row_count() == 1)
        .then(|| state.get_video_images().row_data(0))
        .flatten();
    state.set_video_source_path(
        only.as_ref()
            .map(|row| row.source_path.clone())
            .unwrap_or_default(),
    );
    state.set_video_source_image(
        only.as_ref()
            .map(|row| row.image.clone())
            .unwrap_or_default(),
    );
    state.set_video_source_title(only.map(|row| row.title).unwrap_or_default());
    if let Some(error) = video_image_generation_error(state) {
        state.set_video_status(error.into());
    } else {
        state.invoke_request_video_quote(
            state.get_video_aspect_ratio(),
            state.get_video_resolution(),
            state.get_video_duration_seconds(),
        );
    }
}

fn append_video_images(
    state: &AppState,
    rows: Vec<VideoImageItem>,
    skipped: usize,
    quote_epoch: &AtomicU64,
    request_id: &Mutex<String>,
) {
    let mut images: Vec<_> = state.get_video_images().iter().collect();
    let mut ids: BTreeSet<_> = images.iter().map(|image| image.id.clone()).collect();
    let before = images.len();
    for mut row in rows {
        if ids.insert(row.id.clone()) {
            row.selected = false;
            row.added = false;
            images.push(row);
        }
    }
    let added = images.len() - before;
    if added > 0 {
        state.set_video_images(ModelRc::new(VecModel::from(images)));
        update_video_image_source(state, quote_epoch, request_id);
    }
    state.set_video_images_status(
        if skipped > 0 {
            format!("已添加 {added} 张图片；{skipped} 个文件无法读取、已丢失或超过 100 MB")
        } else if added == 0 {
            "所选图片已在列表中，无需重复添加".to_string()
        } else {
            format!("已添加 {added} 张图片 · 移除不会删除原文件")
        }
        .into(),
    );
}

fn video_image_work_is_current(state: &AppState, epoch: &AtomicU64, expected: u64) -> bool {
    state.get_page() == "video-generation" && epoch.load(Ordering::SeqCst) == expected
}

fn finish_video_image_import(
    state: &AppState,
    prepared: PreparedVideoImages,
    asset_picker: bool,
    epoch: &AtomicU64,
    expected: u64,
    quote_epoch: &AtomicU64,
    request_id: &Mutex<String>,
) {
    if !video_image_work_is_current(state, epoch, expected) {
        return;
    }
    state.set_video_images_loading(false);
    let (mut rows, skipped) = materialize_video_images(prepared);
    if asset_picker {
        let added: BTreeSet<_> = state.get_video_images().iter().map(|row| row.id).collect();
        for row in &mut rows {
            row.added = added.contains(&row.id);
        }
        state.set_video_images_status(
            if skipped == 0 {
                format!("共 {} 张可用图片", rows.len())
            } else {
                format!(
                    "共 {} 张可用图片，已跳过 {skipped} 个不可用文件",
                    rows.len()
                )
            }
            .into(),
        );
        state.set_video_asset_choices(ModelRc::new(VecModel::from(rows)));
    } else {
        append_video_images(state, rows, skipped, quote_epoch, request_id);
    }
}

fn start_video_image_import(
    app: &AppWindow,
    candidates: Vec<(String, PathBuf)>,
    asset_picker: bool,
    epoch: Arc<AtomicU64>,
    quote_epoch: Arc<AtomicU64>,
    request_id: Arc<Mutex<String>>,
) {
    let expected = epoch.fetch_add(1, Ordering::SeqCst) + 1;
    app.global::<AppState>().set_video_images_loading(true);
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let prepared =
            prepare_video_images(candidates, || epoch.load(Ordering::SeqCst) == expected);
        let _ = weak.upgrade_in_event_loop(move |app| {
            finish_video_image_import(
                &app.global::<AppState>(),
                prepared,
                asset_picker,
                &epoch,
                expected,
                &quote_epoch,
                &request_id,
            );
        });
    });
}

pub(super) fn wire_video_image_callbacks(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    quote_epoch: Arc<AtomicU64>,
    request_id: Arc<Mutex<String>>,
) -> Arc<AtomicU64> {
    let state = app.global::<AppState>();
    let epoch = Arc::new(AtomicU64::new(0));
    {
        let weak = app.as_weak();
        let epoch = epoch.clone();
        let quote_epoch = quote_epoch.clone();
        let request_id = request_id.clone();
        state.on_open_video_asset_picker(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_video_generating() || state.get_video_images_loading() {
                return;
            }
            state.set_video_image_dialog("assets".into());
            state.set_video_asset_choices(ModelRc::default());
            state.set_video_asset_selected_count(0);
            state.set_video_images_status("正在读取我的资产…".into());
            start_video_image_import(
                &app,
                video_asset_candidates(&store.borrow()),
                true,
                epoch.clone(),
                quote_epoch.clone(),
                request_id.clone(),
            );
        });
    }
    {
        let weak = app.as_weak();
        let epoch = epoch.clone();
        let quote_epoch = quote_epoch.clone();
        let request_id = request_id.clone();
        state.on_upload_video_images(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_video_generating() || state.get_video_images_loading() {
                return;
            }
            let files = rfd::FileDialog::new()
                .set_title("选择图片（支持多选）")
                .add_filter("Images", crate::image_formats::picker_image_extensions())
                .pick_files();
            let Some(files) = files.filter(|files| !files.is_empty()) else {
                return;
            };
            state.set_video_image_dialog("".into());
            let candidates = files
                .into_iter()
                .map(|path| {
                    let title = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    (title, path)
                })
                .collect();
            start_video_image_import(
                &app,
                candidates,
                false,
                epoch.clone(),
                quote_epoch.clone(),
                request_id.clone(),
            );
        });
    }
    {
        let weak = app.as_weak();
        let epoch = epoch.clone();
        state.on_close_video_image_dialog(move || {
            if let Some(app) = weak.upgrade() {
                cancel_video_image_work(&app.global::<AppState>(), &epoch);
            }
        });
    }
    {
        let weak = app.as_weak();
        state.on_toggle_video_asset(move |id| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_video_images_loading() || state.get_video_generating() {
                return;
            }
            let mut rows: Vec<_> = state.get_video_asset_choices().iter().collect();
            for row in &mut rows {
                if row.id == id && !row.added {
                    row.selected = !row.selected;
                }
            }
            state.set_video_asset_selected_count(
                rows.iter().filter(|row| row.selected && !row.added).count() as i32,
            );
            state.set_video_asset_choices(ModelRc::new(VecModel::from(rows)));
        });
    }
    {
        let weak = app.as_weak();
        let epoch = epoch.clone();
        let quote_epoch = quote_epoch.clone();
        let request_id = request_id.clone();
        state.on_confirm_video_assets(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_video_images_loading()
                || state.get_video_generating()
                || state.get_video_image_dialog() != "assets"
            {
                return;
            }
            let selected: Vec<_> = state
                .get_video_asset_choices()
                .iter()
                .filter(|row| row.selected && !row.added)
                .collect();
            if selected.is_empty() {
                return;
            }
            cancel_video_image_work(&state, &epoch);
            append_video_images(&state, selected, 0, &quote_epoch, &request_id);
        });
    }
    {
        let weak = app.as_weak();
        state.on_remove_video_image(move |id| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_video_generating() || state.get_video_images_loading() {
                return;
            }
            let before = state.get_video_images().row_count();
            let rows: Vec<_> = state
                .get_video_images()
                .iter()
                .filter(|row| row.id != id)
                .collect();
            if rows.len() == before {
                return;
            }
            state.set_video_images(ModelRc::new(VecModel::from(rows)));
            state.set_video_images_status("已从本次列表移除，原文件仍保留".into());
            update_video_image_source(&state, &quote_epoch, &request_id);
        });
    }
    epoch
}

#[cfg(test)]
#[path = "video_images_tests.rs"]
mod tests;
