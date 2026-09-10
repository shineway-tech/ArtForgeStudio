use super::*;

const MAX_VIDEO_IMAGE_BYTES: u64 = 100 * 1024 * 1024;

struct PreparedVideoImage {
    id: String,
    source_asset_id: String,
    title: String,
    path: String,
    preview: preview::PreparedDeliveryPreview,
}

struct PreparedVideoImages {
    images: Vec<PreparedVideoImage>,
    skipped: usize,
}

fn video_image_key(path: &Path) -> String {
    // Lexical identity only: validation and source I/O belong to the held worker.
    let key=path.components().collect::<PathBuf>().to_string_lossy().into_owned();
    if cfg!(windows) { key.to_lowercase() } else { key }
}

fn video_asset_candidates(store: &Store) -> Vec<(String, PathBuf)> {
    store
        .assets
        .iter()
        .filter(|asset| !asset.source_path.is_empty() && asset.source_path != "failed")
        .map(|asset| (asset.title.clone(), PathBuf::from(&asset.source_path)))
        .collect()
}

fn video_asset_choice_rows(
    state: &AppState,
    store: &Store,
    persistence: &PrivatePersistence,
) -> (Vec<VideoImageItem>, Vec<(String, PathBuf)>) {
    let existing_previews = state
        .get_assets()
        .iter()
        .chain(state.get_generations().iter())
        .filter(|row| row.image.size().width > 0 && row.image.size().height > 0)
        .map(|row| {
            (
                (row.id.to_string(), row.source_path.to_string()),
                row.image,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let added_ids = state
        .get_video_images()
        .iter()
        .filter(|row| !row.source_asset_id.is_empty())
        .map(|row| row.source_asset_id.to_string())
        .collect::<BTreeSet<_>>();
    let added_paths = state
        .get_video_images()
        .iter()
        .map(|row| video_image_key(Path::new(row.source_path.as_str())))
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut missing_previews = Vec::new();

    let rows = store
        .assets
        .iter()
        .filter(|asset| {
            !asset.source_path.is_empty()
                && asset.source_path != "failed"
                && persistence.owns_path(Path::new(&asset.source_path))
        })
        .filter_map(|asset| {
            let path = Path::new(&asset.source_path);
            let id = video_image_key(path);
            if !seen.insert(id.clone()) {
                return None;
            }
            let image = existing_previews
                .get(&(asset.id.clone(), asset.source_path.clone()))
                .cloned()
                .unwrap_or_default();
            let has_preview = image.size().width > 0 && image.size().height > 0;
            if !has_preview {
                missing_previews.push((id.clone(), path.to_path_buf()));
            }
            Some(VideoImageItem {
                id: id.clone().into(),
                source_asset_id: asset.id.clone().into(),
                title: asset.title.clone().into(),
                subtitle: SharedString::default(),
                source_path: asset.source_path.clone().into(),
                image,
                has_preview,
                selected: false,
                added: added_ids.contains(&asset.id) || added_paths.contains(&id),
            })
        })
        .collect();
    (rows, missing_previews)
}

fn hydrate_next_video_asset_preview(
    weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    persistence: PrivatePersistence,
    mut pending: Vec<(String, PathBuf)>,
) {
    if pending.is_empty() {
        return;
    }
    let (id, path) = pending.remove(0);
    slint::Timer::single_shot(Duration::from_millis(1), move || {
        let Some(app) = weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        if state.get_video_image_dialog() != "assets" || !captured_video_binding(&store, &persistence) {
            return;
        }
        let image = preview::load_preview_image(&path, PreviewPurpose::Gallery).ok();
        let _ = apply_video_input(&store, &persistence, || {
            if state.get_video_image_dialog() != "assets" {
                return;
            }
            let Some(image) = image else { return; };
            let mut rows: Vec<_> = state.get_video_asset_choices().iter().collect();
            if let Some(row) = rows.iter_mut().find(|row| row.id == id) {
                row.image = image;
                row.has_preview = true;
                state.set_video_asset_choices(ModelRc::new(VecModel::from(rows)));
            }
        });
        hydrate_next_video_asset_preview(weak, store, persistence, pending);
    });
}


fn advance_video_image_epoch(epoch: &AtomicU64) -> Option<u64> {
    epoch.fetch_update(Ordering::SeqCst,Ordering::SeqCst,|value| value.checked_add(1)).ok().and_then(|value| value.checked_add(1)).filter(|value| *value != u64::MAX)
}
fn captured_video_binding(store: &Rc<RefCell<Store>>, persistence: &PrivatePersistence) -> bool {
    store.borrow().private_persistence.as_ref().is_some_and(|current| current.same_binding(persistence))
}
fn apply_video_input<R>(store: &Rc<RefCell<Store>>, persistence: &PrivatePersistence, apply: impl FnOnce()->R) -> Option<R> {
    let _activity=persistence.begin_activity().ok()?;
    if !captured_video_binding(store,persistence) { return None; }
    // same_binding takes the latch internally; never call it inside this closure.
    persistence.upgrade_latch().apply_if_open(apply).ok()
}
fn quote_current_video_image(state: &AppState, store: &Rc<RefCell<Store>>, persistence: &PrivatePersistence, quote_epoch: &AtomicU64) {
    if quote_epoch.load(Ordering::SeqCst)==u64::MAX || !captured_video_binding(store,persistence) { return; }
    // Recompute from the cached catalogue; image changes never issue HTTP requests.
    state.invoke_request_video_quote(state.get_video_aspect_ratio(),state.get_video_resolution(),state.get_video_duration_seconds());
}
fn validate_video_image_header(path:&Path, bytes:&[u8]) -> Result<()> {
    let mut reader=image::ImageReader::new(Cursor::new(bytes));
    if let Ok(format)=image::ImageFormat::from_path(path) { reader.set_format(format); }
    match reader.with_guessed_format()?.into_dimensions() {
        Ok((width,height)) => anyhow::ensure!(width>0 && height>0 && u64::from(width)*u64::from(height)<=100_000_000,"video image dimensions exceed policy"),
        Err(error) => {
            #[cfg(target_os="macos")]
            if path.extension().and_then(|extension|extension.to_str()).is_some_and(|extension|matches!(extension.to_ascii_lowercase().as_str(),"heic"|"heif")) { return Ok(()); }
            return Err(error.into());
        }
    }
    Ok(())
}
fn prepare_video_images(
    persistence: &PrivatePersistence, candidates: Vec<(String,PathBuf)>, should_continue: impl Fn()->bool,
) -> PreparedVideoImages {
    let mut images=Vec::new();
    let mut skipped=0;
    let mut seen=BTreeSet::new();
    for (title,path) in candidates {
        if !should_continue() || !persistence.is_current() { break; }
        let id=video_image_key(&path);
        if !seen.insert(id.clone()) { continue; }
        let prepared=(|| {
            let authority=persistence.storage_authority()?;
            let bytes=authority.read_image_source(&path,MAX_VIDEO_IMAGE_BYTES)?;
            if !should_continue() || !persistence.is_current() { return Ok(None); }
            // Header inspection bounds ordinary formats before allocation; native
            // HEIC/HEIF decoding still uses the same captured bytes, never this path.
            validate_video_image_header(&path,&bytes)?;
            let decoded=decode_image_bytes(&path,&bytes)?.0;
            anyhow::ensure!(decoded.width()>0 && decoded.height()>0 && u64::from(decoded.width())*u64::from(decoded.height())<=100_000_000,"video image dimensions exceed policy");
            if !should_continue() || !persistence.is_current() { return Ok(None); }
            let owned=persist_reference_image_for_namespace(&authority,&decoded)?;
            if !should_continue() || !persistence.is_current() { return Ok(None); }
            let preview=preview::prepare_owned_preview(persistence,&owned,PreviewPurpose::Gallery)?;
            if !should_continue() || !persistence.is_current() { return Ok(None); }
            Ok::<_,anyhow::Error>(Some(PreparedVideoImage { id,source_asset_id:String::new(),title,path:owned.to_string_lossy().into_owned(),preview }))
        })();
        match prepared { Ok(Some(image))=>images.push(image),Ok(None)=>break,Err(_)=>skipped+=1 }
    }
    PreparedVideoImages { images,skipped }
}
struct VideoImageImportWork {
    receiver:mpsc::Receiver<std::result::Result<PreparedVideoImages,DeliveryRetryError>>,
    cancel:Arc<std::sync::atomic::AtomicBool>,
}
fn poll_captured_video_images(
    weak:Weak<AppWindow>,store:Rc<RefCell<Store>>,persistence:PrivatePersistence,asset_picker:bool,
    epoch:Arc<AtomicU64>,expected:u64,quote_epoch:Arc<AtomicU64>,request_id:Arc<Mutex<String>>,work:VideoImageImportWork,
) {
    slint::Timer::single_shot(Duration::from_millis(50),move|| {
        let Some(app)=weak.upgrade()else{work.cancel.store(true,Ordering::SeqCst);return;};
        let state=app.global::<AppState>();
        if !video_image_work_is_current(&state,&epoch,expected) || !captured_video_binding(&store,&persistence) {
            work.cancel.store(true,Ordering::SeqCst);return;
        }
        let finished=finish_delivery_preparation(&work.cancel);
        if matches!(finished,Ok(true)) {
            poll_captured_video_images(weak,store,persistence,asset_picker,epoch,expected,quote_epoch,request_id,work);return;
        }
        let prepared=if finished.is_err(){None}else{match work.receiver.try_recv() {
            Ok(Ok(value))=>Some(value),
            Err(TryRecvError::Empty)=>{
                poll_captured_video_images(weak,store,persistence,asset_picker,epoch,expected,quote_epoch,request_id,work);
                return;
            }
            Ok(Err(_))|Err(TryRecvError::Disconnected)=>None,
        }};
        let changed=apply_video_input(&store,&persistence,|| {
            if !video_image_work_is_current(&state,&epoch,expected) { return false; }
            match prepared {
                Some(prepared)=>finish_video_image_import(&state,prepared,asset_picker,&epoch,expected,&quote_epoch,&request_id),
                None=>{ state.set_video_images_loading(false);state.set_video_images_status("图片读取未完成，原文件仍保留".into());false }
            }
        }).unwrap_or(false);
        if changed { quote_current_video_image(&state,&store,&persistence,&quote_epoch); }
    });
}
pub(super) fn start_captured_video_image_import(
    app:&AppWindow,store:Rc<RefCell<Store>>,persistence:PrivatePersistence,candidates:Vec<(String,PathBuf)>,
    asset_picker:bool,epoch:Arc<AtomicU64>,quote_epoch:Arc<AtomicU64>,request_id:Arc<Mutex<String>>,
) {
    let associations=if asset_picker {
        let mut associations=BTreeMap::<String,String>::new();
        for asset in &store.borrow().assets {
            let key=video_image_key(Path::new(&asset.source_path));
            associations.entry(key).and_modify(|id|{if *id!=asset.id{id.clear();}}).or_insert_with(||asset.id.clone());
        }
        associations
    }else{BTreeMap::new()};
    start_video_images_with_associations(app,store,persistence,candidates,asset_picker,epoch,quote_epoch,request_id,associations);
}
/// The viewer is the sole caller that carries its explicitly captured original
/// item ID. Ordinary picker input never inherits an earlier viewer association.
pub(super) fn start_captured_video_viewer_image_import(
    app:&AppWindow,store:Rc<RefCell<Store>>,persistence:PrivatePersistence,candidate:(String,PathBuf),source_id:String,
    epoch:Arc<AtomicU64>,quote_epoch:Arc<AtomicU64>,request_id:Arc<Mutex<String>>,
) {
    let mut associations=BTreeMap::new();associations.insert(video_image_key(&candidate.1),source_id);
    start_video_images_with_associations(app,store,persistence,vec![candidate],false,epoch,quote_epoch,request_id,associations);
}
fn start_video_images_with_associations(
    app:&AppWindow,store:Rc<RefCell<Store>>,persistence:PrivatePersistence,candidates:Vec<(String,PathBuf)>,
    asset_picker:bool,epoch:Arc<AtomicU64>,quote_epoch:Arc<AtomicU64>,request_id:Arc<Mutex<String>>,associations:BTreeMap<String,String>,
) {
    let expected=apply_video_input(&store,&persistence,|| {
        let state=app.global::<AppState>();
        if state.get_page()!="video-generation" || state.get_video_generating()
            || state.get_video_images().iter().any(|row|!persistence.owns_path(Path::new(row.source_path.as_str()))) { return None; }
        let expected=advance_video_image_epoch(&epoch)?;
        state.set_video_images_loading(true);
        Some(expected)
    }).flatten();
    let Some(expected)=expected else { return; };
    let worker_epoch=epoch.clone();
    let launched=spawn_delivery_preparation(&persistence,move|captured,activity,cancel| {
        let external=captured.upgrade_latch().begin_ordinary_external_worker()
            .map_err(|required|anyhow!(required.as_error().user_message()))?;
        let mut prepared=prepare_video_images(captured,candidates,|| !cancel.load(Ordering::SeqCst)
            && !activity.is_quiescing() && !external.is_cancelled() && worker_epoch.load(Ordering::SeqCst)==expected);
        for image in &mut prepared.images {image.source_asset_id=associations.get(&image.id).cloned().unwrap_or_default();}
        if external.is_cancelled() || activity.is_quiescing() { prepared.images.clear(); }
        drop(external);
        Ok(prepared)
    });
    match launched {
        Ok((cancel,receiver))=>poll_captured_video_images(app.as_weak(),store,persistence,asset_picker,epoch,expected,quote_epoch,request_id,
            VideoImageImportWork { receiver,cancel }),
        Err(_)=>{let _=apply_video_input(&store,&persistence,||{
            let state=app.global::<AppState>();if video_image_work_is_current(&state,&epoch,expected){
                state.set_video_images_loading(false);state.set_video_images_status("图片读取未能启动，请重试".into());
            }
        });},
    }
}

fn materialize_video_images(prepared: PreparedVideoImages) -> (Vec<VideoImageItem>, usize) {
    let rows = prepared
        .images
        .into_iter()
        .map(|image| VideoImageItem {
            id: image.id.into(),
            source_asset_id: image.source_asset_id.into(),
            title: image.title.into(),
            subtitle: SharedString::default(),
            source_path: image.path.into(),
            image: preview::materialize_delivery_preview(&image.preview),
            has_preview: true,
            selected: false,
            added: false,
        })
        .collect();
    (rows, prepared.skipped)
}

pub(super) fn reset_video_images(state: &AppState, epoch: &AtomicU64) {
    // Caller owns the current Store/latch guard. Its viewer seed must enter through
    // start_captured_video_image_import AFTER page initialization and guard release.
    cancel_video_image_work(state,epoch);
    state.set_video_images(ModelRc::default());
    state.set_video_images_status("正在安全读取首张图片…".into());
}

pub(super) fn cancel_video_image_work(state: &AppState, epoch: &AtomicU64) {
    let _=advance_video_image_epoch(epoch);
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
            _ => None,
        }
    }
}

fn update_video_image_source(
    state: &AppState,
    quote_epoch: &AtomicU64,
    request_id: &Mutex<String>,
) -> bool {
    // Any collection change invalidates both an in-flight quote and its idempotency binding.
    let advanced=advance_video_image_epoch(quote_epoch).is_some();
    request_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clear();
    state.set_video_quote_loading(false);
    state.set_video_quote_ready(false);
    state.set_video_quote_id("".into());
    state.set_video_credit_cost("".into());
    state.set_video_source_file_id("".into());
    let only = (state.get_video_images().row_count() > 0)
        .then(|| state.get_video_images().row_data(0))
        .flatten();
    state.set_video_source_id(only.as_ref().map(|row|row.source_asset_id.clone()).unwrap_or_default());
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
    if !advanced {
        state.set_video_status("请求计数已耗尽，请重启后继续".into());
        false
    } else if let Some(error)=video_image_generation_error(state) {
        state.set_video_status(error.into());false
    } else { true }
}

fn append_video_images(
    state: &AppState,
    rows: Vec<VideoImageItem>,
    skipped: usize,
    quote_epoch: &AtomicU64,
    request_id: &Mutex<String>,
) -> bool {
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
    let mut changed=false;
    if added > 0 {
        state.set_video_images(ModelRc::new(VecModel::from(images)));
        changed=update_video_image_source(state, quote_epoch, request_id);
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
    changed
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
) -> bool {
    if !video_image_work_is_current(state, epoch, expected) {
        return false;
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
        false
    } else {
        append_video_images(state, rows, skipped, quote_epoch, request_id)
    }
}

pub(super) fn wire_video_image_callbacks(
    app:&AppWindow,store:Rc<RefCell<Store>>,quote_epoch:Arc<AtomicU64>,request_id:Arc<Mutex<String>>,
) -> Arc<AtomicU64> {
    let state=app.global::<AppState>();
    let epoch=Arc::new(AtomicU64::new(0));
    let dialog_binding:Rc<RefCell<Option<PrivatePersistence>>>=Rc::new(RefCell::new(None));
    {
        let (weak,store,binding)=(app.as_weak(),store.clone(),dialog_binding.clone());
        state.on_open_video_asset_picker(move|| {
            let Some(app)=weak.upgrade() else { return; };
            let Some(persistence)=store.borrow().private_persistence.clone() else { return; };
            let state=app.global::<AppState>();
            let pending=apply_video_input(&store,&persistence,|| {
                if state.get_video_generating() || state.get_video_images_loading() { return None; }
                *binding.borrow_mut()=Some(persistence.clone());
                state.set_video_image_dialog("assets".into());
                state.set_video_asset_selected_count(0);
                let (rows,pending)=video_asset_choice_rows(&state,&store.borrow(),&persistence);
                state.set_video_images_status(format!("共 {} 张可用图片",rows.len()).into());
                state.set_video_asset_choices(ModelRc::new(VecModel::from(rows)));
                state.set_video_images_loading(false);
                Some(pending)
            }).flatten();
            if let Some(pending)=pending {
                hydrate_next_video_asset_preview(weak.clone(),store.clone(),persistence,pending);
            }
        });
    }
    {
        let (weak,store,epoch,quote_epoch,request_id)=(app.as_weak(),store.clone(),epoch.clone(),quote_epoch.clone(),request_id.clone());
        state.on_upload_video_images(move|| {
            let Some(app)=weak.upgrade() else { return; };
            // Capture before the native picker can run a nested platform event loop.
            let Some(persistence)=store.borrow().private_persistence.clone() else { return; };
            let Ok((activity,effect))=persistence.begin_effect() else { return; };
            if !captured_video_binding(&store,&persistence) { return; }
            let state=app.global::<AppState>();
            if state.get_video_generating() || state.get_video_images_loading() { return; }
            let files=rfd::FileDialog::new().set_title("选择图片（支持多选）")
                .add_filter("Images",crate::image_formats::picker_image_extensions()).pick_files();
            if activity.is_quiescing() || !persistence.is_current() || !captured_video_binding(&store,&persistence) { return; }
            drop(effect);
            let Some(files)=files.filter(|files|!files.is_empty()) else { return; };
            let candidates=files.into_iter().map(|path| {
                let title=path.file_name().unwrap_or_default().to_string_lossy().into_owned();(title,path)
            }).collect();
            if apply_video_input(&store,&persistence,||state.set_video_image_dialog("".into())).is_none() { return; }
            start_captured_video_image_import(&app,store.clone(),persistence,candidates,false,epoch.clone(),quote_epoch.clone(),request_id.clone());
        });
    }
    {
        let (weak,store,epoch,binding)=(app.as_weak(),store.clone(),epoch.clone(),dialog_binding.clone());
        state.on_close_video_image_dialog(move|| {
            let Some(app)=weak.upgrade() else { return; };
            let captured=binding.borrow().clone().or_else(||store.borrow().private_persistence.clone());
            let Some(persistence)=captured else { return; };
            let _=apply_video_input(&store,&persistence,|| {
                cancel_video_image_work(&app.global::<AppState>(),&epoch);binding.borrow_mut().take();
            });
        });
    }
    {
        let (weak,store,binding)=(app.as_weak(),store.clone(),dialog_binding.clone());
        state.on_toggle_video_asset(move|id| {
            let Some(app)=weak.upgrade() else { return; };
            let Some(persistence)=binding.borrow().clone() else { return; };
            let _=apply_video_input(&store,&persistence,|| {
                let state=app.global::<AppState>();
                if state.get_video_images_loading() || state.get_video_generating() || state.get_video_image_dialog()!="assets" { return; }
                let mut rows:Vec<_>=state.get_video_asset_choices().iter().collect();
                if rows.iter().any(|row|!persistence.owns_path(Path::new(row.source_path.as_str()))) { return; }
                for row in &mut rows { if row.id==id && !row.added { row.selected=!row.selected; } }
                let Ok(count)=i32::try_from(rows.iter().filter(|row|row.selected&&!row.added).count()) else { return; };
                state.set_video_asset_selected_count(count);
                state.set_video_asset_choices(ModelRc::new(VecModel::from(rows)));
            });
        });
    }
    {
        let (weak,store,epoch,quote_epoch,request_id,binding)=(app.as_weak(),store.clone(),epoch.clone(),quote_epoch.clone(),request_id.clone(),dialog_binding.clone());
        state.on_confirm_video_assets(move|| {
            let Some(app)=weak.upgrade() else { return; };
            let Some(persistence)=binding.borrow().clone() else { return; };
            let state=app.global::<AppState>();
            let changed=apply_video_input(&store,&persistence,|| {
                if state.get_video_images_loading() || state.get_video_generating() || state.get_video_image_dialog()!="assets" { return false; }
                let selected:Vec<_>=state.get_video_asset_choices().iter().filter(|row|row.selected&&!row.added).collect();
                if selected.is_empty() || selected.iter().chain(state.get_video_images().iter().collect::<Vec<_>>().iter())
                    .any(|row|!persistence.owns_path(Path::new(row.source_path.as_str()))) { return false; }
                cancel_video_image_work(&state,&epoch);
                binding.borrow_mut().take();
                append_video_images(&state,selected,0,&quote_epoch,&request_id)
            }).unwrap_or(false);
            if changed { quote_current_video_image(&state,&store,&persistence,&quote_epoch); }
        });
    }
    {
        let (weak,store,epoch,quote_epoch,request_id)=(app.as_weak(),store.clone(),epoch.clone(),quote_epoch.clone(),request_id.clone());
        state.on_remove_video_image(move|id| {
            let Some(app)=weak.upgrade() else { return; };
            let Some(persistence)=store.borrow().private_persistence.clone() else { return; };
            let state=app.global::<AppState>();
            let changed=apply_video_input(&store,&persistence,|| {
                if state.get_video_generating() || state.get_video_images_loading() { return false; }
                let old:Vec<_>=state.get_video_images().iter().collect();
                if old.iter().any(|row|!persistence.owns_path(Path::new(row.source_path.as_str()))) { return false; }
                let rows:Vec<_>=old.iter().filter(|row|row.id!=id).cloned().collect();
                if rows.len()==old.len() { return false; }
                let _=advance_video_image_epoch(&epoch);
                state.set_video_images(ModelRc::new(VecModel::from(rows)));
                state.set_video_images_status("已从本次列表移除，原文件仍保留".into());
                update_video_image_source(&state,&quote_epoch,&request_id)
            }).unwrap_or(false);
            if changed { quote_current_video_image(&state,&store,&persistence,&quote_epoch); }
        });
    }
    epoch
}

#[cfg(test)]
#[path = "video_images_tests.rs"]
pub(in crate::runtime) mod tests;
