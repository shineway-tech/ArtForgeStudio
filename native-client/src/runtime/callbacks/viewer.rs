use super::*;
use std::sync::atomic::AtomicBool;

#[derive(Clone)]
struct ViewerActionCapture {
    persistence: PrivatePersistence,
}

#[derive(Clone)]
struct RemoveBlackStagedSave {
    source: Rc<CapturedViewerSource>,
    item_id: String,
}
type RemoveBlackSaveState = Rc<RefCell<Option<RemoveBlackStagedSave>>>;

fn viewer_reference_source_after_target(
    source: &CapturedViewerSource, app: &AppWindow, context: &AppContext,
    persistence: &PrivatePersistence, target_workspace: &str,
) -> bool {
    if source.is_current(app, context, persistence) { return true; }
    let (ViewerSourceTarget::Canvas(original_workspace) | ViewerSourceTarget::CanvasNode(original_workspace)) = &source.target else { return false; };
    let store = context.store.borrow();
    if !store.private_persistence.as_ref().is_some_and(|binding| binding.same_binding_metadata(persistence))
        || normalize_canvas_workspace_id(&store.active_canvas_workspace_id)
            != normalize_canvas_workspace_id(target_workspace)
        || !context.active_namespace.lock().ok()
            .is_some_and(|active| active.as_ref() == Some(persistence.lease())) { return false; }
    let state = app.global::<AppState>();
    state.get_page() == source.page && state.get_viewer_id() == source.id
        && state.get_viewer_source() == source.source
        && Path::new(state.get_viewer_source_path().as_str()) == source.path
        && viewer_source_presentation(&state) == Some(source.presentation)
        && store.canvas_workspaces.get(original_workspace).is_some_and(|workspace|
            saved_canvas_viewer_source_matches(source, workspace))
}

impl ViewerActionCapture {
    fn capture(context: &AppContext) -> Option<Self> {
        let persistence = context.store.borrow().private_persistence.clone()?;
        let activity = persistence.begin_activity().ok()?;
        let current = !activity.is_quiescing()
            && !persistence.upgrade_latch().is_tripped()
            && context.store.borrow().private_persistence.as_ref()
                .is_some_and(|binding| binding.same_binding_metadata(&persistence));
        drop(activity);
        current.then_some(Self { persistence })
    }

    fn apply<R>(&self, context: &AppContext, apply: impl FnOnce() -> R) -> Option<R> {
        context.apply_user_completion(self.persistence.lease(), || {
            let current = {
                let store = context.store.borrow();
                store.private_persistence.as_ref()
                    .is_some_and(|binding| binding.same_binding_metadata(&self.persistence))
            };
            current.then(apply)
        }).ok().flatten()
    }
}

fn start_captured_viewer_projection(
    app: &AppWindow,
    context: &AppContext,
    capture: &ViewerActionCapture,
    id: &str,
    source: &str,
) -> bool {
    let Some(projection) = prepare_viewer_projection(app, &context.store.borrow(), id, source) else {
        return false;
    };
    let mut projection = Some(projection);
    let effects = capture.apply(context, || {
        projection.take().expect("single viewer projection").publish_metadata(app)
    });
    drop(projection);
    if let Some(effects) = effects {
        start_viewer_preview_effects(app, context.clone(), capture.persistence.clone(), effects);
        true
    } else {
        false
    }
}

fn move_captured_viewer(
    app: &AppWindow,
    context: &AppContext,
    capture: &ViewerActionCapture,
    direction: i32,
) {
    let target = capture.apply(context, || {
        let state = app.global::<AppState>();
        let source = state.get_viewer_source().to_string();
        if source == "reference" { return None; }
        let current = state.get_viewer_id().to_string();
        let ids = viewer_ids(app, &context.store.borrow(), &source);
        let index = ids.iter().position(|id| id == &current)?;
        let next = if direction < 0 {
            index.checked_sub(1)
        } else {
            (index + 1 < ids.len()).then_some(index + 1)
        };
        let Some(next) = next else {
            let message = if state.get_language().as_str() == "en" {
                if direction < 0 { "This is the first image." } else { "This is the last image." }
            } else if direction < 0 { "当前已是第一张" } else { "当前已是最后一张" };
            state.set_viewer_message(message.into());
            return None;
        };
        Some((current, source, ids[next].clone()))
    }).flatten();
    let Some((original_id, original_source, target_id)) = target else { return; };
    let Some(projection) = prepare_viewer_projection(app, &context.store.borrow(), &target_id, &original_source) else { return; };
    let mut projection = Some(projection);
    let effects = capture.apply(context, || {
        let state = app.global::<AppState>();
        if state.get_viewer_id() != original_id || state.get_viewer_source() != original_source { return None; }
        Some(projection.take().expect("single moved viewer projection").publish_metadata(app))
    }).flatten();
    drop(projection);
    if let Some(effects) = effects {
        start_viewer_preview_effects(app, context.clone(), capture.persistence.clone(), effects);
    }
}


fn start_captured_regeneration(app: &AppWindow, context: AppContext) {
    let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
    let state = app.global::<AppState>();
    let source_id = state.get_viewer_id().to_string();
    let source_collection = state.get_viewer_source().to_string();
    let page = state.get_page().to_string();
    let item = {
        let store = context.store.borrow();
        viewer_item(&store, &source_id, &source_collection).cloned()
    };
    let Some(item) = item else {
        let _ = capture.apply(&context, || {
            let state = app.global::<AppState>();
            if state.get_page() == page && state.get_viewer_open()
                && state.get_viewer_id() == source_id && state.get_viewer_source() == source_collection {
                state.set_viewer_message("找不到原生成记录，无法再次生成".into());
            }
        });
        return;
    };
    let item_version = item.clone();
    let (billing_scope, authority, activity) = match context.capture_billing_action(KnownCapability::Bill) {
        Ok(value) => value,
        Err(error) => {
            let _ = capture.apply(&context, || {
                if regeneration_source_current(app, &context, &capture.persistence, &page,
                    &source_id, &source_collection, &item_version) {
                    app.global::<AppState>().set_viewer_message(error.user_message().into());
                }
            });
            return;
        }
    };
    drop(activity);
    if authority.lease() != capture.persistence.lease() { return; }
    let persistence = capture.persistence.clone();
    let launched = spawn_delivery_preparation(&persistence, move |captured, activity, cancel| {
        if activity.is_quiescing() || cancel.load(Ordering::SeqCst) {
            return Err(DeliveryRetryError::AuthenticationRequired);
        }
        prepare_asset_regeneration_for_namespace(captured, item).map_err(DeliveryRetryError::from)
    });
    match launched {
        Ok((cancel, receiver)) => poll_captured_regeneration(
            app.as_weak(), context, persistence, page, source_id, source_collection,
            item_version, authority, billing_scope, cancel, receiver,
        ),
        Err(_) => {}
    }
}

fn regeneration_item_matches(current: &AssetData, original: &AssetData) -> bool {
    current.id == original.id && current.conversation_id == original.conversation_id
        && current.category == original.category && current.kind == original.kind
        && current.prompt == original.prompt && current.ratio == original.ratio
        && current.quality == original.quality && current.model == original.model
        && current.origin == original.origin && current.source_path == original.source_path
        && current.reference_paths == original.reference_paths
}

fn regeneration_source_current(
    app: &AppWindow, context: &AppContext, persistence: &PrivatePersistence,
    page: &str, id: &str, source: &str, original: &AssetData,
) -> bool {
    let store = context.store.borrow();
    if !store.private_persistence.as_ref().is_some_and(|binding| binding.same_binding_metadata(persistence)) {
        return false;
    }
    let state = app.global::<AppState>();
    state.get_page() == page && state.get_viewer_open() && state.get_viewer_id() == id
        && state.get_viewer_source() == source
        && state.get_viewer_source_path() == original.source_path
        && viewer_item(&store, id, source).is_some_and(|item| regeneration_item_matches(item, original))
}

fn poll_captured_regeneration(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence,
    page: String, source_id: String, source_collection: String,
    item_version: AssetData,
    authority: Arc<NamespaceStorageAuthority>, billing_scope: BillingScope,
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<PreparedAssetRegeneration, DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => { poll_captured_regeneration(weak, context, persistence, page, source_id,
                source_collection, item_version, authority, billing_scope,
                cancel, receiver); return; }
            Err(_) => return,
            Ok(false) => {}
        }
        let Ok(Ok(prepared)) = receiver.try_recv() else {
            let _ = context.apply_user_completion(persistence.lease(), || {
                if regeneration_source_current(&app, &context, &persistence, &page,
                    &source_id, &source_collection, &item_version) {
                    app.global::<AppState>().set_viewer_message("原图片未能安全读取，请重试".into());
                }
            });
            return;
        };
        let current = context.apply_user_completion(persistence.lease(), || {
            regeneration_source_current(&app, &context, &persistence, &page,
                &source_id, &source_collection, &item_version)
        }).unwrap_or(false);
        if current {
            let _ = start_asset_regeneration_with_prepared_inputs(
                &app, context, authority, &billing_scope, prepared,
            );
        }
    });
}

fn start_captured_image_editor(
    app: &AppWindow, context: AppContext, points: Rc<VecModel<BrushPoint>>,
    last_point: Rc<RefCell<Option<(f32, f32, f32)>>>,
    editor_source: Rc<RefCell<Option<Rc<CapturedViewerSource>>>>,
) {
    let Ok(source) = capture_current_viewer_source(app, &context) else { return; };
    let source = Rc::new(source); let persistence = source.persistence.clone();
    let path = source.path().to_owned();
    let launched = spawn_delivery_preparation(&persistence, move |captured, activity, cancel| {
        if activity.is_quiescing() || cancel.load(Ordering::SeqCst) {
            return Err(DeliveryRetryError::AuthenticationRequired);
        }
        prepare_owned_preview(captured, &path, PreviewPurpose::Viewer).map_err(DeliveryRetryError::from)
    });
    if let Ok((cancel, receiver)) = launched {
        poll_captured_image_editor(app.as_weak(), context, persistence, source, points,
            last_point, editor_source, cancel, receiver);
    }
}

fn poll_captured_image_editor(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, points: Rc<VecModel<BrushPoint>>,
    last_point: Rc<RefCell<Option<(f32, f32, f32)>>>,
    editor_source: Rc<RefCell<Option<Rc<CapturedViewerSource>>>>, cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<PreparedDeliveryPreview, DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => { poll_captured_image_editor(weak, context, persistence, source, points,
                last_point, editor_source, cancel, receiver); return; }
            Err(_) => return,
            Ok(false) => {}
        }
        let Ok(Ok(preview)) = receiver.try_recv() else { return; };
        let dimensions = preview.dimensions();
        let image = materialize_delivery_preview(&preview);
        let _ = context.apply_user_completion(persistence.lease(), || {
            if !source.is_current(&app, &context, &persistence) { return; }
            points.clear(); *last_point.borrow_mut() = None;
            let state = app.global::<AppState>();
            state.set_image_editor_image(image); state.set_image_editor_source_path(source.path().to_string_lossy().into_owned().into());
            state.set_image_editor_source_width(dimensions.0 as i32); state.set_image_editor_source_height(dimensions.1 as i32);
            state.set_image_editor_brush_size(28.0); state.set_image_editor_brush_shape("circle".into());
            state.set_image_editor_brush_color(slint::Color::from_rgb_u8(255, 77, 79));
            state.set_image_editor_prompt("".into()); state.set_image_editor_status("".into());
            state.set_image_editor_generating(false); configure_image_editor_model(&state);
            state.set_image_editor_return_page(state.get_page()); state.set_viewer_open(false);
            state.set_page("image-editor".into());
            *editor_source.borrow_mut() = Some(source.clone());
        });
    });
}

struct CapturedImageEditorSubmission {
    persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>,
    flight_id: Uuid,
    source_path: PathBuf,
    prompt: String,
    model_code: String,
    quality: String,
    request: CapturedImageEditRequest,
    points: Rc<VecModel<BrushPoint>>,
    brush_version: Vec<(u32, u32, u32, String)>,
}

fn image_editor_brush_version(points: &VecModel<BrushPoint>) -> Vec<(u32, u32, u32, String)> {
    points.iter().map(|point| (
        point.x.to_bits(), point.y.to_bits(), point.size.to_bits(), point.shape.to_string(),
    )).collect()
}

fn clear_owned_editor_flight(flight: &Rc<RefCell<Option<Uuid>>>, id: Uuid) {
    if flight.borrow().as_ref() == Some(&id) { flight.borrow_mut().take(); }
}

impl CapturedImageEditorSubmission {
    fn owns_editor_surface(
        &self, app: &AppWindow, context: &AppContext,
        flight: &Rc<RefCell<Option<Uuid>>>,
    ) -> bool {
        let binding_current = {
            let store = context.store.borrow();
            store.private_persistence.as_ref()
                .is_some_and(|binding| binding.same_binding_metadata(&self.persistence))
        };
        binding_current
            && flight.borrow().as_ref() == Some(&self.flight_id)
            && captured_editor_source_current(&self.source, app, context)
            && app.global::<AppState>().get_page() == "image-editor"
            && Path::new(app.global::<AppState>().get_image_editor_source_path().as_str()) == self.source_path
    }

    fn is_current(
        &self, app: &AppWindow, context: &AppContext,
        flight: &Rc<RefCell<Option<Uuid>>>,
    ) -> bool {
        self.owns_editor_surface(app, context, flight)
            && app.global::<AppState>().get_image_editor_prompt().trim() == self.prompt
            && app.global::<AppState>().get_image_editor_model() == self.model_code
            && app.global::<AppState>().get_image_editor_quality() == self.quality
            && image_editor_brush_version(&self.points) == self.brush_version
    }
}

fn saved_canvas_viewer_source_matches(source: &CapturedViewerSource, workspace: &CanvasWorkspaceData) -> bool {
    match &source.target {
        ViewerSourceTarget::CanvasNode(_) => workspace.notes.iter().any(|row| row.id == source.id && Path::new(&row.image_path) == source.path),
        ViewerSourceTarget::Canvas(_) => workspace.references.iter().any(|row| row.id == source.id && Path::new(&row.source_path) == source.path),
        _ => false,
    }
}

fn captured_editor_source_current(source: &CapturedViewerSource, app: &AppWindow, context: &AppContext) -> bool {
    let store = context.store.borrow();
    if !store.private_persistence.as_ref()
        .is_some_and(|binding| binding.same_binding_metadata(&source.persistence)) { return false; }
    match &source.target {
        ViewerSourceTarget::Asset(collection) => viewer_item(&store, &source.id, collection)
            .is_some_and(|item| Path::new(&item.source_path) == source.path),
        ViewerSourceTarget::Category(category) => references_for_category(&store.references, category)
            .iter().any(|item| item.id == source.id && Path::new(&item.source_path) == source.path),
        ViewerSourceTarget::Canvas(workspace) => {
            let rows = if normalize_canvas_workspace_id(&store.active_canvas_workspace_id)
                == normalize_canvas_workspace_id(workspace) {
                &store.canvas_references
            } else {
                let Some(value) = store.canvas_workspaces.get(workspace) else { return false; };
                &value.references
            };
            rows.iter().any(|item| item.id == source.id && Path::new(&item.source_path) == source.path)
        }
        ViewerSourceTarget::CanvasNode(workspace) => {
            if store.active_canvas_workspace_id == *workspace {
                store.canvas_notes.iter().any(|row| row.id == source.id && Path::new(&row.image_path) == source.path)
            } else {
                store.canvas_workspaces.get(workspace).is_some_and(|saved| saved_canvas_viewer_source_matches(source, saved))
            }
        }
        ViewerSourceTarget::Custom { session, original, return_page } => {
            let state = app.global::<AppState>();
            state.get_custom_prompt_editor_session_id() == *session
                && state.get_custom_prompt_editing_original() == *original
                && state.get_custom_prompt_editor_return_page() == *return_page
                && state.get_custom_prompt_reference_items().iter().any(|item|
                    item.id == source.id && Path::new(item.source_path.as_str()) == source.path)
        }
    }
}

fn start_captured_image_edit(
    app: &AppWindow, context: AppContext, points: Rc<VecModel<BrushPoint>>,
    editor_source: Rc<RefCell<Option<Rc<CapturedViewerSource>>>>,
    flight: Rc<RefCell<Option<Uuid>>>,
) {
    let Some(action) = ViewerActionCapture::capture(&context) else { return; };
    let Some(source) = editor_source.borrow().clone() else { return; };
    let values = action.apply(&context, || {
        let state = app.global::<AppState>();
        if state.get_image_editor_generating() || flight.borrow().is_some() { return None; }
        if points.row_count() == 0 {
            state.set_image_editor_status("请先用笔刷标记需要修改的区域".into()); return None;
        }
        let prompt = state.get_image_editor_prompt().trim().to_string();
        if prompt.is_empty() { state.set_image_editor_status("请填写希望如何修改涂抹区域".into()); return None; }
        let model_code = state.get_image_editor_model().to_string();
        if model_code.trim().is_empty() { state.set_image_editor_status("服务端没有可用的图片编辑模型".into()); return None; }
        if !captured_editor_source_current(&source, app, &context) { return None; }
        Some((prompt, model_code, state.get_image_editor_quality().to_string(),
            PathBuf::from(state.get_image_editor_source_path().to_string())))
    }).flatten();
    let Some((prompt, model_code, quality, source_path)) = values else { return; };
    let persistence = action.persistence.clone();
    let state = app.global::<AppState>();
    let original = {
        let store = context.store.borrow();
        match &source.target {
            ViewerSourceTarget::Asset(collection) => viewer_item(&store, source.source_id(), collection).cloned(),
            _ => None,
        }
    };
    let category = original.as_ref().map(|item| item.category.clone())
        .unwrap_or_else(|| resolve_category(state.get_asset_type().as_str(), &prompt));
    let mode = original.as_ref().map(|item| item.kind.clone()).unwrap_or_else(|| state.get_mode().to_string());
    let conversation_id = original.as_ref().map(|item| item.conversation_id.clone())
        .unwrap_or_else(|| state.get_current_conversation_id().to_string());
    let request = CapturedImageEditRequest {
        prompt: prompt.clone(), model_code: model_code.clone(), quality: quality.clone(),
        estimated_credit_cost: state.get_image_editor_estimated_credit_cost(),
        category, mode, conversation_id,
    };
    let brush = points.iter().map(|point| CapturedImageEditBrushPoint {
        x: point.x, y: point.y, size: point.size, shape: point.shape.to_string(),
    }).collect::<Vec<_>>();
    let brush_version = image_editor_brush_version(&points);
    let (billing_scope, authority, activity) = match context.capture_billing_action(KnownCapability::Bill) {
        Ok(value) => value,
        Err(error) => {
            let _ = action.apply(&context, || {
                if captured_editor_source_current(&source, app, &context) {
                    app.global::<AppState>().set_image_editor_status(error.user_message().into());
                }
            });
            return;
        }
    };
    drop(activity);
    if authority.lease() != persistence.lease() { return; }
    let flight_id = Uuid::new_v4();
    let captured = CapturedImageEditorSubmission {
        persistence: persistence.clone(), source: source.clone(), flight_id,
        source_path: source_path.clone(), prompt, model_code, quality, request,
        points: points.clone(), brush_version,
    };
    let admitted = context.apply_user_completion(persistence.lease(), || {
        *flight.borrow_mut() = Some(flight_id);
        if !captured.is_current(app, &context, &flight) { flight.borrow_mut().take(); return false; }
        let state = app.global::<AppState>(); state.set_image_editor_generating(true);
        state.set_image_editor_status("正在准备图片编辑任务...".into()); true
    }).unwrap_or(false);
    if !admitted { return; }
    let launched = spawn_delivery_preparation(&persistence, move |persistence, activity, cancel| {
        if activity.is_quiescing() || cancel.load(Ordering::SeqCst) {
            return Err(DeliveryRetryError::AuthenticationRequired);
        }
        prepare_image_edit_inputs_for_namespace(persistence, &source_path, brush)
            .map_err(DeliveryRetryError::from)
    });
    match launched {
        Ok((cancel, receiver)) => poll_captured_image_edit(app.as_weak(), context, captured,
            authority, billing_scope, original, flight, cancel, receiver),
        Err(_) => {
            let _ = action.apply(&context, || {
                if !captured.owns_editor_surface(app, &context, &flight) { return; }
                clear_owned_editor_flight(&flight, flight_id);
                let state = app.global::<AppState>(); state.set_image_editor_generating(false);
                state.set_image_editor_status("图片编辑输入准备未能启动，请重试".into());
            });
            clear_owned_editor_flight(&flight, flight_id);
        }
    }
}

fn poll_captured_image_edit(
    weak: Weak<AppWindow>, context: AppContext, captured: CapturedImageEditorSubmission,
    authority: Arc<NamespaceStorageAuthority>, billing_scope: BillingScope, original: Option<AssetData>,
    flight: Rc<RefCell<Option<Uuid>>>,
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<PreparedImageEditInputs, DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => { poll_captured_image_edit(weak, context, captured, authority, billing_scope,
                original, flight, cancel, receiver); return; }
            Err(_) => {
                let _ = context.apply_user_completion(captured.persistence.lease(), || {
                    if captured.owns_editor_surface(&app, &context, &flight) {
                        clear_owned_editor_flight(&flight, captured.flight_id);
                        let state = app.global::<AppState>(); state.set_image_editor_generating(false);
                        state.set_image_editor_status("图片编辑输入准备未能安全完成，请重试".into());
                    }
                });
                clear_owned_editor_flight(&flight, captured.flight_id);
                return;
            }
            Ok(false) => {}
        }
        let Ok(Ok(prepared)) = receiver.try_recv() else {
            let _ = context.apply_user_completion(captured.persistence.lease(), || {
                if captured.owns_editor_surface(&app, &context, &flight) {
                    clear_owned_editor_flight(&flight, captured.flight_id);
                    let state = app.global::<AppState>(); state.set_image_editor_generating(false);
                    state.set_image_editor_status("图片编辑输入准备失败，请重试".into());
                }
            });
            clear_owned_editor_flight(&flight, captured.flight_id);
            return;
        };
        let current = context.apply_user_completion(captured.persistence.lease(), || {
            let current = captured.is_current(&app, &context, &flight);
            if !current && captured.owns_editor_surface(&app, &context, &flight) {
                let state = app.global::<AppState>(); state.set_image_editor_generating(false);
                state.set_image_editor_status("编辑内容已变化，请重新提交".into());
            }
            current
        })
            .unwrap_or(false);
        clear_owned_editor_flight(&flight, captured.flight_id);
        if current {
            start_backend_image_edit_with_prepared_inputs(
                &app, context, authority, &billing_scope, original, prepared, captured.request,
            );
        }
    });
}

#[derive(Clone)]
enum CapturedViewerReferenceIntent {
    Reference { category: String },
    Same { category: String, prompt: String, conversation_id: String },
    Creation {
        workflow_id: String, title: String, template: String, hint: String,
        original_prompt: String,
    },
}

#[derive(Clone)]
enum ViewerReferenceFlight {
    Preparing { ticket: Uuid, source: Rc<CapturedViewerSource>,
        intent: CapturedViewerReferenceIntent },
    Staged { ticket: Uuid, source: Rc<CapturedViewerSource>,
        intent: CapturedViewerReferenceIntent, reference_id: String },
}
type ViewerReferenceSaveState = Rc<RefCell<Option<ViewerReferenceFlight>>>;

fn reference_intent_same(a: &CapturedViewerReferenceIntent, b: &CapturedViewerReferenceIntent) -> bool {
    match (a, b) {
        (CapturedViewerReferenceIntent::Reference { category: a }, CapturedViewerReferenceIntent::Reference { category: b }) => a == b,
        (CapturedViewerReferenceIntent::Same { category: ac, prompt: ap, .. },
            CapturedViewerReferenceIntent::Same { category: bc, prompt: bp, .. }) => ac == bc && ap == bp,
        (CapturedViewerReferenceIntent::Creation { workflow_id: aw, title: at, template: ax, hint: ah, .. },
            CapturedViewerReferenceIntent::Creation { workflow_id: bw, title: bt, template: bx, hint: bh, .. }) =>
                aw == bw && at == bt && ax == bx && ah == bh,
        _ => false,
    }
}

fn reference_flight_matches(
    flight: &ViewerReferenceFlight, source: &CapturedViewerSource,
    intent: &CapturedViewerReferenceIntent,
) -> bool {
    let (saved_source, saved_intent) = match flight {
        ViewerReferenceFlight::Preparing { source, intent, .. }
        | ViewerReferenceFlight::Staged { source, intent, .. } => (source, intent),
    };
    saved_source.persistence.same_binding_metadata(&source.persistence)
        && saved_source.source_id() == source.source_id()
        && saved_source.path() == source.path() && reference_intent_same(saved_intent, intent)
}

fn reference_flight_ticket(state: &ViewerReferenceSaveState, ticket: Uuid) -> bool {
    state.borrow().as_ref().is_some_and(|flight| match flight {
        ViewerReferenceFlight::Preparing { ticket: current, .. }
        | ViewerReferenceFlight::Staged { ticket: current, .. } => *current == ticket,
    })
}

fn clear_reference_preparing(state: &ViewerReferenceSaveState, ticket: Uuid) {
    let owns = state.borrow().as_ref().is_some_and(|flight| matches!(flight,
        ViewerReferenceFlight::Preparing { ticket: current, .. } if *current == ticket));
    if owns { state.borrow_mut().take(); }
}

fn reference_staged_ticket(
    state: &ViewerReferenceSaveState, ticket: Uuid, reference_id: &str,
) -> bool {
    state.borrow().as_ref().is_some_and(|flight| matches!(flight,
        ViewerReferenceFlight::Staged { ticket: current, reference_id: current_id, .. }
            if *current == ticket && current_id == reference_id))
}

fn start_captured_viewer_reference(
    app: &AppWindow, context: AppContext, intent: CapturedViewerReferenceIntent,
    save_state: ViewerReferenceSaveState,
) {
    let existing = save_state.borrow().clone();
    if let Some(ViewerReferenceFlight::Staged {
        ticket, source, intent: saved_intent, reference_id,
    }) = existing.as_ref() {
        let current = match saved_intent {
            CapturedViewerReferenceIntent::Creation { workflow_id, .. } =>
                viewer_reference_source_after_target(source, app, &context, &source.persistence, workflow_id),
            _ => source.is_current(app, &context, &source.persistence),
        };
        if current && reference_intent_same(saved_intent, &intent) {
            retry_captured_reference_save(app, context, source.persistence.clone(), source.clone(),
                saved_intent.clone(), save_state, *ticket, reference_id.clone());
            return;
        }
    }
    let Ok(source) = capture_current_viewer_source(app, &context) else {
        if existing.is_some() { save_state.borrow_mut().take(); }
        return;
    };
    let source = Rc::new(source); let persistence = source.persistence.clone();
    if existing.as_ref().is_some_and(|flight| !reference_flight_matches(flight, &source, &intent)) {
        save_state.borrow_mut().take();
    }
    if save_state.borrow().is_some() {
        let _ = context.apply_user_completion(persistence.lease(), || {
            if source.is_current(app, &context, &persistence) {
                app.global::<AppState>().set_viewer_message("另一参考图操作仍待保存，请先重试原操作".into());
            }
        });
        return;
    }
    let allowed = {
        let store = context.store.borrow();
        match &intent {
            CapturedViewerReferenceIntent::Reference { category }
            | CapturedViewerReferenceIntent::Same { category, .. } =>
                references_for_category(&store.references, category).len()
                    < max_reference_images_for_category(category),
            CapturedViewerReferenceIntent::Creation { workflow_id, .. } => {
                let target = normalize_canvas_workspace_id(workflow_id);
                if normalize_canvas_workspace_id(&store.active_canvas_workspace_id) == target {
                    store.canvas_references.len() < MAX_REFERENCE_IMAGES
                } else {
                    store.canvas_workspaces.get(&target)
                        .map(|workspace| workspace.references.len() < MAX_REFERENCE_IMAGES)
                        .unwrap_or(true)
                }
            }
        }
    };
    if !allowed {
        let limit = match &intent {
            CapturedViewerReferenceIntent::Reference { category }
            | CapturedViewerReferenceIntent::Same { category, .. } =>
                max_reference_images_for_category(category),
            CapturedViewerReferenceIntent::Creation { .. } => MAX_REFERENCE_IMAGES,
        };
        let _ = context.apply_user_completion(persistence.lease(), || {
            if source.is_current(app, &context, &persistence) {
                app.global::<AppState>()
                    .set_viewer_message(reference_limit_message(limit).into());
            }
        });
        return;
    }
    let ticket = Uuid::new_v4();
    let admitted = context.apply_user_completion(persistence.lease(), || {
        if !source.is_current(app, &context, &persistence) { return false; }
        *save_state.borrow_mut() = Some(ViewerReferenceFlight::Preparing {
            ticket, source: source.clone(), intent: intent.clone(),
        });
        true
    }).unwrap_or(false);
    if !admitted { return; }
    let path = source.path().to_owned();
    let launched = spawn_delivery_preparation(&persistence, move |captured, activity, cancel| {
        if activity.is_quiescing() || cancel.load(Ordering::SeqCst) {
            return Err(DeliveryRetryError::AuthenticationRequired);
        }
        let authority = captured.storage_authority()?;
        let bytes = authority.read_image_source(&path, 32 * 1024 * 1024)?;
        let image = decode_reference_bytes(&bytes)?;
        persist_reference_image_for_namespace(&authority, &image).map_err(DeliveryRetryError::from)
    });
    if let Ok((cancel, receiver)) = launched {
        poll_captured_viewer_reference(app.as_weak(), context, persistence, source, intent,
            save_state, ticket, cancel, receiver);
    } else {
        save_state.borrow_mut().take();
    }
}

fn poll_captured_viewer_reference(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, intent: CapturedViewerReferenceIntent,
    save_state: ViewerReferenceSaveState, ticket: Uuid,
    cancel: Arc<AtomicBool>, receiver: mpsc::Receiver<std::result::Result<PathBuf, DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => { poll_captured_viewer_reference(weak, context, persistence, source, intent,
                save_state, ticket, cancel, receiver); return; }
            Err(_) => {
                clear_reference_preparing(&save_state, ticket);
                let _ = context.apply_user_completion(persistence.lease(), || {
                    if source.is_current(&app, &context, &persistence) {
                        app.global::<AppState>().set_viewer_message("参考图准备未能安全完成，请重试".into());
                    }
                });
                return;
            }
            Ok(false) => {}
        }
        if !reference_flight_ticket(&save_state, ticket) { return; }
        let Ok(Ok(path)) = receiver.try_recv() else {
            clear_reference_preparing(&save_state, ticket);
            let _ = context.apply_user_completion(persistence.lease(), || {
                if source.is_current(&app, &context, &persistence) {
                    app.global::<AppState>().set_viewer_message("参考图未能安全保存，请重试".into());
                }
            });
            return;
        };
        commit_captured_viewer_reference(&app, context, persistence, source, intent,
            save_state, ticket, path);
    });
}

fn commit_captured_viewer_reference(
    app: &AppWindow, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, intent: CapturedViewerReferenceIntent,
    save_state: ViewerReferenceSaveState, ticket: Uuid, path: PathBuf,
) {
    let Ok(write) = persistence.prepare_ordered_save() else {
        clear_reference_preparing(&save_state, ticket);
        return;
    };
    let mut write = Some(write); let reference_id = Uuid::new_v4().to_string();
    let mut intent = Some(intent); let mut committed_intent = None;
    let queued = context.apply_user_completion(persistence.lease(), || {
        if !source.is_current(app, &context, &persistence) { return None; }
        let intent_value = intent.take().expect("single viewer reference intent");
        let reference = ReferenceData { id: reference_id.clone(), source_path: path.to_string_lossy().into_owned() };
        let mut store = context.store.borrow_mut();
        match &intent_value {
            CapturedViewerReferenceIntent::Reference { category } => {
                let limit = max_reference_images_for_category(category);
                let rows = references_for_category_mut(&mut store.references, category);
                if rows.len() >= limit { return None; }
                rows.push(reference);
            }
            CapturedViewerReferenceIntent::Same { category, prompt, conversation_id } => {
                let limit = max_reference_images_for_category(category);
                let rows = references_for_category_mut(&mut store.references, category);
                if rows.len() >= limit { return None; }
                rows.push(reference);
                let state = app.global::<AppState>();
                let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
                conversations.insert(0, ConversationItem { id: conversation_id.clone().into(),
                    title: short_text(prompt, 10).into(), image: Image::default(), loading: false });
                state.set_conversations(ModelRc::new(VecModel::from(conversations)));
                state.set_current_conversation_id(conversation_id.clone().into());
                state.set_prompt(prompt.clone().into());
            }
            CapturedViewerReferenceIntent::Creation { workflow_id, original_prompt, .. } => {
                let target = normalize_canvas_workspace_id(workflow_id);
                let count = if normalize_canvas_workspace_id(&store.active_canvas_workspace_id) == target {
                    store.canvas_references.len()
                } else {
                    store.canvas_workspaces.get(&target)
                        .map(|workspace| workspace.references.len()).unwrap_or(0)
                };
                if count >= MAX_REFERENCE_IMAGES { return None; }
                let prompt = switch_canvas_workspace(&mut store, original_prompt, &target);
                store.canvas_references.push(reference);
                app.global::<AppState>().set_canvas_workflow_prompt(prompt.into());
            }
        }
        *save_state.borrow_mut() = Some(ViewerReferenceFlight::Staged {
            ticket, source: source.clone(), intent: intent_value.clone(),
            reference_id: reference_id.clone(),
        });
        committed_intent = Some(intent_value);
        Some(write.take().expect("single viewer reference write").enqueue(local_store_data(app, &store)))
    }).ok().flatten();
    drop(write); drop(intent);
    let Some(intent) = committed_intent else {
        clear_reference_preparing(&save_state, ticket);
        return;
    };
    let Some(Ok(receiver)) = queued else {
        let _ = context.apply_user_completion(persistence.lease(), || {
            if source.is_current(app, &context, &persistence) {
                app.global::<AppState>().set_viewer_message("参考图已暂存但本地保存未确认，请重试".into());
            }
        });
        return;
    };
    let launched = spawn_delivery_preparation(&persistence, move |_, _, _| {
        receiver.recv().map_err(|_| anyhow!("viewer reference Store acknowledgment disconnected"))?
            .map_err(anyhow::Error::from)?;
        Ok(())
    });
    if let Ok((cancel, receiver)) = launched {
        poll_captured_reference_store_ack(
            app.as_weak(), context, persistence, source, intent, save_state, ticket,
            reference_id, cancel, receiver,
        );
    }
}

fn poll_captured_reference_store_ack(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, intent: CapturedViewerReferenceIntent,
    save_state: ViewerReferenceSaveState, ticket: Uuid, reference_id: String,
    cancel: Arc<AtomicBool>, receiver: mpsc::Receiver<std::result::Result<(), DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => { poll_captured_reference_store_ack(weak, context, persistence, source, intent,
                save_state, ticket, reference_id, cancel, receiver); return; }
            Err(_) => {
                let _ = context.apply_user_completion(persistence.lease(), || {
                    if reference_staged_ticket(&save_state, ticket, &reference_id)
                        && source.is_current(&app, &context, &persistence) {
                        app.global::<AppState>()
                            .set_viewer_message("参考图保存结果未确认；请重试原操作".into());
                    }
                });
                return;
            }
            Ok(false) => {}
        }
        if !matches!(receiver.try_recv(), Ok(Ok(()))) {
            let _ = context.apply_user_completion(persistence.lease(), || {
                let current = match &intent {
                    CapturedViewerReferenceIntent::Creation { workflow_id, .. } =>
                        viewer_reference_source_after_target(&source, &app, &context, &persistence, workflow_id),
                    _ => source.is_current(&app, &context, &persistence),
                };
                if reference_staged_ticket(&save_state, ticket, &reference_id) && current {
                    app.global::<AppState>()
                    .set_viewer_message("参考图已暂存，本地保存未确认；请重试原操作".into()); }
            });
            return;
        }
        if !reference_staged_ticket(&save_state, ticket, &reference_id) { return; }
        save_state.borrow_mut().take();
        match &intent {
            CapturedViewerReferenceIntent::Reference { category }
            | CapturedViewerReferenceIntent::Same { category, .. } => {
                let projection = prepare_category_reference_projection(&app, &context.store.borrow(), &category);
                let mut projection = Some(projection);

                let effects = context.apply_user_completion(persistence.lease(), || {
                    if !source.is_current(&app, &context, &persistence)
                        || !references_for_category(&context.store.borrow().references, &category)
                            .iter().any(|row| row.id == reference_id) { return None; }
                    let state = app.global::<AppState>();
                    state.set_viewer_open(false); state.set_viewer_image(Image::default());
                    state.set_viewer_source_path("".into()); state.set_page("generation".into());
                    Some(projection.take().expect("single category reference projection").publish_metadata(&app))
                }).ok().flatten();
                drop(projection);
                if let Some(effects) = effects {
                    start_canvas_reference_preview_effects(&app, persistence, effects);
                }
            }
            CapturedViewerReferenceIntent::Creation { workflow_id, title, template, hint, .. } => {
                let store = context.store.borrow();
                let canvas = prepare_canvas_projection(&app, &store);
                let references = prepare_canvas_reference_projection(&app, &store);
                drop(store);
                let mut projections = Some((canvas, references));
                let effects = context.apply_user_completion(persistence.lease(), || {
                    if !viewer_reference_source_after_target(
                        &source, &app, &context, &persistence, &workflow_id,
                    )
                        || !context.store.borrow().canvas_references.iter().any(|row| row.id == reference_id) { return None; }
                    let (canvas, references) = projections.take().expect("single character projections");
                    let state = app.global::<AppState>();
                    state.set_asset_type(if matches!(workflow_id.as_str(), "plant-growth" | "monster-generator" | "upgrade-evolution" | "building-derivation") { "scene" } else { "character" }.into());
                    state.set_canvas_tool("select".into()); state.set_canvas_grid_style("dot".into()); state.set_canvas_dark_background(true);
                    state.set_canvas_workflow_id(workflow_id.clone().into()); state.set_canvas_workflow_title(title.clone().into());
                    state.set_canvas_workflow_template(template.clone().into()); state.set_canvas_workflow_hint(hint.clone().into());
                    state.set_viewer_message("".into()); state.set_viewer_open(false); state.set_viewer_image(Image::default());
                    state.set_viewer_category("".into()); state.set_viewer_source_path("".into());
                    *context.canvas_history.borrow_mut() = CanvasController::default();
                    state.set_canvas_can_undo(false); state.set_canvas_can_redo(false);
                    state.set_canvas_workspace_switch_request(state.get_canvas_workspace_switch_request().saturating_add(1));
                    state.set_page("canvas".into());
                    Some((canvas.publish_metadata(&app), references.publish_metadata(&app)))
                }).ok().flatten();
                drop(projections);
                if let Some((canvas, references)) = effects {
                    start_canvas_preview_effects(&app, persistence.clone(), canvas);
                    start_canvas_reference_preview_effects(&app, persistence, references);
                }
            }
        }
    });
}

fn retry_captured_reference_save(
    app: &AppWindow, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, intent: CapturedViewerReferenceIntent,
    save_state: ViewerReferenceSaveState, ticket: Uuid, reference_id: String,
) {
    let Ok(write) = persistence.prepare_ordered_save() else { return; };
    let mut write = Some(write);
    let queued = context.apply_user_completion(persistence.lease(), || {
        let source_current = match &intent {
            CapturedViewerReferenceIntent::Creation { workflow_id, .. } =>
                viewer_reference_source_after_target(&source, app, &context, &persistence, workflow_id),
            _ => source.is_current(app, &context, &persistence),
        };
        if !source_current { return None; }
        let store = context.store.borrow();
        let present = match &intent {
            CapturedViewerReferenceIntent::Reference { category }
            | CapturedViewerReferenceIntent::Same { category, .. } =>
                references_for_category(&store.references, category).iter().any(|row| row.id == reference_id),
            CapturedViewerReferenceIntent::Creation { .. } =>
                store.canvas_references.iter().any(|row| row.id == reference_id),
        };
        present.then(|| write.take().expect("single reference retry write")
            .enqueue(local_store_data(app, &store)))
    }).ok().flatten();
    drop(write);
    let Some(Ok(receiver)) = queued else { return; };
    if let Ok((cancel, receiver)) = spawn_delivery_preparation(&persistence, move |_, _, _| {
        receiver.recv().map_err(|_| anyhow!("viewer reference retry acknowledgment disconnected"))?
            .map_err(anyhow::Error::from)?;
        Ok(())
    }) {
        poll_captured_reference_store_ack(app.as_weak(), context, persistence, source,
            intent, save_state, ticket, reference_id, cancel, receiver);
    }
}
fn start_captured_remove_black(app: &AppWindow, context: AppContext, save_state: RemoveBlackSaveState) {
    let Ok(source) = capture_current_viewer_source(app, &context) else { return; };
    let source = Rc::new(source);
    let original = {
        let store = context.store.borrow();
        viewer_item(&store, source.source_id(), &source.source).cloned()
    };
    let Some(original) = original else { return; };
    let persistence = source.persistence.clone();
    let existing = save_state.borrow().clone();
    let retry = existing.clone().filter(|saved| {
        saved.source.persistence.same_binding_metadata(&persistence)
            && saved.source.source_id() == source.source_id()
            && saved.source.path() == source.path()
            && context.store.borrow().assets.iter().any(|item| item.id == saved.item_id)
    });
    if let Some(saved) = retry {
        retry_captured_remove_black_save(app, context, persistence, source, save_state, saved.item_id);
        return;
    }
    if existing.as_ref().is_some_and(|saved|
        !saved.source.persistence.same_binding_metadata(&persistence)) {
        save_state.borrow_mut().take();
    }
    if save_state.borrow().is_some() {
        let _ = context.apply_user_completion(persistence.lease(), || {
            if source.is_current(app, &context, &persistence) {
                app.global::<AppState>().set_viewer_message("另一处理结果仍待保存，请先重试原操作".into());
            }
        });
        return;
    }
    let admitted = context.apply_user_completion(persistence.lease(), || {
        if !source.is_current(app, &context, &persistence) { return false; }
        let state = app.global::<AppState>();
        if state.get_viewer_processing() { return false; }
        if state.get_viewer_remove_black_done() {
            state.set_viewer_message(processing_done_message(app, ProcessImageMode::RemoveBlack).into());
            return false;
        }
        state.set_viewer_processing(true); state.set_viewer_processing_progress(1);
        state.set_viewer_processing_label(processing_label(app, ProcessImageMode::RemoveBlack).into());
        state.set_viewer_message("".into());
        true
    }).unwrap_or(false);
    if !admitted { return; }
    let launched = spawn_delivery_preparation(&persistence, move |captured, activity, cancel| {
        if activity.is_quiescing() || cancel.load(Ordering::SeqCst) {
            return Err(DeliveryRetryError::AuthenticationRequired);
        }
        prepare_remove_black_image(captured, &original, cancel).map_err(DeliveryRetryError::from)
    });
    match launched {
        Ok((cancel, receiver)) => poll_captured_remove_black(
            app.as_weak(), context, persistence, source, save_state, cancel, receiver,
        ),
        Err(_) => {
            let _ = context.apply_user_completion(persistence.lease(), || {
                if source.is_current(app, &context, &persistence) {
                    let state = app.global::<AppState>(); state.set_viewer_processing(false);
                    state.set_viewer_message("图片处理未能启动，请重试".into());
                }
            });
        }
    }
}

fn poll_captured_remove_black(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, save_state: RemoveBlackSaveState, cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<PreparedRemoveBlackImage, DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => {
                poll_captured_remove_black(weak, context, persistence, source, save_state, cancel, receiver);
                return;
            }
            Err(_) => {
                let _ = context.apply_user_completion(persistence.lease(), || {
                    if source.is_current(&app, &context, &persistence) {
                        let state = app.global::<AppState>(); state.set_viewer_processing(false);
                        state.set_viewer_message("图片处理未能安全完成，请重试".into());
                    }
                });
                return;
            }
            Ok(false) => {}
        }
        let prepared = match receiver.try_recv() {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                let _ = context.apply_user_completion(persistence.lease(), || {
                    if source.is_current(&app, &context, &persistence) {
                        let state = app.global::<AppState>(); state.set_viewer_processing(false);
                        state.set_viewer_message("图片处理失败，请重试".into());
                    }
                });
                return;
            }
            Err(TryRecvError::Empty) => {
                poll_captured_remove_black(weak, context, persistence, source, save_state, cancel, receiver);
                return;
            }
        };
        commit_captured_remove_black(&app, context, persistence, source, save_state, prepared);
    });
}

fn commit_captured_remove_black(
    app: &AppWindow, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, save_state: RemoveBlackSaveState, prepared: PreparedRemoveBlackImage,
) {
    if prepared.lease() != persistence.lease() { return; }
    let item_id = prepared.item.id.clone();
    let Ok(write) = persistence.prepare_ordered_save() else {
        let _ = context.apply_user_completion(persistence.lease(), || {
            if source.is_current(app, &context, &persistence) {
                let state = app.global::<AppState>(); state.set_viewer_processing(false);
                state.set_viewer_message("本地保存未能开始，请重试".into());
            }
        });
        return;
    };
    let mut write = Some(write); let mut item = Some(prepared.item);
    let queued = context.apply_user_completion(persistence.lease(), || {
        if !source.is_current(app, &context, &persistence) { return None; }
        let item = item.take().expect("single remove-black item");
        let mut store = context.store.borrow_mut();
        store.assets.insert(0, item.clone()); store.generations.insert(0, item);
        *save_state.borrow_mut() = Some(RemoveBlackStagedSave {
            source: source.clone(), item_id: item_id.clone(),
        });
        Some(write.take().expect("single remove-black ordered save").enqueue(local_store_data(app, &store)))
    });
    drop(write); drop(item);
    let receiver = match queued.ok().flatten() {
        Some(Ok(receiver)) => receiver,
        Some(Err(error)) => {
            drop(error);
            let _ = context.apply_user_completion(persistence.lease(), || {
                if source.is_current(app, &context, &persistence) {
                    let state = app.global::<AppState>(); state.set_viewer_processing(false);
                    state.set_viewer_message("本地保存未确认，请重试".into());
                }
            });
            return;
        }
        None => return,
    };
    let launched = spawn_delivery_preparation(&persistence, move |_, activity, cancel| {
        // A retirement controls later UI delivery; it must not reinterpret a
        // real committed Store receipt as a failed save.
        let _ = (activity, cancel);
        receiver.recv().map_err(|_| anyhow!("viewer Store acknowledgement disconnected"))?
            .map_err(anyhow::Error::from)?;
        Ok(())
    });
    match launched {
        Ok((cancel, receiver)) => poll_remove_black_store_ack(
            app.as_weak(), context, persistence, source, save_state, item_id, cancel, receiver,
        ),
        Err(_) => {
            // The command already owns the snapshot and can still commit.
            // Retain the staged row rather than manufacturing a rollback.
            let _ = context.apply_user_completion(persistence.lease(), || {
                if source.is_current(app, &context, &persistence) {
                    let state = app.global::<AppState>(); state.set_viewer_processing(false);
                    state.set_viewer_message("本地保存结果尚未确认，请稍后重试".into());
                }
            });
        }
    }
}

fn poll_remove_black_store_ack(
    weak: Weak<AppWindow>, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, save_state: RemoveBlackSaveState,
    item_id: String, cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<(), DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => {
                poll_remove_black_store_ack(weak, context, persistence, source, save_state, item_id, cancel, receiver);
                return;
            }
            Err(_) => {
                // The worker may have sent an acknowledged receipt before its
                // registered thread failed. Keep the staged row and suppress
                // success publication; never manufacture a memory rollback.
                let _ = context.apply_user_completion(persistence.lease(), || {
                    if source.is_current(&app, &context, &persistence) {
                        let state = app.global::<AppState>(); state.set_viewer_processing(false);
                        state.set_viewer_message("本地保存结果尚未确认，请稍后重试".into());
                    }
                });
                return;
            }
            Ok(false) => {}
        }
        match receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                let _ = context.apply_user_completion(persistence.lease(), || {
                    if source.is_current(&app, &context, &persistence) {
                        let state = app.global::<AppState>(); state.set_viewer_processing(false);
                        state.set_viewer_message("处理结果已暂存，本地保存未确认；请重试保存".into());
                    }
                });
                return;
            }
            Err(_) => {
            let _ = context.apply_user_completion(persistence.lease(), || {
                if source.is_current(&app, &context, &persistence) {
                    let state = app.global::<AppState>(); state.set_viewer_processing(false);
                    state.set_viewer_message("本地保存结果尚未确认，请稍后重试".into());
                }
            });
            return;
            }
        }
        let visuals = prepare_delivery_visuals(&app, &context.store.borrow());
        let mut visuals = Some(visuals);
        let effects = context.apply_user_completion(persistence.lease(), || {
            if !source.is_current(&app, &context, &persistence)
                || !context.store.borrow().assets.iter().any(|item| item.id == item_id) { return None; }
            let state = app.global::<AppState>(); state.set_viewer_processing_progress(100);
            state.set_viewer_processing(false); state.set_viewer_open(false);
            state.set_viewer_image(Image::default()); state.set_viewer_source_path("".into());
            state.set_page("generation".into());
            Some(visuals.take().expect("single remove-black visuals").publish_metadata(&app, persistence.clone()))
        }).ok().flatten();
        drop(visuals);
        if let Some(effects) = effects { start_activation_visual_effects(&app, context, effects); }
        if save_state.borrow().as_ref().is_some_and(|saved| saved.item_id == item_id) {
            save_state.borrow_mut().take();
        }
    });
}

fn retry_captured_remove_black_save(
    app: &AppWindow, context: AppContext, persistence: PrivatePersistence,
    source: Rc<CapturedViewerSource>, save_state: RemoveBlackSaveState, item_id: String,
) {
    let Ok(write) = persistence.prepare_ordered_save() else { return; };
    let mut write = Some(write);
    let queued = context.apply_user_completion(persistence.lease(), || {
        if !source.is_current(app, &context, &persistence)
            || !context.store.borrow().assets.iter().any(|item| item.id == item_id) { return None; }
        let store = context.store.borrow();
        Some(write.take().expect("single remove-black retry write").enqueue(local_store_data(app, &store)))
    }).ok().flatten();
    drop(write);
    let Some(Ok(receiver)) = queued else { return; };
    if let Ok((cancel, receiver)) = spawn_delivery_preparation(&persistence, move |_, _, _| {
        receiver.recv().map_err(|_| anyhow!("viewer retry Store acknowledgment disconnected"))?
            .map_err(anyhow::Error::from)?;
        Ok(())
    }) {
        poll_remove_black_store_ack(app.as_weak(), context, persistence, source, save_state, item_id, cancel, receiver);
    }
}

// Metadata provenance only. Reading still requires held namespace authority.
#[derive(Clone,PartialEq,Eq)]
enum ViewerSourceTarget {
    Asset(String),Category(String),Canvas(String),CanvasNode(String),Custom{session:String,original:String,return_page:String},
}
#[derive(Clone,Copy,PartialEq,Eq)]
enum ViewerSourcePresentation { Viewer,Cutout }
pub(super) struct CapturedViewerSource {
    store:Rc<RefCell<Store>>,persistence:PrivatePersistence,session:SessionScope,
    page:String,id:String,source:String,path:PathBuf,target:ViewerSourceTarget,presentation:ViewerSourcePresentation,
}
impl CapturedViewerSource {
    pub(super) fn path(&self)->&Path {&self.path}
    pub(super) fn source_id(&self)->&str {&self.id}
    /// Pure metadata: safe inside an already admitted completion, never reacquires
    /// the upgrade latch or a counted permit, nor reads/opens a filesystem path.
    pub(super) fn is_current(&self,app:&AppWindow,context:&AppContext,persistence:&PrivatePersistence)->bool {
        if !Rc::ptr_eq(&self.store,&context.store) || !self.persistence.same_binding_metadata(persistence) {return false;}
        let store=context.store.borrow();
        if !store.private_persistence.as_ref().is_some_and(|current|current.same_binding_metadata(&self.persistence)) {return false;}
        if !context.active_namespace.lock().ok().is_some_and(|active|active.as_ref()==Some(self.persistence.lease()))
            || !context.backend.as_ref().is_some_and(|backend|backend.api.session().is_scope_current(&self.session)) {return false;}
        let state=app.global::<AppState>();
        if state.get_page()!=self.page || state.get_viewer_id()!=self.id || state.get_viewer_source()!=self.source
            || Path::new(state.get_viewer_source_path().as_str())!=self.path
            || viewer_source_presentation(&state)!=Some(self.presentation) {return false;}
        match &self.target {
            ViewerSourceTarget::Asset(source)=>viewer_item(&store,&self.id,source).is_some_and(|row|Path::new(&row.source_path)==self.path),
            ViewerSourceTarget::Category(category)=>resolve_category(state.get_asset_type().as_str(),"")==*category
                && references_for_category(&store.references,category).iter().any(|row|row.id==self.id && Path::new(&row.source_path)==self.path),
            ViewerSourceTarget::Canvas(workspace)=>store.active_canvas_workspace_id==*workspace
                && store.canvas_references.iter().any(|row|row.id==self.id && Path::new(&row.source_path)==self.path),
            ViewerSourceTarget::CanvasNode(workspace)=>store.active_canvas_workspace_id==*workspace
                && store.canvas_notes.iter().any(|row|row.id==self.id && Path::new(&row.image_path)==self.path),
            ViewerSourceTarget::Custom{session,original,return_page}=>state.get_custom_prompt_editor_open()
                && state.get_custom_prompt_editor_session_id()==*session && state.get_custom_prompt_editing_original()==*original
                && state.get_custom_prompt_editor_return_page()==*return_page
                && state.get_custom_prompt_reference_items().iter().any(|row|row.id==self.id && Path::new(row.source_path.as_str())==self.path),
        }
    }
}
fn viewer_source_presentation(state:&AppState)->Option<ViewerSourcePresentation> {
    if state.get_viewer_open() && !state.get_cutout_open(){Some(ViewerSourcePresentation::Viewer)}
    else if state.get_cutout_open() && !state.get_viewer_open(){Some(ViewerSourcePresentation::Cutout)}else{None}
}
pub(super) fn capture_current_viewer_source(app:&AppWindow,context:&AppContext)->Result<CapturedViewerSource> {
    let state=app.global::<AppState>();let id=state.get_viewer_id().to_string();let source=state.get_viewer_source().to_string();
    let path_text=state.get_viewer_source_path().to_string();
    anyhow::ensure!(!id.is_empty() && !matches!(path_text.trim(),""|"failed"|"asset"),"viewer original source is unproven");
    let path=PathBuf::from(path_text);let page=state.get_page().to_string();
    let presentation=viewer_source_presentation(&state).ok_or_else(||anyhow!("original viewer presentation unavailable"))?;
    let store=context.store.borrow();let persistence=store.private_persistence.clone().ok_or_else(||anyhow!("original Store binding missing"))?;
    anyhow::ensure!(persistence.owns_path(&path),"viewer original path is not owned");
    let target=if source=="reference" {
        let category=resolve_category(state.get_asset_type().as_str(),"");
        if page=="generation" && references_for_category(&store.references,&category).iter().any(|row|row.id==id && Path::new(&row.source_path)==path){ViewerSourceTarget::Category(category)}
        else if page=="canvas" && store.canvas_references.iter().any(|row|row.id==id && Path::new(&row.source_path)==path){ViewerSourceTarget::Canvas(store.active_canvas_workspace_id.clone())}
        else if state.get_custom_prompt_editor_open() && !state.get_custom_prompt_editor_session_id().is_empty()
            && state.get_custom_prompt_reference_items().iter().any(|row|row.id==id && Path::new(row.source_path.as_str())==path){
            ViewerSourceTarget::Custom{session:state.get_custom_prompt_editor_session_id().into(),original:state.get_custom_prompt_editing_original().into(),return_page:state.get_custom_prompt_editor_return_page().into()}
        }else{anyhow::bail!("viewer reference no longer matches its original target");}
    }else if source == "canvas" {
        anyhow::ensure!(page == "canvas" && store.canvas_notes.iter().any(|row| row.id == id
            && matches!(row.kind.as_str(), "image" | "board-image") && Path::new(&row.image_path) == path),
            "viewer canvas image no longer matches its original target");
        ViewerSourceTarget::CanvasNode(store.active_canvas_workspace_id.clone())
    }else{
        anyhow::ensure!(matches!(source.as_str(),"asset"|"generation"|"inspiration"),"unsupported viewer source");
        anyhow::ensure!(viewer_item(&store,&id,&source).is_some_and(|row|Path::new(&row.source_path)==path),"viewer asset no longer matches its original target");
        ViewerSourceTarget::Asset(source.clone())
    };
    drop(store);
    let session=context.current_account_session_scope().ok_or_else(||anyhow!("original viewer session missing"))?;
    let captured=CapturedViewerSource{store:context.store.clone(),persistence:persistence.clone(),session,page,id,source,path,target,presentation};
    anyhow::ensure!(captured.is_current(app,context,&persistence),"original viewer target changed");Ok(captured)
}

// Exact metadata provenance tests; no native effects or global paths.
// Only the import's own acknowledged DEFAULT-target transition may consult the
// saved original workspace. Ordinary source checks above stay strict.
fn viewer_canvas_source_after_target(source:&CapturedViewerSource, app:&AppWindow, context:&AppContext)->bool {
    let (ViewerSourceTarget::Canvas(original_workspace) | ViewerSourceTarget::CanvasNode(original_workspace))=&source.target else {
        return source.is_current(app,context,&source.persistence);
    };
    if source.is_current(app,context,&source.persistence) { return true; }
    if !Rc::ptr_eq(&source.store,&context.store) { return false; }
    let store=context.store.borrow();
    if normalize_canvas_workspace_id(&store.active_canvas_workspace_id)!=DEFAULT_CANVAS_WORKSPACE_ID
        || !store.private_persistence.as_ref().is_some_and(|binding|binding.same_binding_metadata(&source.persistence))
        || !context.active_namespace.lock().ok().is_some_and(|active|active.as_ref()==Some(source.persistence.lease()))
        || !context.backend.as_ref().is_some_and(|backend|backend.api.session().is_scope_current(&source.session)) { return false; }
    let state=app.global::<AppState>();
    state.get_page()==source.page && state.get_viewer_id()==source.id && state.get_viewer_source()==source.source
        && Path::new(state.get_viewer_source_path().as_str())==source.path
        && viewer_source_presentation(&state)==Some(source.presentation)
        && store.canvas_workspaces.get(original_workspace).is_some_and(|workspace|
            saved_canvas_viewer_source_matches(source, workspace))
}

#[cfg(test)]
thread_local! { static VIEWER_CANVAS_COMPLETIONS:Cell<u32>=const{Cell::new(0)}; }
fn finish_viewer_canvas_import(app:&AppWindow,context:&AppContext,source:&CapturedViewerSource,result:Result<()>) {
    #[cfg(test)]
    VIEWER_CANVAS_COMPLETIONS.with(|count|count.set(count.get()+1));
    if !source.persistence.is_current() { return; }
    let mut projections=if result.is_ok() {
        let store=context.store.borrow();
        Some((prepare_canvas_projection(app,&store),prepare_canvas_reference_projection(app,&store)))
    } else { None };
    let mut effects=None;
    let _=context.apply_user_completion(source.persistence.lease(),||{
        if !viewer_canvas_source_after_target(source,app,context) { return; }
        let state=app.global::<AppState>();
        if result.is_err() {
            state.set_viewer_message(if state.get_language().as_str()=="en" {
                "Canvas save was not confirmed; the original viewer is unchanged"
            } else {"画布保存未确认，原查看图片未改变"}.into());
            return;
        }
        let (canvas,references)=projections.take().expect("prepared viewer canvas projection");
        effects=Some((canvas.publish_metadata(app),references.publish_metadata(app)));
        state.set_viewer_message("".into());state.set_viewer_open(false);
        state.set_viewer_image(Image::default());state.set_viewer_source_path("".into());
        state.set_generation_status(if state.get_language().as_str()=="en" {
            "Image imported to the infinite canvas"
        } else {"图片已导入无限画布"}.into());
        state.set_canvas_workflow_id("".into());state.set_canvas_workflow_title("".into());
        state.set_canvas_workflow_template("".into());state.set_canvas_workflow_hint("".into());
        *context.canvas_history.borrow_mut()=CanvasController::default();
        state.set_canvas_can_undo(false);state.set_canvas_can_redo(false);
        state.set_canvas_workspace_switch_request(state.get_canvas_workspace_switch_request().saturating_add(1));
        state.set_page("canvas".into());
    });
    if let Some((canvas,references))=effects {
        start_canvas_preview_effects(app,source.persistence.clone(),canvas);
        start_canvas_reference_preview_effects(app,source.persistence.clone(),references);
    }
}

type ViewerCanvasPending=Rc<RefCell<Option<(Rc<CapturedViewerSource>,ViewerCanvasImportStage)>>>;
fn start_or_retry_viewer_canvas_import(
    app:&AppWindow,context:AppContext,pending:ViewerCanvasPending,running:Rc<Cell<bool>>,
){
    if running.get(){return;}
    let previous=pending.borrow().as_ref().map(|(source,stage)|(source.clone(),stage.clone()));
    let (source,stage,retry)=if let Some((source,stage))=previous.filter(|(source,stage)|
        source.persistence.is_current() && stage.borrow().is_some() && viewer_canvas_source_after_target(source,app,&context)) {
        let retry=stage.borrow().clone();(source,stage,retry)
    }else{
        let Ok(source)=capture_current_viewer_source(app,&context)else{return;};
        if !source.persistence.is_current(){return;}
        (Rc::new(source),Rc::new(RefCell::new(None)),None)
    };
    *pending.borrow_mut()=Some((source.clone(),stage.clone()));running.set(true);
    let original=source.clone();let checked=source.clone();let completed_context=context.clone();
    let completed_pending=pending.clone();let completed_running=running.clone();let weak=app.as_weak();
    let completed=move|result:Result<()>|{
        completed_running.set(false);
        if result.is_ok() && completed_pending.borrow().as_ref().is_some_and(|(source,_)|Rc::ptr_eq(source,&original)){
            completed_pending.borrow_mut().take();
        }
        if let Some(app)=weak.upgrade(){finish_viewer_canvas_import(&app,&completed_context,&original,result);}
    };
    let target=move|app:&AppWindow,context:&AppContext,after_switch:bool|{
        if after_switch{viewer_canvas_source_after_target(&checked,app,context)}
        else{checked.is_current(app,context,&checked.persistence)}
    };
    let result=if let Some(retry)=retry {
        retry_staged_viewer_canvas_import(app,context.clone(),retry,target,completed)
    }else{
        start_captured_viewer_image_import_to_canvas_with_stage(app,context.clone(),source.path().to_owned(),
            DEFAULT_CANVAS_WORKSPACE_ID.into(),stage,target,completed)
    };
    if let Err(error)=result {
        running.set(false);finish_viewer_canvas_import(app,&context,&source,Err(error));
    }
}

#[cfg(test)]
mod captured_viewer_source_tests {
    use super::*;
    struct Fixture(video_image_callbacks::tests::scoped_inputs::Fixture);
    impl std::ops::Deref for Fixture {type Target=video_image_callbacks::tests::scoped_inputs::Fixture;fn deref(&self)->&Self::Target{&self.0}}
    impl Drop for Fixture {fn drop(&mut self){
        let workers=drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
        let retired=self.context.user_activity.begin_quiesce(self.persistence.lease()).map(|guard|guard.retire());
        if !std::thread::panicking(){workers.unwrap();retired.unwrap();}
    }}
    fn setup()->(Fixture,AppWindow,ReferenceData) {
        i_slint_backend_testing::init_no_event_loop();
        let fixture=Fixture(video_image_callbacks::tests::scoped_inputs::Fixture::new());
        let app=AppWindow::new().unwrap();
        let image=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(80,80,image::Rgba([10,20,30,255])));
        let path=persist_reference_image_for_namespace(&fixture.authority,&image).unwrap();
        let row=ReferenceData{id:"owned-reference".into(),source_path:path.to_string_lossy().into_owned()};
        fixture.context.store.borrow_mut().references.character.push(row.clone());
        let state=app.global::<AppState>();state.set_page("generation".into());state.set_asset_type("character".into());
        state.set_viewer_open(true);state.set_viewer_source("reference".into());state.set_viewer_id(row.id.clone().into());state.set_viewer_source_path(row.source_path.clone().into());
        (fixture,app,row)
    }
    #[test]
    fn core_viewer_source_reference_cutout_metadata_never_requires_cached_pixels() {
        let(f,app,row)=setup();let state=app.global::<AppState>();
        state.set_viewer_image(Image::default());state.set_viewer_open(false);state.set_cutout_open(true);
        let source=capture_current_viewer_source(&app,&f.context).unwrap();
        assert_eq!(source.path(),Path::new(&row.source_path));assert_eq!(source.source_id(),"owned-reference");
        assert!(f.context.apply_user_completion(f.persistence.lease(),||source.is_current(&app,&f.context,&f.persistence)).unwrap());
        let bytes=f.authority.read_image_source(source.path(),1024*1024).unwrap();
        assert_eq!(decode_reference_bytes(&bytes).unwrap().width(),80);
        state.set_asset_type("scene".into());assert!(!source.is_current(&app,&f.context,&f.persistence));
    }
    #[test]
    fn core_viewer_source_rejects_unproven_pixels_wrong_path_and_replacement_binding() {
        let(f,app,row)=setup();let state=app.global::<AppState>();
        let source=capture_current_viewer_source(&app,&f.context).unwrap();
        state.set_viewer_source_path("".into());assert!(capture_current_viewer_source(&app,&f.context).is_err());
        assert!(!source.is_current(&app,&f.context,&f.persistence));
        state.set_viewer_source_path("failed".into());assert!(capture_current_viewer_source(&app,&f.context).is_err());
        state.set_viewer_source_path(row.source_path.into());state.set_viewer_id("unrelated".into());
        assert!(capture_current_viewer_source(&app,&f.context).is_err());state.set_viewer_id(row.id.into());
        let same=f.persistence.clone();assert!(source.is_current(&app,&f.context,&same));
        let other=PrivatePersistence::for_test((*f.writer).clone(),f.persistence.lease().clone(),f.context.user_activity.clone(),f.persistence.upgrade_latch());
        assert!(!source.is_current(&app,&f.context,&other));
        f.context.store.borrow_mut().private_persistence=Some(other);
        assert!(!source.is_current(&app,&f.context,&f.persistence));
    }
    #[test]
    fn core_viewer_source_custom_reference_keeps_original_editor_target_after_panel_switch() {
        let(f,app,row)=setup();let state=app.global::<AppState>();
        f.context.store.borrow_mut().references.character.clear();
        state.set_page("settings".into());state.set_custom_prompt_editor_open(true);state.set_custom_prompt_editor_session_id("editor-A".into());
        state.set_custom_prompt_reference_items(ModelRc::new(VecModel::from(vec![ReferenceItem{id:row.id.clone().into(),source_path:row.source_path.into(),..Default::default()}])));
        let source=capture_current_viewer_source(&app,&f.context).unwrap();
        assert!(source.is_current(&app,&f.context,&f.persistence));
        state.set_custom_prompt_editor_session_id("editor-B".into());assert!(!source.is_current(&app,&f.context,&f.persistence));
        state.set_custom_prompt_editor_session_id("editor-A".into());state.set_custom_prompt_reference_items(ModelRc::new(VecModel::from(Vec::<ReferenceItem>::new())));
        assert!(!source.is_current(&app,&f.context,&f.persistence));
    }
}

#[cfg(test)]
mod viewer_canvas_bridge_tests {
    use super::*;
    struct Fixture(video_image_callbacks::tests::scoped_inputs::Fixture);
    impl std::ops::Deref for Fixture {type Target=video_image_callbacks::tests::scoped_inputs::Fixture;fn deref(&self)->&Self::Target{&self.0}}
    impl Drop for Fixture {fn drop(&mut self){
        let canvas=drain_canvas_workers_for_lease_for_test(self.persistence.lease());
        let previews=drain_canvas_preview_workers_for_lease_for_test(self.persistence.lease());
        let delivery=drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
        let retired=self.context.user_activity.begin_quiesce(self.persistence.lease()).map(|guard|guard.retire());
        if !std::thread::panicking(){canvas.unwrap();previews.unwrap();delivery.unwrap();retired.unwrap();}
    }}
    fn setup()->(Fixture,AppWindow,ReferenceData) {
        i_slint_backend_testing::init_no_event_loop();
        let f=Fixture(video_image_callbacks::tests::scoped_inputs::Fixture::new());
        let transition=f.context.namespace_operations.try_begin_transition().unwrap();
        let recovery=transition.begin_prepublication_recovery(f.persistence.lease()).unwrap();
        recovery.verify_no_unsupported_imports(&f.authority).unwrap();
        let recovered=recovery.finish().unwrap();
        transition.prepare_publication(f.persistence.lease(),recovered).unwrap().publish();
        let image=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(80,80,image::Rgba([13,31,57,255])));
        let path=persist_reference_image_for_namespace(&f.authority,&image).unwrap();
        let reference=ReferenceData{id:"original-canvas-reference".into(),source_path:path.to_string_lossy().into_owned()};
        {
            let mut store=f.context.store.borrow_mut();
            store.active_canvas_workspace_id="source-workspace".into();
            store.canvas_references.push(reference.clone());
        }
        let app=AppWindow::new().unwrap();let state=app.global::<AppState>();
        state.set_page("canvas".into());state.set_logged_in(true);state.set_session_state("online".into());
        state.set_viewer_open(true);state.set_viewer_source("reference".into());
        state.set_viewer_id(reference.id.clone().into());state.set_viewer_source_path(reference.source_path.clone().into());
        state.set_viewer_message("original-viewer".into());state.set_generation_status("original-canvas".into());
        wire_viewer_callbacks(&app,f.context.clone());
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        (f,app,reference)
    }
    fn pump_completion(){video_image_callbacks::tests::scoped_inputs::pump(||VIEWER_CANVAS_COMPLETIONS.with(Cell::get)>0);}
    #[test]
    fn core_viewer_canvas_node_import_and_creation_keep_owned_original() {
        let (f, app, reference) = setup();
        let state = app.global::<AppState>();
        {
            let mut store = f.context.store.borrow_mut();
            store.canvas_references.clear();
            store.canvas_notes.push(CanvasNoteData { id: reference.id.clone(), kind: "board-image".into(),
                image_path: reference.source_path.clone(), width: 80.0, height: 80.0, ..Default::default() });
        }
        state.set_viewer_open(false);
        state.invoke_open_canvas_image_detail(reference.id.clone().into(), reference.source_path.clone().into(),
            Image::default(), "original".into(), 80.0, 80.0);
        assert!(state.get_viewer_open());
        assert_eq!(state.get_viewer_source(), "canvas");
        let source = capture_current_viewer_source(&app, &f.context).unwrap();
        assert!(source.is_current(&app, &f.context, &f.persistence));
        state.invoke_viewer_open_creation_workflow("building-derivation".into(), "建筑衍生器".into(), "template".into(), "hint".into());
        video_image_callbacks::tests::scoped_inputs::pump(|| !state.get_viewer_open());
        assert_eq!(state.get_asset_type(), "scene");
        assert_eq!(state.get_canvas_workflow_id(), "building-derivation");
        let saved = f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.active_canvas_workspace_id, "building-derivation");
        assert_eq!(saved.canvas_workspaces["building-derivation"].references.len(), 1);
        assert!(f.persistence.owns_path(Path::new(&saved.canvas_workspaces["building-derivation"].references[0].source_path)));
        assert_eq!(saved.canvas_workspaces["source-workspace"].notes[0].image_path, reference.source_path);
        assert!(!source.is_current(&app, &f.context, &f.persistence));
    }

    #[test]
    fn core_viewer_canvas_node_import_waits_for_default_workspace_save() {
        let (f, app, reference) = setup();
        let state = app.global::<AppState>();
        {
            let mut store = f.context.store.borrow_mut();
            store.canvas_references.clear();
            store.canvas_notes.push(CanvasNoteData { id: reference.id.clone(), kind: "board-image".into(),
                image_path: reference.source_path.clone(), width: 80.0, height: 80.0, ..Default::default() });
        }
        state.set_viewer_source("canvas".into());
        state.invoke_viewer_import_to_canvas();
        assert!(state.get_viewer_open());
        pump_completion();
        assert!(!state.get_viewer_open());
        let saved = f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.active_canvas_workspace_id, DEFAULT_CANVAS_WORKSPACE_ID);
        assert_eq!(saved.canvas_notes.len(), 1);
        assert_eq!(saved.canvas_workspaces["source-workspace"].notes[0].image_path, reference.source_path);
    }

    #[test]
    fn core_viewer_canvas_nondefault_reference_waits_for_real_default_target_save() {
        let(f,app,reference)=setup();
        f.context.store.borrow_mut().canvas_notes=(0..MAX_CANVAS_NODES).map(|index|
            CanvasNoteData{id:format!("original-{index}"),kind:"note".into(),..Default::default()}).collect();
        f.persistence.save_store(local_store_data(&app,&f.context.store.borrow())).unwrap();
        let original=f.authority.read_image_source(Path::new(&reference.source_path),1024*1024).unwrap();
        app.global::<AppState>().invoke_viewer_import_to_canvas();
        assert!(app.global::<AppState>().get_viewer_open());
        assert_eq!(f.context.store.borrow().active_canvas_workspace_id,"source-workspace");
        pump_completion();
        assert!(!app.global::<AppState>().get_viewer_open());
        let saved=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.active_canvas_workspace_id,DEFAULT_CANVAS_WORKSPACE_ID);
        assert_eq!(saved.canvas_notes.len(),1);
        assert_eq!(saved.canvas_workspaces["source-workspace"].references,vec![reference.clone()]);
        assert_eq!(saved.canvas_workspaces["source-workspace"].notes.len(),MAX_CANVAS_NODES);
        let path=PathBuf::from(&saved.canvas_notes[0].image_path);
        assert!(path.starts_with(f.persistence.lease().namespace.path(ManagedUserArea::CanvasUploads)));
        assert!(f.context.file_index.as_ref().unwrap().find_file_by_path_for_namespace(&f.authority,
            ManagedUserArea::CanvasUploads,path.file_name().unwrap().to_str().unwrap()).unwrap().is_some());
        assert_eq!(f.authority.read_image_source(Path::new(&reference.source_path),1024*1024).unwrap(),original);
    }
    #[test]
    fn core_viewer_canvas_changed_reference_rejects_actual_late_import() {
        let(f,app,_)=setup();
        app.global::<AppState>().invoke_viewer_import_to_canvas();
        f.context.store.borrow_mut().canvas_references.clear();
        pump_completion();
        assert!(app.global::<AppState>().get_viewer_open());
        assert_eq!(app.global::<AppState>().get_viewer_message(),"original-viewer");
        assert_eq!(f.context.store.borrow().active_canvas_workspace_id,"source-workspace");
        assert!(f.context.store.borrow().canvas_notes.is_empty());
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().canvas_notes.is_empty());
    }
    #[test]
    fn core_viewer_canvas_user_workspace_switch_rejects_actual_late_import() {
        let(f,app,_)=setup();
        app.global::<AppState>().invoke_viewer_import_to_canvas();
        switch_canvas_workspace(&mut f.context.store.borrow_mut(),"","later-workspace");
        pump_completion();
        assert!(app.global::<AppState>().get_viewer_open());
        assert_eq!(f.context.store.borrow().active_canvas_workspace_id,"later-workspace");
        assert!(f.context.store.borrow().canvas_notes.is_empty());
        assert_eq!(app.global::<AppState>().get_generation_status(),"original-canvas");
    }
    #[test]
    fn core_viewer_canvas_sqlite_failure_keeps_original_viewer_without_success() {
        let(f,app,_)=setup();
        let root=f.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql=rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_viewer_canvas_save BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled canvas save failure'); END;").unwrap();
        app.global::<AppState>().invoke_viewer_import_to_canvas();pump_completion();
        assert!(app.global::<AppState>().get_viewer_open());
        assert!(!app.global::<AppState>().get_viewer_source_path().is_empty());
        assert!(app.global::<AppState>().get_viewer_message().contains("保存未确认"));
        let saved=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.active_canvas_workspace_id,"source-workspace");assert!(saved.canvas_notes.is_empty());
        assert_eq!(f.context.store.borrow().canvas_notes.len(),1,"staged owned result must remain after save rejection");
    }
    #[test]
    fn core_viewer_canvas_failed_save_retries_exact_staged_default_without_duplicate() {
        let(f,app,reference)=setup();
        let original=f.authority.read_image_source(Path::new(&reference.source_path),1024*1024).unwrap();
        let root=f.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql=rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_viewer_canvas_retry BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled canvas retry failure'); END;").unwrap();
        app.global::<AppState>().invoke_viewer_import_to_canvas();pump_completion();
        assert!(app.global::<AppState>().get_viewer_open());
        assert_eq!(f.context.store.borrow().canvas_notes.len(),1);
        let staged_id=f.context.store.borrow().canvas_notes[0].id.clone();
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().canvas_notes.is_empty());
        sql.execute_batch("DROP TRIGGER reject_viewer_canvas_retry").unwrap();
        app.global::<AppState>().invoke_viewer_import_to_canvas();
        video_image_callbacks::tests::scoped_inputs::pump(||!app.global::<AppState>().get_viewer_open());
        let saved=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.active_canvas_workspace_id,DEFAULT_CANVAS_WORKSPACE_ID);
        assert_eq!(saved.canvas_notes.len(),1);assert_eq!(saved.canvas_notes[0].id,staged_id);
        assert_eq!(saved.canvas_workspaces["source-workspace"].references,vec![reference.clone()]);
        assert_eq!(f.authority.read_image_source(Path::new(&reference.source_path),1024*1024).unwrap(),original);
    }
    fn rejected_staged_import(f:&Fixture,app:&AppWindow)->rusqlite::Connection {
        let root=f.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql=rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_staged_canvas BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled staged save'); END;").unwrap();
        app.global::<AppState>().invoke_viewer_import_to_canvas();pump_completion();
        assert!(app.global::<AppState>().get_viewer_open());assert_eq!(f.context.store.borrow().canvas_notes.len(),1);
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().canvas_notes.is_empty());
        sql.execute_batch("DROP TRIGGER reject_staged_canvas").unwrap();sql
    }
    #[test]
    fn core_viewer_canvas_retry_saves_current_edits_and_repeated_click_does_not_append(){
        let(f,app,reference)=setup();let _sql=rejected_staged_import(&f,&app);
        let id=f.context.store.borrow().canvas_notes[0].id.clone();
        {
            let mut store=f.context.store.borrow_mut();store.canvas_notes[0].x=321.0;
            store.canvas_notes.push(CanvasNoteData{id:"later-note".into(),kind:"note".into(),..Default::default()});
        }
        app.global::<AppState>().invoke_viewer_import_to_canvas();
        app.global::<AppState>().invoke_viewer_import_to_canvas();
        video_image_callbacks::tests::scoped_inputs::pump(||!app.global::<AppState>().get_viewer_open());
        let saved=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.canvas_notes.len(),2);assert_eq!(saved.canvas_notes.iter().filter(|note|note.id==id).count(),1);
        assert_eq!(saved.canvas_notes.iter().find(|note|note.id==id).unwrap().x,321.0);
        assert!(saved.canvas_notes.iter().any(|note|note.id=="later-note"));
        assert_eq!(saved.canvas_workspaces["source-workspace"].references,vec![reference]);
    }
    #[test]
    fn core_viewer_canvas_retry_rejects_staged_node_path_replacement_without_new_write(){
        let(f,app,reference)=setup();let _sql=rejected_staged_import(&f,&app);
        f.context.store.borrow_mut().canvas_notes[0].image_path=reference.source_path.clone();
        let before=VIEWER_CANVAS_COMPLETIONS.with(Cell::get);
        app.global::<AppState>().invoke_viewer_import_to_canvas();
        video_image_callbacks::tests::scoped_inputs::pump(||VIEWER_CANVAS_COMPLETIONS.with(Cell::get)>before);
        assert!(app.global::<AppState>().get_viewer_open());assert_eq!(f.context.store.borrow().canvas_notes.len(),1);
        assert_eq!(f.context.store.borrow().canvas_notes[0].image_path,reference.source_path);
        let saved=f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.active_canvas_workspace_id,"source-workspace");assert!(saved.canvas_notes.is_empty());
    }
    #[test]
    fn core_viewer_canvas_retry_rejects_changed_owned_bytes_and_retains_both_files(){
        let(f,app,reference)=setup();let _sql=rejected_staged_import(&f,&app);
        let path=PathBuf::from(&f.context.store.borrow().canvas_notes[0].image_path);
        let original=std::fs::read(&reference.source_path).unwrap();
        let mut changed=std::fs::read(&path).unwrap();let last=changed.len()-1;changed[last]^=1;
        std::fs::write(&path,&changed).unwrap();
        let before=VIEWER_CANVAS_COMPLETIONS.with(Cell::get);
        app.global::<AppState>().invoke_viewer_import_to_canvas();
        video_image_callbacks::tests::scoped_inputs::pump(||VIEWER_CANVAS_COMPLETIONS.with(Cell::get)>before);
        assert!(app.global::<AppState>().get_viewer_open());assert_eq!(f.context.store.borrow().canvas_notes.len(),1);
        assert_eq!(std::fs::read(&path).unwrap(),changed);assert_eq!(std::fs::read(&reference.source_path).unwrap(),original);
        assert!(f.writer.load_client_state_for_namespace(f.persistence.lease()).unwrap().unwrap().canvas_notes.is_empty());
    }

}

pub(super) fn wire_viewer_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();
    let canvas_history = context.canvas_history.clone();
    let image_editor_points = Rc::new(VecModel::<BrushPoint>::default());
    let image_editor_last_point = Rc::new(RefCell::new(None::<(f32, f32, f32)>));
    let image_editor_source = Rc::new(RefCell::new(None::<Rc<CapturedViewerSource>>));
    let image_editor_flight = Rc::new(RefCell::new(None::<Uuid>));
    let remove_black_save = Rc::new(RefCell::new(None::<RemoveBlackStagedSave>));
    let viewer_reference_save = Rc::new(RefCell::new(None::<ViewerReferenceFlight>));
    state.set_image_editor_points(image_editor_points.clone().into());

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_open_viewer(move |id, source| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            start_captured_viewer_projection(&app, &context, &capture, id.as_str(), source.as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_close_viewer(move || {
            if let Some(app) = app_weak.upgrade() {
                let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
                let _ = capture.apply(&context, || {
                    let state = app.global::<AppState>();
                    state.set_viewer_message("".into());
                    state.set_viewer_open(false);
                    state.set_viewer_image(Image::default());
                    state.set_viewer_category("".into());
                    state.set_viewer_source_path("".into());
                });
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_viewer_prev(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            move_captured_viewer(&app, &context, &capture, -1);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_viewer_next(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            move_captured_viewer(&app, &context, &capture, 1);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_download_asset(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            download_asset(&app, &store, id.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_viewer_copy_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            copy_viewer_image(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_download_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            download_viewer_image(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_open_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open_viewer_image(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_start_viewer_file_drag(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let state = app.global::<AppState>();
            let id = state.get_viewer_id().to_string();
            let source = state.get_viewer_source().to_string();
            let path = viewer_item(&store.borrow(), &id, &source)
                .map(|item| PathBuf::from(item.source_path.trim()));
            let Some(path) = path else {
                return false;
            };
            // The shared callback captures this current Store/view target and
            // prepares the held source in its registered worker. It owns the
            // later native dispatch and captured pointer reset.
            state.invoke_start_thumbnail_file_drag(path.to_string_lossy().into_owned().into())
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_viewer_cutout_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let _ = capture.apply(&context, || {
                let state = app.global::<AppState>();
                state.set_cutout_type("general".into());
                state.set_cutout_message("".into());
                state.set_cutout_progress(0);
                state.set_cutout_result_path("".into());
                state.set_cutout_result_name("".into());
                state.set_cutout_result_image(Image::default());
                state.set_cutout_estimated_credits("20".into());
                state.set_viewer_open(false);
                state.set_cutout_open(true);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_close_cutout(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let _ = capture.apply(&context, || {
                let state = app.global::<AppState>();
                if state.get_cutout_processing() {
                    state.set_cutout_message(if state.get_language().as_str() == "en" {
                        "Please wait for the cutout task to finish"
                    } else { "抠图处理中，请等待任务完成" }.into());
                    return;
                }
                state.set_cutout_open(false);
                state.set_cutout_message("".into());
                state.set_viewer_open(true);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let save_state = remove_black_save.clone();
        state.on_viewer_remove_black(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_captured_remove_black(&app, context.clone(), save_state.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_open_upscale_dialog(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let _ = capture.apply(&context, || {
                let state = app.global::<AppState>();
                if state.get_viewer_upscale_done() {
                    state.set_viewer_message(processing_done_message(&app, ProcessImageMode::Upscale {
                        scale: 2, target_long_edge: 2048,
                    }).into());
                    return;
                }
                state.set_upscale_scale(2);
                state.set_upscale_quality("2K".into());
                state.set_upscale_open(true);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_close_upscale_dialog(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let _ = capture.apply(&context, || {
                let state = app.global::<AppState>();
                if !state.get_viewer_processing() { state.set_upscale_open(false); }
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_start_upscale_image(move |scale, quality| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_backend_upscale(
                &app,
                context.clone(),
                scale.clamp(2, 4) as u32,
                quality.to_string(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_viewer_regenerate(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_captured_regeneration(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_viewer_edit(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let edited = capture.apply(&context, || {
                let state = app.global::<AppState>();
                state.set_prompt(state.get_viewer_prompt());
                state.set_quote_title(state.get_viewer_title());
                state.set_quote_prompt(state.get_viewer_prompt());
                state.set_quote_ratio(state.get_viewer_ratio());
                state.set_quote_quality(state.get_viewer_quality());
                state.set_viewer_open(false);
                state.set_viewer_image(Image::default());
                state.set_viewer_source_path("".into());
                state.set_page("generation".into());
            }).is_some();
            if edited {
                let visuals = prepare_delivery_visuals(&app, &context.store.borrow());
                let mut visuals = Some(visuals);
                let effects = capture.apply(&context, || {
                    (app.global::<AppState>().get_page() == "generation")
                        .then(|| visuals.take().expect("single viewer edit projection").publish_metadata(&app, capture.persistence.clone()))
                }).flatten();
                drop(visuals);
                if let Some(effects) = effects { start_activation_visual_effects(&app, context.clone(), effects); }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let points = image_editor_points.clone();
        let last_point = image_editor_last_point.clone();
        let context = context.clone();
        let editor_source = image_editor_source.clone();
        state.on_viewer_open_image_editor(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_captured_image_editor(&app, context.clone(), points.clone(), last_point.clone(), editor_source.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let last_point = image_editor_last_point.clone();
        let points = image_editor_points.clone();
        let context = context.clone();
        let editor_source = image_editor_source.clone();
        state.on_close_image_editor(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let _ = capture.apply(&context, || {
                let state = app.global::<AppState>();
                if state.get_image_editor_generating() { return; }
                let return_page = state.get_image_editor_return_page().to_string();
                *last_point.borrow_mut() = None; points.clear();
                state.set_image_editor_image(Image::default()); state.set_image_editor_source_path("".into());
                editor_source.borrow_mut().take();
                state.set_page(return_page.into()); state.set_viewer_open(true);
            });
        });
    }

    {
        let points = image_editor_points.clone();
        let last_point = image_editor_last_point.clone();
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_begin_image_editor_stroke(move |x, y, size, aspect, shape, color| {
            let Some(app) = app_weak.upgrade() else { return; };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let _ = capture.apply(&context, || {
                if app.global::<AppState>().get_page() != "image-editor"
                    || app.global::<AppState>().get_image_editor_generating() { return; }
                append_brush_segment(&points, None, (x, y, size), aspect, &shape, color);
                *last_point.borrow_mut() = Some((x, y, size));
            });
        });
    }

    {
        let points = image_editor_points.clone();
        let last_point = image_editor_last_point.clone();
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_continue_image_editor_stroke(move |x, y, size, aspect, shape, color| {
            let Some(app) = app_weak.upgrade() else { return; };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let _ = capture.apply(&context, || {
                if app.global::<AppState>().get_page() != "image-editor"
                    || app.global::<AppState>().get_image_editor_generating() { return; }
                let previous = *last_point.borrow();
                append_brush_segment(&points, previous, (x, y, size), aspect, &shape, color);
                *last_point.borrow_mut() = Some((x, y, size));
            });
        });
    }

    {
        let last_point = image_editor_last_point;
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_end_image_editor_stroke(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
            let _ = capture.apply(&context, || {
                let state = app.global::<AppState>();
                if state.get_page() == "image-editor" && !state.get_image_editor_generating() {
                    *last_point.borrow_mut() = None;
                }
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let points = image_editor_points.clone();
        let context = context.clone();
        let editor_source = image_editor_source.clone();
        let flight = image_editor_flight.clone();
        state.on_submit_image_edit(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_captured_image_edit(&app, context.clone(), points.clone(), editor_source.clone(), flight.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let save_state = viewer_reference_save.clone();
        state.on_viewer_use_same(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(state.get_asset_type().as_str(), "");
            let prompt = state.get_viewer_prompt().to_string();
            let conversation_id = Uuid::new_v4().to_string();
            start_captured_viewer_reference(&app, context.clone(),
                CapturedViewerReferenceIntent::Same { category, prompt, conversation_id }, save_state.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let save_state = viewer_reference_save.clone();
        state.on_viewer_use_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let category = resolve_category(app.global::<AppState>().get_asset_type().as_str(), "");
            start_captured_viewer_reference(&app, context.clone(),
                CapturedViewerReferenceIntent::Reference { category }, save_state.clone());
        });
    }

    {
        let app_weak=app.as_weak();let context=context.clone();
        let pending:ViewerCanvasPending=Rc::new(RefCell::new(None));
        let running=Rc::new(Cell::new(false));
        state.on_viewer_import_to_canvas(move||{
            let Some(app)=app_weak.upgrade()else{return;};
            start_or_retry_viewer_canvas_import(&app,context.clone(),pending.clone(),running.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_open_canvas_image_detail(
            move |id, source_path, image, prompt, width, height| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                if source_path.trim().is_empty() {
                    return;
                }
                let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
                let _ = capture.apply(&context, || {
                if app.global::<AppState>().get_page() != "canvas" || !context.store.borrow().canvas_notes.iter()
                    .any(|row| row.id == id.as_str() && row.image_path == source_path.as_str()
                        && matches!(row.kind.as_str(), "image" | "board-image")) { return; }
                let state = app.global::<AppState>();
                let prompt = prompt.to_string();
                state.set_viewer_message("".into());
                state.set_viewer_id(id);
                state.set_viewer_source("canvas".into());
                state.set_viewer_category(state.get_asset_type());
                state.set_viewer_source_path(source_path);
                state.set_viewer_image(image);
                state.set_viewer_title(
                    if state.get_canvas_workflow_title().is_empty() {
                        if state.get_language().as_str() == "en" {
                            "Canvas Image".into()
                        } else {
                            "画布图片".into()
                        }
                    } else {
                        state.get_canvas_workflow_title()
                    },
                );
                state.set_viewer_prompt(prompt.clone().into());
                state.set_viewer_prompt_lines(estimated_prompt_lines(&prompt));
                state.set_viewer_time("".into());
                state.set_viewer_ratio(state.get_ratio());
                state.set_viewer_quality(state.get_quality());
                state.set_viewer_model(state.get_image_model_name());
                state.set_viewer_repeat_enabled(false);
                state.set_viewer_cutout_done(false);
                state.set_viewer_remove_black_done(false);
                state.set_viewer_upscale_done(false);
                state.set_viewer_width(width.round().max(1.0) as i32);
                state.set_viewer_height(height.round().max(1.0) as i32);
                state.set_viewer_open(true);
                });
            },
        );
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let save_state = viewer_reference_save.clone();
        state.on_viewer_open_creation_workflow(move |workflow_id, title, template, hint| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(_capture) = ViewerActionCapture::capture(&context) else { return; };
            let state = app.global::<AppState>();
            let is_scene_workflow = matches!(
                workflow_id.as_str(),
                "plant-growth" | "monster-generator" | "upgrade-evolution" | "building-derivation"
            );
            let is_character_workflow = matches!(
                workflow_id.as_str(),
                "character-age" | "character-outfit" | "character-body"
            );
            if !is_scene_workflow && !is_character_workflow {
                state.set_viewer_message(
                    if state.get_language().as_str() == "en" {
                        "Unsupported import workflow"
                    } else {
                        "不支持的导入方式"
                    }
                    .into(),
                );
                return;
            }
            start_captured_viewer_reference(&app, context.clone(), CapturedViewerReferenceIntent::Creation {
                workflow_id: workflow_id.into(), title: title.into(), template: template.into(), hint: hint.into(),
                original_prompt: state.get_canvas_workflow_prompt().to_string(),
            }, save_state.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_request_delete_asset(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
                let _ = capture.apply(&context, || {
                    let state = app.global::<AppState>();
                    let source = state.get_viewer_source().to_string();
                    let can_remove_file = false;
                    state.set_pending_delete_kind("asset".into());
                    state.set_pending_delete_id(id); state.set_pending_delete_source(source.into());
                    state.set_pending_delete_can_remove_file(can_remove_file); state.set_delete_confirm_open(true);
                });
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_request_delete_thumbnail(move |id, source| {
            if let Some(app) = app_weak.upgrade() {
                let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
                let _ = capture.apply(&context, || {
                    let state = app.global::<AppState>();
                    let can_remove_file = false;
                    state.set_pending_delete_kind("asset".into()); state.set_pending_delete_id(id);
                    state.set_pending_delete_source(source); state.set_pending_delete_can_remove_file(can_remove_file);
                    state.set_delete_confirm_open(true);
                });
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let context = context.clone();
        state.on_confirm_delete(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_captured_asset_delete(&app, context.clone(), false);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let context = context.clone();
        state.on_confirm_delete_local_file(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The user deferred local-file deletion. This remains a metadata-only
            // delete and never unlinks the owned source.
            start_captured_asset_delete(&app, context.clone(), true);
        });
    }
}

enum RemovedStoreRecord {
    Asset {
        source: String,
        index: usize,
        item: AssetData,
    },
    Reference {
        category: String,
        index: usize,
        item: ReferenceData,
    },
}

impl RemovedStoreRecord {
    fn source_path(&self) -> &str {
        match self {
            Self::Asset { item, .. } => &item.source_path,
            Self::Reference { item, .. } => &item.source_path,
        }
    }

    fn restore(self, store: &mut Store) {
        match self {
            Self::Asset {
                source,
                index,
                item,
            } => {
                let items = asset_collection_mut(store, &source);
                items.insert(index.min(items.len()), item);
            }
            Self::Reference {
                category,
                index,
                item,
            } => {
                let items = references_for_category_mut(&mut store.references, &category);
                items.insert(index.min(items.len()), item);
            }
        }
    }

    fn recoverable_failed_delivery_id(&self) -> Option<&str> {
        match self {
            Self::Asset { source, item, .. }
                if source == "generation"
                    && item.source_path == "failed"
                    && item.delivery_recoverable =>
            {
                Some(item.id.as_str())
            }
            Self::Asset { .. } | Self::Reference { .. } => None,
        }
    }

    fn conflicts_with_current(&self, store: &Store) -> bool {
        match self {
            Self::Asset { source, item, .. } => viewer_item(store, &item.id, source).is_some(),
            Self::Reference { category, item, .. } =>
                references_for_category(&store.references, category).iter().any(|row| row.id == item.id),
        }
    }
}

enum StoreRecordRemovalCommitError {
    StorePersistence(anyhow::Error),
    DeliveryAbandonment,
    DeliveryRollback(anyhow::Error),
}

fn commit_removed_store_record_with<P, A>(
    store: &mut Store,
    removed: RemovedStoreRecord,
    mut persist_store: P,
    abandon_delivery: A,
) -> std::result::Result<RemovedStoreRecord, StoreRecordRemovalCommitError>
where
    P: FnMut(&Store) -> Result<()>,
    A: FnOnce(&str) -> Result<bool>,
{
    let recoverable_failed_delivery_id = removed
        .recoverable_failed_delivery_id()
        .map(ToOwned::to_owned);
    if let Err(error) = persist_store(store) {
        removed.restore(store);
        return Err(StoreRecordRemovalCommitError::StorePersistence(error));
    }
    if let Some(failed_asset_id) = recoverable_failed_delivery_id {
        if !matches!(abandon_delivery(&failed_asset_id), Ok(true)) {
            removed.restore(store);
            return match persist_store(store) {
                Ok(()) => Err(StoreRecordRemovalCommitError::DeliveryAbandonment),
                Err(error) => Err(StoreRecordRemovalCommitError::DeliveryRollback(error)),
            };
        }
    }
    Ok(removed)
}

fn asset_collection_mut<'a>(store: &'a mut Store, source: &str) -> &'a mut Vec<AssetData> {
    match source {
        "asset" => &mut store.assets,
        "inspiration" => &mut store.inspiration,
        _ => &mut store.generations,
    }
}

fn take_pending_store_record(
    store: &mut Store,
    state: &AppState,
    id: &str,
    source: &str,
) -> Option<RemovedStoreRecord> {
    if source == "reference" {
        let category = resolve_category(&state.get_asset_type().to_string(), "");
        let items = references_for_category_mut(&mut store.references, &category);
        let index = items.iter().position(|item| item.id == id)?;
        return Some(RemovedStoreRecord::Reference {
            category,
            index,
            item: items.remove(index),
        });
    }
    let items = asset_collection_mut(store, source);
    let index = items.iter().position(|item| item.id == id)?;
    Some(RemovedStoreRecord::Asset {
        source: source.to_string(),
        index,
        item: items.remove(index),
    })
}


struct CapturedDeleteCommit {
    id: String,
    source: String,
    removed: Option<RemovedStoreRecord>,
    retry: bool,
    local_delete_requested: bool,
}

enum CapturedDeleteWorkerResult {
    WriterRejected,
    Committed { file_retained: bool, cleanup_pending: bool },
}

fn start_captured_asset_delete(
    app: &AppWindow, context: AppContext, local_delete_requested: bool,
) {
    let Some(capture) = ViewerActionCapture::capture(&context) else { return; };
    let Ok(write) = capture.persistence.prepare_ordered_save() else { return; };
    let mut write = Some(write);
    let mut removed = None;
    let queued = capture.apply(&context, || {
        let state = app.global::<AppState>();
        let id = state.get_pending_delete_id().to_string();
        let source = state.get_pending_delete_source().to_string();
        if id.is_empty() { return None; }
        let mut store = context.store.borrow_mut();
        let record = take_pending_store_record(&mut store, &state, &id, &source);
        let retry = record.is_none();
        let failed_id = record.as_ref().and_then(RemovedStoreRecord::recoverable_failed_delivery_id)
            .map(str::to_owned)
            .or_else(|| (retry && source == "generation").then(|| id.clone()));
        let receiver = write.take().expect("single delete write")
            .enqueue(local_store_data(app, &store));
        removed = Some((id, source, record, failed_id, retry));
        Some(receiver)
    }).flatten();
    drop(write);
    let Some((id, source, record, failed_id, retry)) = removed else { return; };
    let receiver = match queued {
        Some(Ok(receiver)) => receiver,
        Some(Err(error)) => {
            drop(error);
            if let Some(record) = record {
                restore_captured_asset_delete(app, context, capture, id, source, record,
                    "删除记录未能安全保存，原记录已恢复");
            }
            return;
        }
        None => return,
    };
    let launched = spawn_delivery_preparation(&capture.persistence, move |captured, _, _| {
        match receiver.recv() {
            Ok(Err(_)) => return Ok(CapturedDeleteWorkerResult::WriterRejected),
            Err(_) => return Err(DeliveryRetryError::Local(anyhow!(
                "viewer delete Store acknowledgment disconnected"
            ))),
            Ok(Ok(())) => {}
        }
        let authority = match captured.storage_authority() {
            Ok(authority) => authority,
            Err(_) => return Ok(CapturedDeleteWorkerResult::Committed {
                file_retained: true, cleanup_pending: true,
            }),
        };
        if let Some(failed_id) = failed_id {
            let settled = recoverable_delivery_for_failed_asset_for_namespace(&authority, &failed_id)
                .and_then(|pair| match pair {
                    Some((generation, _)) => abandon_pending_delivery_for_namespace(
                        &authority, &generation.identity(), &failed_id,
                    ),
                    None if retry => Ok(true),
                    None => Ok(false),
                });
            if !matches!(settled, Ok(true)) {
                return Ok(CapturedDeleteWorkerResult::Committed {
                    file_retained: true, cleanup_pending: true,
                });
            }
        }
        Ok(CapturedDeleteWorkerResult::Committed { file_retained: false, cleanup_pending: false })
    });
    let staged = CapturedDeleteCommit {
        id, source, removed: record, retry, local_delete_requested,
    };
    match launched {
        Ok((cancel, receiver)) => poll_captured_asset_delete(
            app.as_weak(), context, capture, staged, cancel, receiver,
        ),
        Err(_) => {
            let _ = capture.apply(&context, || app.global::<AppState>()
                .set_viewer_message("删除结果未确认；原删除操作可安全重试".into()));
        }
    }
}

fn restore_captured_asset_delete(
    app: &AppWindow, context: AppContext, capture: ViewerActionCapture,
    id: String, source: String, removed: RemovedStoreRecord, message: &'static str,
) {
    let Ok(write) = capture.persistence.prepare_ordered_save() else { return; };
    let mut write = Some(write); let mut removed = Some(removed);
    let queued = capture.apply(&context, || {
        let state = app.global::<AppState>();
        let mut store = context.store.borrow_mut();
        if removed.as_ref().is_some_and(|record| record.conflicts_with_current(&store)) {
            state.set_viewer_message("删除目标已被新记录替换，未覆盖当前数据".into());
            return None;
        }
        removed.take().expect("single delete rollback").restore(&mut store);
        app.global::<AppState>().set_viewer_message(message.into());
        Some(write.take().expect("single rollback write").enqueue(local_store_data(app, &store)))
    }).flatten();
    drop(write); drop(removed);
    let Some(Ok(receiver)) = queued else { return; };
    if let Ok((cancel, receiver)) = spawn_delivery_preparation(&capture.persistence, move |_, _, _| {
        receiver.recv().map_err(|_| anyhow!("viewer delete rollback acknowledgment disconnected"))?
            .map_err(anyhow::Error::from)?;
        Ok(())
    }) {
        poll_captured_delete_rollback(app.as_weak(), context, capture, id, source, message, cancel, receiver);
    }
}

fn poll_captured_delete_rollback(
    weak: Weak<AppWindow>, context: AppContext, capture: ViewerActionCapture,
    id: String, source: String, message: &'static str, cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<(), DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => { poll_captured_delete_rollback(weak, context, capture, id, source, message, cancel, receiver); return; }
            Err(_) => return,
            Ok(false) => {}
        }
        if matches!(receiver.try_recv(), Ok(Ok(()))) {
            let mut visuals = Some(prepare_delivery_visuals(&app, &context.store.borrow()));
            let effects = capture.apply(&context, || {
                let state = app.global::<AppState>();
                if state.get_pending_delete_id() == id && state.get_pending_delete_source() == source {
                    state.set_viewer_message(message.into());
                }
                visuals.take().expect("single delete rollback projection")
                    .publish_metadata(&app, capture.persistence.clone())
            });
            drop(visuals);
            if let Some(effects) = effects { start_activation_visual_effects(&app, context, effects); }
        }
    });
}

fn poll_captured_asset_delete(
    weak: Weak<AppWindow>, context: AppContext, capture: ViewerActionCapture,
    staged: CapturedDeleteCommit, cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<std::result::Result<CapturedDeleteWorkerResult, DeliveryRetryError>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let Some(app) = weak.upgrade() else { cancel.store(true, Ordering::SeqCst); return; };
        match finish_delivery_preparation(&cancel) {
            Ok(true) => { poll_captured_asset_delete(weak, context, capture, staged, cancel, receiver); return; }
            Err(_) => {
                let _ = capture.apply(&context, || app.global::<AppState>()
                    .set_viewer_message("删除结果未确认；原删除操作可安全重试".into()));
                return;
            }
            Ok(false) => {}
        }
        let (file_retained, cleanup_pending) = match receiver.try_recv() {
            Ok(Ok(CapturedDeleteWorkerResult::WriterRejected)) => {
                if let Some(removed) = staged.removed {
                    restore_captured_asset_delete(&app, context, capture, staged.id, staged.source,
                        removed, "删除记录未能保存，原记录已恢复");
                } else {
                    let _ = capture.apply(&context, || app.global::<AppState>()
                        .set_viewer_message("删除记录仍未确认；请重试原删除操作".into()));
                }
                return;
            }
            Ok(Ok(CapturedDeleteWorkerResult::Committed { file_retained, cleanup_pending })) =>
                (file_retained, cleanup_pending),
            _ => {
                let _ = capture.apply(&context, || app.global::<AppState>()
                    .set_viewer_message("删除结果未确认；原删除操作可安全重试".into()));
                return;
            }
        };
        let visuals = prepare_delivery_visuals(&app, &context.store.borrow());
        let mut visuals = Some(visuals);
        let effects = capture.apply(&context, || {
            let state = app.global::<AppState>();
            // A newer dialog must not prevent publishing this committed deletion.
            if state.get_pending_delete_id() == staged.id && state.get_pending_delete_source() == staged.source {
                state.set_pending_delete_kind("".into());
                state.set_pending_delete_id("".into()); state.set_pending_delete_source("".into());
                state.set_pending_delete_can_remove_file(false); state.set_delete_confirm_open(false);
            }
            if state.get_viewer_id() == staged.id && state.get_viewer_source() == staged.source {
                state.set_viewer_open(false); state.set_viewer_image(Image::default()); state.set_viewer_source_path("".into());
            }
            if cleanup_pending {
                state.set_viewer_message("记录已删除；恢复记录清理待重试，本机图片文件已保留".into());
            } else if staged.local_delete_requested || file_retained {
                state.set_viewer_message("记录已删除，本机图片文件已保留".into());
            }
            Some(visuals.take().expect("single delete projection").publish_metadata(&app, capture.persistence.clone()))
        }).flatten();
        drop(visuals);
        if let Some(effects) = effects { start_activation_visual_effects(&app, context, effects); }
    });
}
fn confirm_pending_asset_delete(
    app: &AppWindow,
    context: &AppContext,
    store: &Rc<RefCell<Store>>,
    delete_local_file: bool,
) {
    let state = app.global::<AppState>();
    let id = state.get_pending_delete_id().to_string();
    let source = state.get_pending_delete_source().to_string();
    let removal_result = {
        let mut store_mut = store.borrow_mut();
        let Some(removed) = take_pending_store_record(&mut store_mut, &state, &id, &source) else {
            return;
        };
        let result = commit_removed_store_record_with(
            &mut store_mut,
            removed,
            |store| save_local_store_checked(app, store),
            |failed_asset_id| {
                let Some(scope) = current_generation_session_scope(context) else {
                    return Ok(false);
                };
                abandon_pending_delivery(
                    &scope.owner_user_id,
                    scope.auth_epoch,
                    failed_asset_id,
                )
            },
        );
        rebuild_storage_references(&store_mut);
        result.map(|removed| {
            let shared = store_references_path(&store_mut, Path::new(removed.source_path()));
            (removed, shared)
        })
    };
    let (removed, shared_in_store) = match removal_result {
        Ok(value) => value,
        Err(error) => {
            state.set_viewer_message(
                match error {
                    StoreRecordRemovalCommitError::StorePersistence(error) => {
                        format!("删除记录失败：{error}")
                    }
                    StoreRecordRemovalCommitError::DeliveryAbandonment => {
                        "删除失败：无法更新本地生成恢复记录，失败图片已保留".to_string()
                    }
                    StoreRecordRemovalCommitError::DeliveryRollback(error) => format!(
                        "删除失败：无法更新本地生成恢复记录，且恢复失败图片写入失败：{error}"
                    ),
                }
                .into(),
            );
            push_all(app, &store.borrow());
            return;
        }
    };

    let mut removed = Some(removed);
    if delete_local_file {
        let path_text = removed
            .as_ref()
            .map(|record| record.source_path().to_string())
            .unwrap_or_default();
        if let Some(path) = managed_output_path(&path_text) {
            let protected = shared_in_store
                || path_has_live_ui_reference(&state, &path)
                || pending_recovery_may_reference_files()
                || indexed_reference_count(&path) > 0;
            if !protected {
                invalidate_previews_for_source(&path);
                match fs::remove_file(&path) {
                    Ok(()) => remove_indexed_file(&path),
                    Err(error) => {
                        let mut store_mut = store.borrow_mut();
                        if let Some(record) = removed.take() {
                            record.restore(&mut store_mut);
                        }
                        let restore_result = save_local_store_checked(app, &store_mut);
                        rebuild_storage_references(&store_mut);
                        drop(store_mut);
                        state.set_viewer_message(
                            match restore_result {
                                Ok(()) => format!("本地文件删除失败，记录已保留：{error}"),
                                Err(save_error) => format!(
                                    "本地文件删除失败，且恢复记录写入失败：{error}；{save_error}"
                                ),
                            }
                            .into(),
                        );
                        state.set_delete_confirm_open(false);
                        push_all(app, &store.borrow());
                        return;
                    }
                }
            } else {
                state.set_viewer_message("图片仍被其他记录或未完成任务使用，本地文件已保留".into());
            }
        }
    }
    // Drop the removed record only after all path information has been consumed.
    drop(removed);
    state.set_pending_delete_id("".into());
    state.set_pending_delete_source("".into());
    state.set_pending_delete_can_remove_file(false);
    state.set_delete_confirm_open(false);
    state.set_viewer_open(false);
    state.set_viewer_image(Image::default());
    state.set_viewer_source_path("".into());
    push_all(app, &store.borrow());
}

fn configure_image_editor_model(state: &AppState) {
    let preferred = state.get_image_model().to_string();
    let selected = state
        .get_catalog_models()
        .iter()
        .filter(|model| model.purpose == "image_generation" && model.supports_image_edit)
        .find(|model| model.code.as_str() == preferred)
        .or_else(|| {
            state
                .get_catalog_models()
                .iter()
                .find(|model| model.purpose == "image_generation" && model.supports_image_edit)
        });
    let Some(model) = selected else {
        state.set_image_editor_model("".into());
        state.set_image_editor_model_name("".into());
        state.set_image_editor_price_1k(0);
        state.set_image_editor_price_2k(0);
        state.set_image_editor_price_4k(0);
        return;
    };
    let mut quality = match state.get_viewer_quality().to_ascii_uppercase().as_str() {
        "4K" => "4K",
        "2K" => "2K",
        "1K" => "1K",
        _ if state
            .get_image_editor_source_width()
            .max(state.get_image_editor_source_height())
            > 2048 =>
        {
            "4K"
        }
        _ if state
            .get_image_editor_source_width()
            .max(state.get_image_editor_source_height())
            > 1024 =>
        {
            "2K"
        }
        _ => "1K",
    };
    let quality_price = |value: &str| match value {
        "4K" => model.price_4k,
        "2K" => model.price_2k,
        _ => model.price_1k,
    };
    if quality_price(quality) <= 0 {
        quality = ["1K", "2K", "4K"]
            .into_iter()
            .find(|candidate| quality_price(candidate) > 0)
            .unwrap_or(quality);
    }
    state.set_image_editor_model(model.code);
    state.set_image_editor_model_name(model.name);
    state.set_image_editor_quality(quality.into());
    state.set_image_editor_price_1k(model.price_1k);
    state.set_image_editor_price_2k(model.price_2k);
    state.set_image_editor_price_4k(model.price_4k);
}

fn current_viewer_source_path(state: &AppState) -> Result<PathBuf> {
    let source_path = PathBuf::from(state.get_viewer_source_path().to_string());
    if source_path.is_file() {
        return Ok(source_path);
    }
    persist_slint_reference(&state.get_viewer_image())
}

fn add_viewer_reference_to_creation_workspace(
    store: &mut Store,
    current_prompt: &str,
    workflow_id: &str,
    source_path: &Path,
) -> Result<String> {
    let target_workspace_id = normalize_canvas_workspace_id(workflow_id);
    let active_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    let target_reference_count = if active_workspace_id == target_workspace_id {
        store.canvas_references.len()
    } else {
        store
            .canvas_workspaces
            .get(&target_workspace_id)
            .map(|workspace| workspace.references.len())
            .unwrap_or(0)
    };
    if target_reference_count >= MAX_REFERENCE_IMAGES {
        return Err(anyhow!(reference_limit_message(MAX_REFERENCE_IMAGES)));
    }

    let prompt = switch_canvas_workspace(store, current_prompt, &target_workspace_id);
    store.canvas_references.push(ReferenceData {
        id: Uuid::new_v4().to_string(),
        source_path: source_path.display().to_string(),
    });
    Ok(prompt)
}

fn prepare_image_edit_inputs(app: &AppWindow, points: &[BrushPoint]) -> Result<(PathBuf, PathBuf)> {
    const MAX_UPLOAD_BYTES: usize = 7_500_000;
    const MAX_EDGE: u32 = 4096;
    let state = app.global::<AppState>();
    let original_path = PathBuf::from(state.get_image_editor_source_path().to_string());
    let mut source = if original_path.is_file() {
        decode_image_file(&original_path)?.0.to_rgba8()
    } else {
        let buffer = state
            .get_image_editor_image()
            .to_rgba8()
            .ok_or_else(|| anyhow!("无法读取原图像素"))?;
        image::RgbaImage::from_raw(
            buffer.width(),
            buffer.height(),
            buffer.as_bytes().to_vec(),
        )
        .ok_or_else(|| anyhow!("原图像素格式无效"))?
    };
    if source.width() == 0 || source.height() == 0 {
        return Err(anyhow!("原图尺寸无效"));
    }
    if source.width().max(source.height()) > MAX_EDGE {
        source = image::DynamicImage::ImageRgba8(source)
            .resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Lanczos3)
            .to_rgba8();
    }

    let mut source_bytes = encode_png_rgba(&source, source.width(), source.height())?;
    while source_bytes.len() > MAX_UPLOAD_BYTES && source.width().max(source.height()) > 1024 {
        let width = ((source.width() as f32 * 0.82).round() as u32).max(1);
        let height = ((source.height() as f32 * 0.82).round() as u32).max(1);
        source = image::imageops::resize(
            &source,
            width,
            height,
            image::imageops::FilterType::Lanczos3,
        );
        source_bytes = encode_png_rgba(&source, source.width(), source.height())?;
    }
    if source_bytes.len() > MAX_UPLOAD_BYTES {
        return Err(anyhow!("原图文件过大，无法在不破坏遮罩尺寸的情况下上传"));
    }

    let mask = rasterize_image_edit_mask(points, source.width(), source.height())?;
    let mask_bytes = encode_png_rgba(&mask, mask.width(), mask.height())?;
    let directory = configured_output_directory().join("image-edit-inputs");
    if !ensure_managed_subdirectory(&directory) {
        return Err(anyhow!("无法创建安全的图片编辑暂存目录"));
    }
    let stem = Local::now().format("%Y%m%d%H%M%S%3f");
    let source_path = unique_path(directory.join(format!("{stem}-source.png")));
    let mask_path = unique_path(directory.join(format!("{stem}-mask.png")));
    atomic_write_file(&source_path, &source_bytes)?;
    if let Err(error) = atomic_write_file(&mask_path, &mask_bytes) {
        let _ = fs::remove_file(&source_path);
        return Err(error);
    }
    Ok((source_path, mask_path))
}

pub(super) fn rasterize_image_edit_mask(
    points: &[BrushPoint],
    width: u32,
    height: u32,
) -> Result<image::RgbaImage> {
    if width == 0 || height == 0 {
        return Err(anyhow!("遮罩尺寸无效"));
    }
    let mut mask = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));
    for point in points {
        let center_x = point.x.clamp(0.0, 1.0) * width.saturating_sub(1) as f32;
        let center_y = point.y.clamp(0.0, 1.0) * height.saturating_sub(1) as f32;
        let radius = (point.size.clamp(0.002, 0.5) * width as f32 / 2.0).max(0.5);
        let left = (center_x - radius).floor().max(0.0) as u32;
        let right = (center_x + radius)
            .ceil()
            .min(width.saturating_sub(1) as f32) as u32;
        let top = (center_y - radius).floor().max(0.0) as u32;
        let bottom = (center_y + radius)
            .ceil()
            .min(height.saturating_sub(1) as f32) as u32;
        for y in top..=bottom {
            for x in left..=right {
                let inside = if point.shape.as_str() == "square" {
                    true
                } else {
                    let dx = x as f32 - center_x;
                    let dy = y as f32 - center_y;
                    dx * dx + dy * dy <= radius * radius
                };
                if inside {
                    mask.put_pixel(x, y, image::Rgba([255, 255, 255, 0]));
                }
            }
        }
    }
    Ok(mask)
}

fn append_brush_segment(
    model: &VecModel<BrushPoint>,
    from: Option<(f32, f32, f32)>,
    to: (f32, f32, f32),
    aspect: f32,
    shape: &str,
    color: slint::Color,
) {
    const MAX_BRUSH_POINTS: usize = 25_000;
    if model.row_count() >= MAX_BRUSH_POINTS {
        return;
    }

    for point in interpolated_brush_points(from, to, aspect, shape, color)
        .into_iter()
        .take(MAX_BRUSH_POINTS - model.row_count())
    {
        model.push(point);
    }
}

fn interpolated_brush_points(
    from: Option<(f32, f32, f32)>,
    to: (f32, f32, f32),
    aspect: f32,
    shape: &str,
    color: slint::Color,
) -> Vec<BrushPoint> {
    let shape = if shape == "square" {
        "square"
    } else {
        "circle"
    };
    let clamp_point = |(x, y, size): (f32, f32, f32)| {
        (
            if x.is_finite() {
                x.clamp(0.0, 1.0)
            } else {
                0.0
            },
            if y.is_finite() {
                y.clamp(0.0, 1.0)
            } else {
                0.0
            },
            if size.is_finite() {
                size.clamp(0.002, 0.5)
            } else {
                0.02
            },
        )
    };
    let to = clamp_point(to);
    let Some(from) = from.map(clamp_point) else {
        return vec![BrushPoint {
            x: to.0,
            y: to.1,
            size: to.2,
            shape: shape.into(),
            color,
        }];
    };

    let safe_aspect = if aspect.is_finite() {
        aspect.clamp(0.05, 20.0)
    } else {
        1.0
    };
    let dx = to.0 - from.0;
    let dy = (to.1 - from.1) / safe_aspect;
    let distance = (dx * dx + dy * dy).sqrt();
    let spacing = (to.2 * 0.32).max(0.0005);
    let steps = ((distance / spacing).ceil() as usize).clamp(1, 512);

    (1..=steps)
        .map(|index| {
            let progress = index as f32 / steps as f32;
            BrushPoint {
                x: from.0 + (to.0 - from.0) * progress,
                y: from.1 + (to.1 - from.1) * progress,
                size: from.2 + (to.2 - from.2) * progress,
                shape: shape.into(),
                color,
            }
        })
        .collect()
}

#[cfg(test)]
mod image_editor_tests {
    use super::*;

    #[test]
    fn brush_segments_are_interpolated_without_large_gaps() {
        let color = slint::Color::from_rgb_u8(36, 184, 255);
        let points = interpolated_brush_points(
            Some((0.1, 0.2, 0.02)),
            (0.9, 0.2, 0.02),
            1.0,
            "square",
            color,
        );

        assert!(points.len() > 20);
        assert!((points.last().unwrap().x - 0.9).abs() < f32::EPSILON);
        assert!(points.iter().all(|point| {
            (0.0..=1.0).contains(&point.x)
                && (0.0..=1.0).contains(&point.y)
                && point.size > 0.0
                && point.shape == "square"
                && point.color == color
        }));
    }

    #[test]
    fn unknown_brush_shape_falls_back_to_circle() {
        let color = slint::Color::from_rgb_u8(255, 77, 79);
        let points = interpolated_brush_points(None, (0.5, 0.5, 0.02), 1.0, "triangle", color);

        assert_eq!(points.len(), 1);
        assert_eq!(points[0].shape, "circle");
        assert_eq!(points[0].color, color);
    }

    #[test]
    fn image_edit_mask_makes_only_painted_pixels_transparent() {
        let point = BrushPoint {
            x: 0.5,
            y: 0.5,
            size: 0.4,
            shape: "circle".into(),
            color: slint::Color::from_rgb_u8(255, 0, 0),
        };
        let mask = rasterize_image_edit_mask(&[point], 20, 10).expect("mask");

        assert_eq!(mask.dimensions(), (20, 10));
        assert_eq!(mask.get_pixel(10, 5).0[3], 0);
        assert_eq!(mask.get_pixel(0, 0).0[3], 255);
        assert_eq!(mask.get_pixel(19, 9).0[3], 255);
    }

    #[test]
    fn square_image_edit_mask_preserves_source_dimensions() {
        let point = BrushPoint {
            x: 0.0,
            y: 0.0,
            size: 0.2,
            shape: "square".into(),
            color: slint::Color::from_rgb_u8(0, 0, 0),
        };
        let mask = rasterize_image_edit_mask(&[point], 40, 30).expect("mask");

        assert_eq!(mask.dimensions(), (40, 30));
        assert_eq!(mask.get_pixel(0, 0).0[3], 0);
        assert_eq!(mask.get_pixel(39, 29).0[3], 255);
    }
}

#[cfg(test)]
mod recoverable_card_delete_tests {
    use super::*;

    fn recoverable_card() -> AssetData {
        AssetData {
            id: "failed-card".to_string(),
            conversation_id: "conversation".to_string(),
            title: "Recoverable delivery".to_string(),
            category: "scene".to_string(),
            kind: "generate".to_string(),
            time: "2026-08-27 00:00:00".to_string(),
            prompt: "prompt".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model: "model".to_string(),
            origin: "backend".to_string(),
            width: 0,
            height: 0,
            source_path: "failed".to_string(),
            reference_paths: Vec::new(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable: true,
            delivery_downloading: false,
        }
    }

    fn take_recoverable_card(store: &mut Store) -> RemovedStoreRecord {
        RemovedStoreRecord::Asset {
            source: "generation".to_string(),
            index: 0,
            item: store.generations.remove(0),
        }
    }

    #[test]
    fn recoverable_card_delete_abandonment_failure_restores_memory_and_durable_store_without_ack()
    {
        let mut store = Store::default();
        store.generations.push(recoverable_card());
        let removed = take_recoverable_card(&mut store);
        let events = RefCell::new(Vec::new());
        let durable_ids = RefCell::new(vec!["failed-card".to_string()]);

        let result = commit_removed_store_record_with(
            &mut store,
            removed,
            |store| {
                let ids = store
                    .generations
                    .iter()
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>();
                events.borrow_mut().push(if ids.is_empty() {
                    "persist-card-removal"
                } else {
                    "persist-card-rollback"
                });
                *durable_ids.borrow_mut() = ids;
                Ok(())
            },
            |failed_asset_id| {
                assert_eq!(failed_asset_id, "failed-card");
                events.borrow_mut().push("abandon-delivery");
                Err(anyhow!("recovery persistence failed"))
            },
        );

        assert!(matches!(
            result,
            Err(StoreRecordRemovalCommitError::DeliveryAbandonment)
        ));
        assert_eq!(store.generations.len(), 1);
        assert_eq!(store.generations[0].id, "failed-card");
        assert_eq!(store.generations[0].title, "Recoverable delivery");
        assert_eq!(durable_ids.into_inner(), vec!["failed-card"]);
        assert_eq!(
            events.into_inner(),
            vec![
                "persist-card-removal",
                "abandon-delivery",
                "persist-card-rollback",
            ]
        );
    }

    #[test]
    fn recoverable_card_delete_store_failure_restores_card_before_abandonment() {
        let mut store = Store::default();
        store.generations.push(recoverable_card());
        let removed = take_recoverable_card(&mut store);
        let events = RefCell::new(Vec::new());

        let result = commit_removed_store_record_with(
            &mut store,
            removed,
            |_| {
                events.borrow_mut().push("persist-card-removal");
                Err(anyhow!("local store persistence failed"))
            },
            |_| {
                events.borrow_mut().push("abandon-delivery");
                Ok(true)
            },
        );

        assert!(matches!(
            result,
            Err(StoreRecordRemovalCommitError::StorePersistence(_))
        ));
        assert_eq!(store.generations.len(), 1);
        assert_eq!(store.generations[0].id, "failed-card");
        assert_eq!(events.into_inner(), vec!["persist-card-removal"]);
    }
}

#[cfg(test)]
mod actual_viewer_callback_tests {
    use super::*;
    const OWNER: &str = "11111111-1111-4111-8111-111111111111";
    const GROUP: &str = "22222222-2222-4222-8222-222222222222";

    struct Fixture(video_image_callbacks::tests::scoped_inputs::Fixture);
    impl std::ops::Deref for Fixture {
        type Target = video_image_callbacks::tests::scoped_inputs::Fixture;
        fn deref(&self) -> &Self::Target { &self.0 }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let workers = drain_delivery_commit_workers_for_lease_for_test(self.persistence.lease());
            let previews = drain_activation_preview_workers_for_lease_for_test(self.persistence.lease());
            let retired = self.context.user_activity.begin_quiesce(self.persistence.lease()).map(|guard| guard.retire());
            if !std::thread::panicking() { workers.unwrap(); previews.unwrap(); retired.unwrap(); }
        }
    }

    fn failed_card(id: &str, recoverable: bool) -> AssetData {
        AssetData {
            id: id.into(), conversation_id: "conversation".into(), title: "Failed card".into(),
            category: "scene".into(), kind: "generate".into(), time: "2026-09-08 00:00".into(),
            prompt: "retained prompt".into(), ratio: "1:1".into(), quality: "1K".into(),
            model: "retained-model".into(), origin: "backend".into(), width: 0, height: 0,
            source_path: "failed".into(), reference_paths: vec![], cutout_done: false,
            remove_black_done: false, upscale_done: false, is_new: false,
            delivery_recoverable: recoverable, delivery_downloading: false,
        }
    }

    fn pending_failed_delivery(scope: &BillingScope, id: &str) -> PendingGenerationRecord {
        PendingGenerationRecord {
            source_asset_id: String::new(), video_request: None, schema_version: 2,
            cancel_requested: false, created_at_epoch_ms: Local::now().timestamp_millis(),
            client_request_id: "viewer-delete-request".into(), owner_user_id: scope.request.session.owner_user_id.clone(),
            billing_account_group_id: scope.request.account_group_id.clone(), auth_epoch: scope.request.session.auth_epoch,
            local_task_id: "viewer-delete-local".into(), server_task_id: "viewer-delete-server".into(),
            raw_prompt: "retained prompt".into(), generation_prompt: "retained prompt".into(),
            task_type: "image_generation".into(), category: "scene".into(), mode: "game".into(),
            ratio: "1:1".into(), quality: "1K".into(), model_code: "retained-model".into(),
            conversation_id: "conversation".into(), count: 1, target_width: 0, target_height: 0,
            create_conversation: false, reference_paths: vec![], reference_sha256: vec![],
            reference_size_bytes: vec![], lineage_reference_paths: vec![], uploaded_file_ids: vec![],
            deliveries: vec![PendingDeliveryRecord {
                item_index: 0, file_id: "viewer-delete-file".into(), sha256: "retained-sha".into(),
                size_bytes: 17, local_path: String::new(), acknowledged: false,
                failed_asset_id: id.into(), abandoned: false,
            }],
            terminal: true, expected_success_count: 1, canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }

    fn publish_group(fixture: &Fixture) -> BillingScope {
        let manager = &fixture.context.billing_context;
        let session = fixture.context.current_account_session_scope().unwrap();
        assert_eq!(session.owner_user_id, OWNER);
        manager.bind_authenticated_session(session.clone()).unwrap();
        let ticket = manager.begin_switch(
            &session, "viewer-device", GROUP, PreviousBillingAuthority::StillValid,
        ).unwrap();
        let snapshot: AccountSnapshot = serde_json::from_value(serde_json::json!({
            "user":{"id":OWNER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-08T00:00:00Z"},
            "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":GROUP,"name":"viewer fixture","group_status":"active","role":"owner","member_id":null,
                "relationship_status":null,"readable_context":true,"selectable":true,"group_version":"1",
                "membership_version":null,"capabilities":["bill"],"quota":null}
        })).unwrap();
        let staged = manager.stage_confirmation(&ticket, snapshot.billing_group.clone(), snapshot).unwrap();
        fixture.writer.save_selected_group(OWNER, "viewer-device", GROUP).unwrap();
        manager.publish_persisted(ticket, staged);
        manager.confirmed_scope().unwrap()
    }

    fn setup() -> (Fixture, AppWindow) {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = Fixture(video_image_callbacks::tests::scoped_inputs::Fixture::new());
        let transition = fixture.context.namespace_operations.try_begin_transition().unwrap();
        let recovery = transition.begin_prepublication_recovery(fixture.persistence.lease()).unwrap();
        recovery.verify_no_unsupported_imports(&fixture.authority).unwrap();
        let recovered = recovery.finish().unwrap();
        transition.prepare_publication(fixture.persistence.lease(), recovered).unwrap().publish();
        publish_group(&fixture);
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_page("generation".into()); state.set_logged_in(true); state.set_session_state("online".into());
        wire_viewer_callbacks(&app, fixture.context.clone());
        (fixture, app)
    }

    fn reset_private_ui(state: &AppState) {
        state.set_page("generation".into());
        state.set_viewer_open(true); state.set_viewer_id("failed-a".into());
        state.set_viewer_source("generation".into()); state.set_viewer_source_path("failed".into());
        state.set_viewer_message("original viewer".into()); state.set_cutout_open(false);
        state.set_upscale_open(false); state.set_delete_confirm_open(false);
    }

    #[test]
    fn actual_viewer_pure_in_app_callbacks_fail_closed_without_private_binding() {
        i_slint_backend_testing::init_no_event_loop();
        let context = AppContext::default();
        context.store.borrow_mut().generations = vec![failed_card("failed-a", false), failed_card("failed-b", false)];
        let app = AppWindow::new().unwrap();
        wire_viewer_callbacks(&app, context);
        let state = app.global::<AppState>();
        let mut mutations = Vec::new();

        reset_private_ui(&state); state.invoke_close_viewer();
        if !state.get_viewer_open() { mutations.push("close"); }
        reset_private_ui(&state); state.invoke_open_viewer("failed-b".into(), "generation".into());
        if state.get_viewer_id() != "failed-a" { mutations.push("open"); }
        reset_private_ui(&state); state.invoke_viewer_next();
        if state.get_viewer_id() != "failed-a" { mutations.push("next"); }
        reset_private_ui(&state); state.invoke_viewer_cutout_image();
        if !state.get_viewer_open() || state.get_cutout_open() { mutations.push("cutout"); }
        reset_private_ui(&state); state.set_viewer_open(false); state.set_cutout_open(true); state.invoke_close_cutout();
        if state.get_viewer_open() || !state.get_cutout_open() { mutations.push("close-cutout"); }
        reset_private_ui(&state); state.invoke_open_upscale_dialog();
        if state.get_upscale_open() { mutations.push("open-upscale"); }
        reset_private_ui(&state); state.set_upscale_open(true); state.invoke_close_upscale_dialog();
        if !state.get_upscale_open() { mutations.push("close-upscale"); }
        reset_private_ui(&state); state.invoke_viewer_edit();
        if !state.get_viewer_open() || state.get_page() != "generation" { mutations.push("edit"); }
        reset_private_ui(&state); state.invoke_request_delete_asset("failed-a".into());
        if state.get_delete_confirm_open() { mutations.push("delete-stage"); }
        assert!(mutations.is_empty(), "missing binding mutated private callbacks: {mutations:?}");
    }

    #[test]
    fn actual_viewer_pure_in_app_callbacks_fail_closed_after_exact_426() {
        let (fixture, app) = setup();
        fixture.context.store.borrow_mut().generations = vec![failed_card("failed-a", false)];
        let state = app.global::<AppState>();
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        let mut mutations = Vec::new();

        reset_private_ui(&state); state.invoke_close_viewer();
        if !state.get_viewer_open() { mutations.push("close"); }
        reset_private_ui(&state); state.invoke_viewer_cutout_image();
        if !state.get_viewer_open() || state.get_cutout_open() { mutations.push("cutout"); }
        reset_private_ui(&state); state.invoke_open_upscale_dialog();
        if state.get_upscale_open() { mutations.push("open-upscale"); }
        reset_private_ui(&state); state.invoke_viewer_edit();
        if !state.get_viewer_open() || state.get_page() != "generation" { mutations.push("edit"); }
        reset_private_ui(&state); state.invoke_request_delete_asset("failed-a".into());
        if state.get_delete_confirm_open() { mutations.push("delete-stage"); }
        assert!(mutations.is_empty(), "exact 426 mutated private callbacks: {mutations:?}");
    }

    #[test]
    fn actual_recoverable_failed_card_delete_removes_durable_store_and_exact_namespace_recovery() {
        let (fixture, app) = setup();
        let scope = fixture.context.billing_context.confirmed_scope().unwrap();
        let record = pending_failed_delivery(&scope, "failed-card");
        upsert_pending_generation_for_namespace(&fixture.authority, &scope, record).unwrap();
        fixture.context.store.borrow_mut().generations.push(failed_card("failed-card", true));
        fixture.persistence.save_store(local_store_data(&app, &fixture.context.store.borrow())).unwrap();
        let state = app.global::<AppState>(); state.set_viewer_source("generation".into());

        state.invoke_request_delete_asset("failed-card".into());
        assert!(state.get_delete_confirm_open()); state.invoke_confirm_delete();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            let memory_removed = !fixture.context.store.borrow().generations.iter().any(|row| row.id == "failed-card");
            let durable_removed = fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
                .ok().flatten().is_some_and(|saved| !saved.generations.iter().any(|row| row.id == "failed-card"));
            let recovery_settled = recoverable_delivery_for_failed_asset_for_namespace(&fixture.authority, "failed-card")
                .is_ok_and(|row| row.is_none());
            memory_removed && durable_removed && recovery_settled && !state.get_delete_confirm_open()
        });
        let durable = fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap();
        assert!(!durable.generations.iter().any(|row| row.id == "failed-card"));
        assert!(recoverable_delivery_for_failed_asset_for_namespace(&fixture.authority, "failed-card").unwrap().is_none());
        assert!(!state.get_delete_confirm_open());
    }

    #[test]
    fn actual_recoverable_failed_card_sqlite_failure_preserves_card_and_recovery() {
        let (fixture, app) = setup();
        let scope = fixture.context.billing_context.confirmed_scope().unwrap();
        let record = pending_failed_delivery(&scope, "failed-card");
        upsert_pending_generation_for_namespace(&fixture.authority, &scope, record).unwrap();
        fixture.context.store.borrow_mut().generations.push(failed_card("failed-card", true));
        fixture.persistence.save_store(local_store_data(&app, &fixture.context.store.borrow())).unwrap();
        let root = fixture.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql = rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_viewer_delete BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled viewer delete failure'); END;").unwrap();
        let state = app.global::<AppState>(); state.set_viewer_source("generation".into()); state.set_viewer_message("".into());

        state.invoke_request_delete_asset("failed-card".into()); state.invoke_confirm_delete();
        video_image_callbacks::tests::scoped_inputs::pump(|| !state.get_viewer_message().is_empty());
        assert!(fixture.context.store.borrow().generations.iter().any(|row| row.id == "failed-card"));
        let durable = fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap();
        assert!(durable.generations.iter().any(|row| row.id == "failed-card"));
        assert!(recoverable_delivery_for_failed_asset_for_namespace(&fixture.authority, "failed-card").unwrap().is_some());
        assert!(!state.get_viewer_message().contains("controlled viewer delete failure"),
            "SQLite details are internal and must not be exposed in private UI state");
        sql.execute_batch("DROP TRIGGER reject_viewer_delete").unwrap();
    }

    #[test]
    fn actual_viewer_remove_black_uses_owned_input_output_index_and_ordered_store_ack() {
        let (fixture, app) = setup();
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            24, 16, image::Rgba([48, 24, 12, 255]),
        ));
        let source = persist_reference_image_for_namespace(&fixture.authority, &image).unwrap();
        let mut original = failed_card("remove-black-source", false);
        original.source_path = source.to_string_lossy().into_owned();
        original.category = "scene".into(); original.width = 24; original.height = 16;
        fixture.context.store.borrow_mut().assets.push(original.clone());
        fixture.persistence.save_store(local_store_data(&app, &fixture.context.store.borrow())).unwrap();
        let state = app.global::<AppState>(); state.set_page("generation".into());
        state.set_viewer_open(true); state.set_viewer_source("asset".into());
        state.set_viewer_id(original.id.clone().into()); state.set_viewer_source_path(original.source_path.clone().into());
        state.set_viewer_title(original.title.clone().into()); state.set_viewer_category(original.category.clone().into());
        state.set_viewer_width(24); state.set_viewer_height(16); state.set_viewer_remove_black_done(false);

        state.invoke_viewer_remove_black();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            !state.get_viewer_processing()
                && fixture.context.store.borrow().assets.iter().any(|row| row.id != original.id && row.remove_black_done)
        });
        let result = fixture.context.store.borrow().assets.iter()
            .find(|row| row.id != original.id && row.remove_black_done).cloned().unwrap();
        let durable = fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap();
        assert!(durable.assets.iter().any(|row| row.id == result.id));
        assert!(durable.generations.iter().any(|row| row.id == result.id));
        let path = Path::new(&result.source_path);
        assert_eq!(path.parent(), Some(fixture.persistence.lease().namespace.path(ManagedUserArea::Output).as_path()));
        let indexed = fixture.authority.delivery_index().unwrap()
            .find_file_by_path_for_namespace(&fixture.authority, ManagedUserArea::Output,
                path.file_name().unwrap().to_str().unwrap()).unwrap();
        assert!(indexed.is_some(), "owned remove-black output was not indexed before Store acknowledgement");
    }


    fn owned_viewer(fixture: &Fixture, app: &AppWindow, id: &str) -> AssetData {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            32, 20, image::Rgba([44, 22, 11, 255]),
        ));
        let path = persist_reference_image_for_namespace(&fixture.authority, &image).unwrap();
        let mut item = failed_card(id, false);
        item.source_path = path.to_string_lossy().into_owned(); item.category = "scene".into();
        item.width = 32; item.height = 20;
        fixture.context.store.borrow_mut().assets.push(item.clone());
        fixture.persistence.save_store(local_store_data(app, &fixture.context.store.borrow())).unwrap();
        let state = app.global::<AppState>(); state.set_page("generation".into());
        state.set_asset_type("scene".into()); state.set_viewer_open(true); state.set_viewer_source("asset".into());
        state.set_viewer_id(item.id.clone().into()); state.set_viewer_source_path(item.source_path.clone().into());
        state.set_viewer_title(item.title.clone().into()); state.set_viewer_category(item.category.clone().into());
        state.set_viewer_width(32); state.set_viewer_height(20); state.set_viewer_remove_black_done(false);
        item
    }

    #[test]
    fn actual_remove_black_writer_failure_retries_current_store_without_duplicate_output() {
        let (fixture, app) = setup();
        let original = owned_viewer(&fixture, &app, "remove-black-retry");
        let root = fixture.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql = rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_viewer_remove_black BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled'); END;").unwrap();
        let state = app.global::<AppState>(); state.invoke_viewer_remove_black();
        video_image_callbacks::tests::scoped_inputs::pump(|| state.get_viewer_message().contains("暂存"));
        assert_eq!(fixture.context.store.borrow().assets.iter()
            .filter(|row| row.id != original.id && row.remove_black_done).count(), 1);
        fixture.context.store.borrow_mut().custom_prompts.push("later edit".into());
        sql.execute_batch("DROP TRIGGER reject_viewer_remove_black").unwrap();
        state.invoke_viewer_remove_black();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).ok().flatten()
                .is_some_and(|saved| saved.custom_prompts.iter().any(|row| row == "later edit")
                    && saved.assets.iter().filter(|row| row.id != original.id && row.remove_black_done).count() == 1)
        });
        assert_eq!(fixture.context.store.borrow().assets.iter()
            .filter(|row| row.id != original.id && row.remove_black_done).count(), 1);
    }

    #[test]
    fn actual_reference_writer_failure_retries_same_intent_without_duplicate_row() {
        let (fixture, app) = setup();
        let _ = owned_viewer(&fixture, &app, "reference-retry");
        let root = fixture.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql = rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_viewer_reference BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled'); END;").unwrap();
        let state = app.global::<AppState>(); state.invoke_viewer_use_reference();
        video_image_callbacks::tests::scoped_inputs::pump(|| state.get_viewer_message().contains("暂存"));
        assert_eq!(fixture.context.store.borrow().references.scene.len(), 1);
        fixture.context.store.borrow_mut().custom_prompts.push("later reference edit".into());
        sql.execute_batch("DROP TRIGGER reject_viewer_reference").unwrap();
        state.invoke_viewer_use_reference();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).ok().flatten()
                .is_some_and(|saved| saved.custom_prompts.iter().any(|row| row == "later reference edit")
                    && saved.references.scene.len() == 1)
        });
        assert_eq!(fixture.context.store.borrow().references.scene.len(), 1);
    }
    struct ReleaseWorkerAndJoin {
        release: Option<mpsc::Sender<()>>,
        transition: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for ReleaseWorkerAndJoin {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() { let _ = release.send(()); }
            if let Some(transition) = self.transition.take() { let _ = transition.join(); }
        }
    }

    fn trip_after_prepared_send() -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        set_delivery_preparation_after_send_for_test(move || {
            entered_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        (entered_rx, release_tx)
    }

    #[test]
    fn actual_image_edit_double_click_then_exact_426_never_starts_old_request() {
        let (fixture, app) = setup();
        let original = owned_viewer(&fixture, &app, "edit-late-426");
        let state = app.global::<AppState>(); state.invoke_viewer_open_image_editor();
        video_image_callbacks::tests::scoped_inputs::pump(|| state.get_page() == "image-editor");
        assert_eq!(state.get_image_editor_source_path().as_str(), original.source_path);
        state.set_image_editor_prompt("replace the marked area".into());
        state.set_image_editor_model("fixture-edit-model".into()); state.set_image_editor_quality("1K".into());
        state.set_image_editor_price_1k(1);
        assert_eq!(state.get_image_editor_estimated_credit_cost(), 1);
        state.invoke_begin_image_editor_stroke(0.5, 0.5, 0.1, 1.0, "circle".into(), slint::Color::from_rgb_u8(255, 0, 0));
        let (entered, release) = trip_after_prepared_send();
        state.invoke_submit_image_edit(); state.invoke_submit_image_edit();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let latch = fixture.persistence.upgrade_latch();
        let transition = std::thread::spawn(move || {
            latch.trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        });
        let mut guard = ReleaseWorkerAndJoin { release: Some(release), transition: Some(transition) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while fixture.persistence.upgrade_latch().snapshot().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fixture.persistence.upgrade_latch().snapshot().is_some());
        guard.release.take().unwrap().send(()).unwrap();
        guard.transition.take().unwrap().join().unwrap();
        drain_delivery_commit_workers_for_lease_for_test(fixture.persistence.lease()).unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
        assert!(load_pending_generations_for_namespace(&fixture.authority).unwrap().is_empty(),
            "late editor preparation dispatched billed work after exact 426");
    }

    #[test]
    fn actual_regeneration_failed_history_does_not_read_missing_output_before_exact_426() {
        let (fixture, app) = setup();
        let mut historical = failed_card("regenerate-failed-history", false);
        historical.reference_paths.clear(); historical.prompt = "retained regeneration request".into();
        fixture.context.store.borrow_mut().generations.push(historical.clone());
        fixture.persistence.save_store(local_store_data(&app, &fixture.context.store.borrow())).unwrap();
        let state = app.global::<AppState>(); state.set_viewer_open(true); state.set_viewer_source("generation".into());
        state.set_viewer_id(historical.id.clone().into()); state.set_viewer_source_path("failed".into());
        let (entered, release) = trip_after_prepared_send();
        state.invoke_viewer_regenerate();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let latch = fixture.persistence.upgrade_latch();
        let transition = std::thread::spawn(move || {
            latch.trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        });
        let mut guard = ReleaseWorkerAndJoin { release: Some(release), transition: Some(transition) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while fixture.persistence.upgrade_latch().snapshot().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fixture.persistence.upgrade_latch().snapshot().is_some());
        guard.release.take().unwrap().send(()).unwrap();
        guard.transition.take().unwrap().join().unwrap();
        drain_delivery_commit_workers_for_lease_for_test(fixture.persistence.lease()).unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
        assert!(load_pending_generations_for_namespace(&fixture.authority).unwrap().is_empty());
    }

    #[test]
    fn actual_local_file_delete_request_keeps_owned_file_after_metadata_commit() {
        let (fixture, app) = setup();
        let original = owned_viewer(&fixture, &app, "metadata-only-local-delete");
        let source = PathBuf::from(&original.source_path);
        let state = app.global::<AppState>();
        state.invoke_request_delete_asset(original.id.clone().into());
        assert!(!state.get_pending_delete_can_remove_file());
        state.invoke_confirm_delete_local_file();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).ok().flatten()
                .is_some_and(|saved| !saved.assets.iter().any(|row| row.id == original.id))
                && !state.get_delete_confirm_open()
        });
        assert!(source.is_file(), "deferred local deletion removed the owned source");
        assert!(state.get_viewer_message().contains("保留"));
    }

    #[test]
    fn actual_delete_writer_rejection_never_overwrites_same_id_replacement() {
        let (fixture, app) = setup();
        let original = owned_viewer(&fixture, &app, "delete-replacement");
        let root = fixture.persistence.lease().namespace.root().parent().unwrap().parent().unwrap();
        let sql = rusqlite::Connection::open(root.join("fixture.sqlite3")).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_viewer_delete_replacement BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT,'controlled'); END;").unwrap();
        let (entered, release) = trip_after_prepared_send();
        let mut worker = ReleaseWorkerAndJoin { release: Some(release), transition: None };
        let state = app.global::<AppState>();
        state.invoke_request_delete_asset(original.id.clone().into());
        state.invoke_confirm_delete();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        sql.execute_batch("DROP TRIGGER reject_viewer_delete_replacement").unwrap();
        let mut replacement = original.clone(); replacement.title = "later replacement".into();
        fixture.context.store.borrow_mut().assets.push(replacement.clone());
        fixture.persistence.save_store(local_store_data(&app, &fixture.context.store.borrow())).unwrap();
        worker.release.take().unwrap().send(()).unwrap();
        video_image_callbacks::tests::scoped_inputs::pump(|| state.get_viewer_message().contains("替换"));
        let current = fixture.context.store.borrow().assets.iter()
            .find(|row| row.id == replacement.id).cloned().unwrap();
        assert_eq!(current.title, "later replacement");
        let saved = fixture.writer.load_client_state_for_namespace(fixture.persistence.lease())
            .unwrap().unwrap();
        assert_eq!(saved.assets.iter().find(|row| row.id == replacement.id).unwrap().title,
            "later replacement");
    }

    #[test]
    fn actual_reference_stale_preparation_clears_only_its_ticket_and_allows_retry() {
        let (fixture, app) = setup();
        let original = owned_viewer(&fixture, &app, "reference-stale-preparation");
        let state = app.global::<AppState>();
        let (entered, release) = trip_after_prepared_send();
        let mut worker = ReleaseWorkerAndJoin { release: Some(release), transition: None };
        state.invoke_viewer_use_reference();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        state.set_viewer_id("new-viewer-target".into());
        worker.release.take().unwrap().send(()).unwrap();
        drain_delivery_commit_workers_for_lease_for_test(fixture.persistence.lease()).unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
        state.set_viewer_id(original.id.clone().into());
        state.set_viewer_source_path(original.source_path.clone().into());
        state.set_viewer_open(true);
        state.invoke_viewer_use_reference();
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).ok().flatten()
                .is_some_and(|saved| saved.references.scene.len() == 1)
        });
        assert_eq!(fixture.context.store.borrow().references.scene.len(), 1);
    }

    #[test]
    fn actual_image_edit_changed_request_releases_owned_busy_without_dispatch() {
        let (fixture, app) = setup();
        let original = owned_viewer(&fixture, &app, "edit-request-version");
        let state = app.global::<AppState>(); state.invoke_viewer_open_image_editor();
        video_image_callbacks::tests::scoped_inputs::pump(|| state.get_page() == "image-editor");
        assert_eq!(state.get_image_editor_source_path().as_str(), original.source_path);
        state.set_image_editor_prompt("first request".into());
        state.set_image_editor_model("fixture-edit-model".into()); state.set_image_editor_quality("1K".into());
        state.set_image_editor_price_1k(1);
        assert_eq!(state.get_image_editor_estimated_credit_cost(), 1);
        state.invoke_begin_image_editor_stroke(0.5, 0.5, 0.1, 1.0, "circle".into(),
            slint::Color::from_rgb_u8(255, 0, 0));
        let (entered, release) = trip_after_prepared_send();
        let mut worker = ReleaseWorkerAndJoin { release: Some(release), transition: None };
        state.invoke_submit_image_edit();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        state.set_image_editor_prompt("newer request retained".into());
        worker.release.take().unwrap().send(()).unwrap();
        video_image_callbacks::tests::scoped_inputs::pump(|| !state.get_image_editor_generating());
        assert_eq!(state.get_image_editor_prompt().as_str(), "newer request retained");
        assert!(load_pending_generations_for_namespace(&fixture.authority).unwrap().is_empty());
    }
}
pub(super) fn add_reference_from_drag_data(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    drag_data_to_paths(data)
        .into_iter()
        .fold(false, |added, path| {
            add_reference_from_path(app, store, &path) || added
        })
}

pub(super) fn drag_data_to_path(data: &str) -> Option<PathBuf> {
    drag_data_to_paths(data).into_iter().next()
}

pub(super) fn drag_data_to_paths(data: &str) -> Vec<PathBuf> {
    data.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(drag_line_to_path)
        .collect()
}

fn drag_line_to_path(raw: &str) -> Option<PathBuf> {
    if raw
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        let url = reqwest::Url::parse(raw).ok()?;
        if let Ok(path) = url.to_file_path() {
            return Some(path);
        }
        let decoded = percent_decode_path(url.path());
        #[cfg(windows)]
        let decoded = decoded.trim_start_matches('/').replace('/', "\\");
        return Some(PathBuf::from(decoded));
    }

    let decoded = percent_decode_path(raw);
    #[cfg(windows)]
    let decoded = decoded.trim_start_matches('/').replace('/', "\\");
    Some(PathBuf::from(decoded))
}

pub(super) fn external_image_url(data: &str) -> Option<String> {
    let html_source = data
        .split_once("src=\"")
        .and_then(|(_, tail)| tail.split_once('"').map(|(value, _)| value))
        .or_else(|| {
            data.split_once("src='")
                .and_then(|(_, tail)| tail.split_once('\'').map(|(value, _)| value))
        });
    let candidates = html_source.into_iter().chain(
        data.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| {
                line.strip_prefix("SourceURL:")
                    .map(str::trim)
                    .or_else(|| line.starts_with("http").then_some(line))
            }),
    );
    for candidate in candidates {
        let Ok(url) = reqwest::Url::parse(candidate) else {
            continue;
        };
        if matches!(url.scheme(), "http" | "https") && url.username().is_empty() {
            return Some(url.to_string());
        }
    }
    None
}

pub(super) fn file_uri_for_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "failed" {
        return String::new();
    }
    #[cfg(windows)]
    {
        let normalized = path.replace('\\', "/");
        let encoded = percent_encode_uri_path(&normalized);
        if encoded.starts_with("//") {
            format!("file:{encoded}")
        } else {
            format!("file:///{encoded}")
        }
    }
    #[cfg(not(windows))]
    {
        let encoded = percent_encode_uri_path(path);
        if encoded.starts_with('/') {
            format!("file://{encoded}")
        } else {
            format!("file:///{encoded}")
        }
    }
}

pub(super) fn percent_encode_uri_path(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                output.push(*byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

pub(super) fn percent_decode_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

pub(super) fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub(super) fn add_reference_from_path(app: &AppWindow, store: &Rc<RefCell<Store>>, path: &Path) -> bool {
    let Some(persistence) = store.borrow().private_persistence.clone() else { return false; };
    let state = app.global::<AppState>();
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    let canvas = state.get_page().as_str() == "canvas";
    add_reference_from_captured_path(app, store, path, &persistence, &category, canvas)
}

pub(super) fn add_reference_from_captured_path(app: &AppWindow, store: &Rc<RefCell<Store>>, path: &Path,
    persistence: &PrivatePersistence, category: &str, canvas: bool,
) -> bool {
    let Ok(_effect) = persistence.begin_effect() else { return false; };
    if !store.borrow().private_persistence.as_ref().is_some_and(|current| current.same_binding(persistence)) { return false; }
    let source = (|| {
        let authority = persistence.storage_authority()?;
        let image = decode_owned_reference_source(&authority, path)?;
        persist_reference_image_for_namespace(&authority, &image)
    })();
    match source {
        Ok(path) => append_owned_reference(app, store, path, persistence, category, canvas),
        Err(_) => { app.global::<AppState>().set_generation_status("参考图未能安全导入；原文件保持不变".into()); false },
    }
}

pub(super) fn append_owned_reference(app: &AppWindow, store: &Rc<RefCell<Store>>, path: PathBuf,
    persistence: &PrivatePersistence, category: &str, canvas: bool,
) -> bool {
    let Ok(_effect) = persistence.begin_effect() else { return false; };
    let Some(source_path) = path.to_str().map(str::to_owned) else { return false; };
    let mut store = store.borrow_mut();
    if !store.private_persistence.as_ref().is_some_and(|current| current.same_binding(persistence)) { return false; }
    let previous = references_for_context(&store, category, canvas).clone();
    let max_references = max_reference_images_for_category(category);
    if previous.len() >= max_references {
        app.global::<AppState>().set_generation_status(reference_limit_message(max_references).into()); return false;
    }
    references_for_context_mut(&mut store, category, canvas).push(ReferenceData { id: Uuid::new_v4().to_string(), source_path });
    if save_local_store_checked(app, &store).is_err() {
        *references_for_context_mut(&mut store, category, canvas) = previous;
        app.global::<AppState>().set_generation_status("参考图状态未能安全保存，请重试".into()); return false;
    }
    if canvas { push_canvas_references(app, &store); } else { push_references(app, &store); }
    app.global::<AppState>().set_generation_status("已添加参考图".into()); true
}
