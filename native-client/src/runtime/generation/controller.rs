use super::*;

pub(super) fn start_generation(
    app: &AppWindow,
    context: AppContext,
    override_prompt: Option<String>,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
    existing_generation_policy: ExistingGenerationPolicy,
) {
    start_generation_for_destination(
        app,
        context,
        override_prompt,
        create_conversation,
        retry_failed_id,
        forced_count,
        existing_generation_policy,
        GenerationDestination::Gallery,
    );
}

pub(super) fn start_canvas_generation(
    app: &AppWindow,
    context: AppContext,
    source_node_id: String,
    prompt: String,
) {
    start_generation_for_destination(
        app,
        context,
        Some(prompt),
        false,
        None,
        None,
        ExistingGenerationPolicy::StopExisting,
        GenerationDestination::Canvas { source_node_id },
    );
}

fn start_generation_for_destination(
    app: &AppWindow,
    context: AppContext,
    override_prompt: Option<String>,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
    existing_generation_policy: ExistingGenerationPolicy,
    destination: GenerationDestination,
) {
    let state = app.global::<AppState>();
    let visible_prompt = state.get_prompt().trim().to_string();
    let applied_chinese = state
        .get_deep_optimization_applied_chinese()
        .trim()
        .to_string();
    let applied_english = state
        .get_deep_optimization_applied_english()
        .trim()
        .to_string();
    let input_prompt =
        submitted_prompt_for_visible_prompt(&visible_prompt, &applied_chinese, &applied_english);
    let raw_prompt = if let Some(override_prompt) = override_prompt {
        override_prompt.trim().to_string()
    } else {
        let selected_prompts = {
            let store = context.store.borrow();
            selected_custom_prompt_replacements_for_category(
                &store,
                &current_workspace_category(app),
            )
        };
        compose_inline_custom_prompts(&input_prompt, &selected_prompts)
    };
    if raw_prompt.trim().is_empty() {
        state.set_generation_status("请输入生成需求".into());
        return;
    }
    if !require_online_operation(app, "生成图片") {
        return;
    }
    if context.backend.is_none() {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    }
    start_backend_generation(
        app,
        context,
        raw_prompt,
        create_conversation,
        retry_failed_id,
        forced_count,
        existing_generation_policy,
        destination,
    );
}

pub(super) fn start_asset_regeneration(
    app: &AppWindow,
    context: AppContext,
    item: AssetData,
) -> bool {
    if !restore_asset_regeneration_inputs(app, &context, &item) {
        return false;
    }
    let state = app.global::<AppState>();
    state.set_viewer_message("".into());
    state.set_viewer_open(false);
    start_generation(
        app,
        context,
        Some(item.prompt),
        false,
        None,
        None,
        ExistingGenerationPolicy::KeepExisting,
    );
    true
}

fn restore_asset_regeneration_inputs(
    app: &AppWindow,
    context: &AppContext,
    item: &AssetData,
) -> bool {
    let state = app.global::<AppState>();
    let category = resolve_category(&item.category, &item.prompt);
    let max_references = max_reference_images_for_category(&category);
    if item.reference_paths.len() > max_references {
        let message = reference_limit_message(max_references);
        state.set_viewer_message(message.into());
        state.set_generation_status(message.into());
        return false;
    }

    let mut references = Vec::with_capacity(item.reference_paths.len());
    for source_path in &item.reference_paths {
        let path = PathBuf::from(source_path);
        if !path.is_file() {
            let message = format!("原参考图已不存在，无法再次生成：{}", path.display());
            state.set_viewer_message(message.clone().into());
            state.set_generation_status(message.into());
            return false;
        }
        if load_preview_image(&path, PreviewPurpose::Reference).is_err() {
            let message = format!("原参考图无法读取，无法再次生成：{}", path.display());
            state.set_viewer_message(message.clone().into());
            state.set_generation_status(message.into());
            return false;
        }
        references.push(ReferenceData {
            id: Uuid::new_v4().to_string(),
            source_path: path.display().to_string(),
        });
    }

    state.set_asset_type(category.clone().into());
    if !item.ratio.trim().is_empty() {
        state.set_ratio(item.ratio.clone().into());
    }
    if !item.quality.trim().is_empty() {
        state.set_quality(item.quality.clone().into());
    }
    if !item.kind.trim().is_empty() {
        state.set_mode(item.kind.clone().into());
    }
    state.set_current_conversation_id(item.conversation_id.clone().into());
    {
        let mut store = context.store.borrow_mut();
        *references_for_category_mut(&mut store.references, &category) = references;
        push_references(app, &store);
        push_generations(app, &store);
    }
    sync_generation_state_for_current_category(context, app);
    true
}

fn submitted_prompt_for_visible_prompt(
    visible_prompt: &str,
    applied_chinese: &str,
    applied_english: &str,
) -> String {
    if !applied_english.trim().is_empty() && visible_prompt.trim() == applied_chinese.trim() {
        applied_english.trim().to_string()
    } else {
        visible_prompt.trim().to_string()
    }
}

#[cfg(test)]
mod deep_prompt_tests {
    use super::{
        insert_canvas_generated_asset, replace_canvas_generation_placeholder,
        submitted_prompt_for_visible_prompt, CanvasNoteData, Store,
    };

    #[test]
    fn applied_chinese_prompt_submits_its_matching_english_version() {
        assert_eq!(
            submitted_prompt_for_visible_prompt(
                "月下的锻造工坊",
                "月下的锻造工坊",
                "a moonlit forge workshop",
            ),
            "a moonlit forge workshop",
        );
    }

    #[test]
    fn editing_the_readable_prompt_invalidates_the_english_binding() {
        assert_eq!(
            submitted_prompt_for_visible_prompt(
                "月下的古老锻造工坊",
                "月下的锻造工坊",
                "a moonlit forge workshop",
            ),
            "月下的古老锻造工坊",
        );
    }

    #[test]
    fn composer_generation_replaces_its_loading_rectangle_in_place() {
        let mut notes = vec![CanvasNoteData {
            id: "loading-result".to_string(),
            kind: "image".to_string(),
            content: "tomato growth".to_string(),
            x: 100.0,
            y: 200.0,
            width: 340.0,
            height: 250.0,
            selected: true,
            ..CanvasNoteData::default()
        }];

        assert!(replace_canvas_generation_placeholder(
            &mut notes,
            "loading-result",
            "loading-result",
            "generated.png",
            1024.0,
            512.0,
            0,
        ));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].image_path, "generated.png");
        assert_eq!(notes[0].content, "");
        assert!((notes[0].width / notes[0].height - 2.0).abs() < 0.001);
        assert!((notes[0].width - 680.0).abs() < 0.001);
        assert!((notes[0].height - 340.0).abs() < 0.001);
        assert!((notes[0].x + notes[0].width / 2.0 - 270.0).abs() < 0.001);
        assert!((notes[0].y + notes[0].height / 2.0 - 325.0).abs() < 0.001);
        assert!(!notes[0].selected);
    }

    #[test]
    fn composer_generation_moves_result_away_from_an_existing_image() {
        let mut notes = vec![
            CanvasNoteData {
                id: "existing-image".to_string(),
                kind: "image".to_string(),
                image_path: "existing.png".to_string(),
                x: 100.0,
                y: 200.0,
                width: 340.0,
                height: 250.0,
                ..CanvasNoteData::default()
            },
            CanvasNoteData {
                id: "loading-result".to_string(),
                kind: "image".to_string(),
                content: "tomato growth".to_string(),
                x: 100.0,
                y: 200.0,
                width: 340.0,
                height: 250.0,
                selected: true,
                ..CanvasNoteData::default()
            },
        ];

        assert!(replace_canvas_generation_placeholder(
            &mut notes,
            "loading-result",
            "loading-result",
            "generated.png",
            680.0,
            500.0,
            0,
        ));

        let generated = notes
            .iter()
            .find(|note| note.id == "loading-result")
            .expect("generated image");
        let existing = notes
            .iter()
            .find(|note| note.id == "existing-image")
            .expect("existing image");
        assert!(
            generated.x >= existing.x + existing.width + 48.0
                || generated.x + generated.width + 48.0 <= existing.x
                || generated.y >= existing.y + existing.height + 48.0
                || generated.y + generated.height + 48.0 <= existing.y
        );
    }

    #[test]
    fn canvas_generation_is_added_to_other_assets_without_copying_its_file() {
        let mut store = Store::default();
        let references = vec!["reference.png".to_string()];

        insert_canvas_generated_asset(
            &mut store,
            "生成角色体型变化",
            "生成角色体型变化",
            "game",
            "2K",
            "openai_image",
            "generation",
            "canvas-conversation",
            "2026-09-02 12:30",
            "generated.png",
            &references,
            2048,
            1152,
            false,
        );

        assert_eq!(store.assets.len(), 1);
        assert!(store.generations.is_empty());
        let asset = &store.assets[0];
        assert_eq!(asset.category, "other");
        assert_eq!(asset.source_path, "generated.png");
        assert_eq!(asset.ratio, "16:9");
        assert_eq!(asset.reference_paths, references);
    }
}

pub(super) fn compose_inline_custom_prompts(
    input_prompt: &str,
    replacements: &[(String, String)],
) -> String {
    let mut composed = input_prompt.to_string();
    let mut missing = Vec::new();
    for (name, content) in replacements {
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        let display = inline_custom_prompt_display_text(name);
        if composed.contains(&display) {
            composed = composed.replacen(&display, content, 1);
        } else {
            missing.push(content.to_string());
        }
    }
    let composed = composed.trim();
    if composed.is_empty() || composed == "//" {
        return missing.join("\n\n");
    }
    if missing.is_empty() {
        composed.to_string()
    } else {
        missing.push(composed.to_string());
        missing.join("\n\n")
    }
}

pub(super) fn retry_failed_generation(app: &AppWindow, context: AppContext, id: String) {
    let store = context.store.clone();
    let item = {
        let store_ref = store.borrow();
        store_ref
            .generations
            .iter()
            .find(|item| item.id == id && item.source_path == "failed")
            .cloned()
    };
    let Some(item) = item else {
        app.global::<AppState>()
            .set_generation_status("未找到可重试的失败图片".into());
        return;
    };
    if item.prompt.trim().is_empty() {
        app.global::<AppState>()
            .set_generation_status("失败图片没有可重试的提示词".into());
        return;
    }
    if !restore_asset_regeneration_inputs(app, &context, &item) {
        return;
    }
    let state = app.global::<AppState>();
    state.set_count(1);
    state.set_prompt(item.prompt.clone().into());
    start_generation(
        app,
        context,
        Some(item.prompt),
        false,
        Some(item.id),
        Some(1),
        ExistingGenerationPolicy::KeepExisting,
    );
}

pub(super) fn stop_generation(app: &AppWindow, context: &AppContext) {
    let store = &context.store;
    let state = app.global::<AppState>();
    let category = current_workspace_category(app);
    let task_id = context
        .generations
        .active
        .borrow()
        .get(&category)
        .map(|task| task.task_id.clone());
    let Some(task_id) = task_id else {
        sync_generation_state_for_current_category(context, app);
        return;
    };
    let Some(task) = remove_active_generation(context, &category, &task_id) else {
        sync_generation_state_for_current_category(context, app);
        return;
    };
    discard_canvas_generation_placeholder(&state, &task.destination);
    refresh_delivery_download_flags(app, context);
    set_generation_status_for_category(context, app, &category, "已停止生成");
    sync_generation_state_for_current_category(context, app);
    if task.destination == GenerationDestination::Gallery {
        if !task.prompt.trim().is_empty() {
            state.set_prompt(task.prompt.clone().into());
        }
        finish_conversation_placeholder(&state, &task.conversation_id, None);
    }
    push_references(app, &store.borrow());
    if let Some(client_request_id) = task.client_request_id.as_ref() {
        if let Ok(mut cancellations) = context.cancelled_generation_requests.lock() {
            cancellations.insert(client_request_id.clone());
        }
    }
    if generation_scope_allows_polling(&app.as_weak(), context, &task.session_scope) {
        if let (Some(backend), Some(server_task_id)) =
            (context.backend.clone(), task.server_task_id)
        {
            let session_scope = task.session_scope;
            let worker_scope = session_scope.clone();
            let (sender, receiver) = mpsc::channel::<()>();
            std::thread::spawn(move || {
                let _ = GenerationApi::new(backend.api.clone())
                    .cancel_scoped(&server_task_id, &worker_scope);
                let _ = sender.send(());
            });
            observe_detached_generation_scope(
                app.as_weak(),
                context.clone(),
                session_scope,
                Rc::new(RefCell::new(Some(receiver))),
            );
        }
    }
}

pub(super) fn add_stream_success_item(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    raw_prompt: &str,
    category: &str,
    mode: &str,
    quality: &str,
    image_model: &str,
    origin: &str,
    conversation_id: &str,
    display_prompt: &str,
    time: &str,
    staged_path: &Path,
    reference_paths: &[String],
    upscale_done: bool,
) -> Result<(Image, String, String)> {
    let source_path = save_generated_file(app, staged_path, raw_prompt)?;
    let (width, height) = inspect_image_dimensions(Path::new(&source_path))?;
    let (width, height) = (width as i32, height as i32);
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        title: short_text(raw_prompt, 18),
        category: category.to_string(),
        kind: mode.to_string(),
        time: time.to_string(),
        prompt: display_generation_prompt(display_prompt),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality.to_string(),
        model: image_model.to_string(),
        origin: origin.to_string(),
        width,
        height,
        source_path: source_path.clone(),
        reference_paths: reference_paths.to_vec(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done,
        is_new: true,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let conversation_image =
        load_preview_image(Path::new(&source_path), PreviewPurpose::Reference)?;
    let generated_id = item.id.clone();
    let history_prompt = item.prompt.clone();
    let notification = NotificationData {
        id: Uuid::new_v4().to_string(),
        title: format!("Generation succeeded: {}", short_text(raw_prompt, 24)),
        model: image_model.to_string(),
        time: time.to_string(),
        reason: String::new(),
        success: true,
        read: false,
    };
    let mut store_mut = store.borrow_mut();
    persist_generated_asset_checked(
        app,
        &mut store_mut,
        item,
        notification,
        true,
        Some(&history_prompt),
    )?;
    push_all(app, &store_mut);
    Ok((conversation_image, source_path, generated_id))
}

pub(super) fn replace_failed_delivery_asset_checked(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    failed_asset_id: &str,
    staged_path: &Path,
    time: &str,
) -> Result<(String, String)> {
    let failed_asset = {
        let store = store.borrow();
        let mut matches = store.generations.iter().filter(|item| {
            item.id == failed_asset_id
                && item.source_path == "failed"
                && item.delivery_recoverable
        });
        let failed_asset = matches.next().cloned();
        if matches.next().is_some() {
            anyhow::bail!("failed delivery card is ambiguous");
        }
        failed_asset.ok_or_else(|| anyhow!("failed delivery card is missing"))?
    };
    let (width, height) = inspect_image_dimensions(staged_path)?;
    let source_path = save_generated_file(app, staged_path, &failed_asset.prompt)?;
    let completed_asset = AssetData {
        id: failed_asset.id.clone(),
        conversation_id: failed_asset.conversation_id,
        title: failed_asset.title,
        category: failed_asset.category,
        kind: failed_asset.kind,
        time: time.to_string(),
        prompt: failed_asset.prompt.clone(),
        ratio: ratio_from_actual_dimensions(width as i32, height as i32),
        quality: failed_asset.quality,
        model: failed_asset.model.clone(),
        origin: failed_asset.origin,
        width: width as i32,
        height: height as i32,
        source_path: source_path.clone(),
        reference_paths: failed_asset.reference_paths,
        cutout_done: failed_asset.cutout_done,
        remove_black_done: failed_asset.remove_black_done,
        upscale_done: failed_asset.upscale_done,
        is_new: true,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let notification = NotificationData {
        id: Uuid::new_v4().to_string(),
        title: format!(
            "图片下载完成：{}",
            short_text(&failed_asset.prompt, 24)
        ),
        model: failed_asset.model,
        time: time.to_string(),
        reason: String::new(),
        success: true,
        read: false,
    };
    let persisted = {
        let mut store = store.borrow_mut();
        replace_failed_delivery_asset_with(
            &mut store,
            failed_asset_id,
            completed_asset,
            notification,
            |pending| save_local_store_checked(app, pending),
        )
    };
    if let Err(error) = persisted {
        let _ = fs::remove_file(&source_path);
        return Err(error);
    }
    push_all(app, &store.borrow());
    Ok((source_path, failed_asset_id.to_string()))
}

pub(super) fn add_canvas_stream_success_item(
    app: &AppWindow,
    context: &AppContext,
    source_node_id: &str,
    raw_prompt: &str,
    bytes: &[u8],
    result_index: i32,
    total_count: i32,
    mode: &str,
    quality: &str,
    image_model: &str,
    origin: &str,
    conversation_id: &str,
    display_prompt: &str,
    time: &str,
    reference_paths: &[String],
    upscale_done: bool,
) -> Result<String> {
    let (bytes, _, width, height) = generated_image_from_bytes(bytes)?;
    let loading_node_id = app
        .global::<AppState>()
        .get_canvas_generation_loading_node_id()
        .to_string();
    let replaces_loading_placeholder = result_index == 0 && loading_node_id == source_node_id;
    let target_workspace_id = {
        let store = context.store.borrow();
        let active_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
        let target_workspace_id = canvas_workspace_id_for_source(&store, source_node_id)
            .ok_or_else(|| anyhow!("生成来源所在画板已不存在"))?;
        let (node_count, link_count) = if target_workspace_id == active_workspace_id {
            (store.canvas_notes.len(), store.canvas_links.len())
        } else {
            let workspace = store
                .canvas_workspaces
                .get(&target_workspace_id)
                .ok_or_else(|| anyhow!("生成来源所在画板已不存在"))?;
            (workspace.notes.len(), workspace.links.len())
        };
        if !replaces_loading_placeholder && (node_count >= 200 || link_count >= 400) {
            return Err(anyhow!("画布已达到容量上限"));
        }
        target_workspace_id
    };
    let source_path = save_generated_bytes(app, &bytes, raw_prompt)?;
    let mut store = context.store.borrow_mut();
    let active_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    let target_is_active = target_workspace_id == active_workspace_id;
    let (target_notes, target_links) = if target_is_active {
        let store = &mut *store;
        (&mut store.canvas_notes, &mut store.canvas_links)
    } else {
        let workspace = store
            .canvas_workspaces
            .get_mut(&target_workspace_id)
            .ok_or_else(|| anyhow!("生成来源所在画板已不存在"))?;
        (&mut workspace.notes, &mut workspace.links)
    };

    let source = target_notes
        .iter()
        .find(|note| note.id == source_node_id)
        .cloned()
        .ok_or_else(|| anyhow!("生成来源所在画板已不存在"))?;

    if target_is_active {
        context
            .canvas_history
            .borrow_mut()
            .record(CanvasSnapshot {
                notes: target_notes.clone(),
                links: target_links.clone(),
            });
    }

    if replace_canvas_generation_placeholder(
        target_notes,
        source_node_id,
        &loading_node_id,
        &source_path,
        width as f32,
        height as f32,
        result_index,
    ) {
        insert_canvas_generated_asset(
            &mut store,
            raw_prompt,
            display_prompt,
            mode,
            quality,
            image_model,
            origin,
            conversation_id,
            time,
            &source_path,
            reference_paths,
            width as i32,
            height as i32,
            upscale_done,
        );
        save_local_store(app, &store);
        let state = app.global::<AppState>();
        state.set_canvas_generation_loading_node_id("".into());
        if target_is_active {
            state.set_canvas_selected_id("".into());
            state.set_canvas_selected_count(0);
            push_canvas_notes(app, &store);
            state.set_canvas_can_undo(context.canvas_history.borrow().can_undo());
            state.set_canvas_can_redo(context.canvas_history.borrow().can_redo());
        }
        push_generations(app, &store);
        return Ok(source_path);
    }

    let mut result = CanvasNoteData {
        id: Uuid::new_v4().to_string(),
        kind: "image".to_string(),
        content: String::new(),
        image_path: source_path.clone(),
        width: 340.0,
        height: 250.0,
        parent_group_id: String::new(),
        z_index: target_notes
            .iter()
            .map(|note| note.z_index)
            .max()
            .unwrap_or(0)
            + 1,
        selected: false,
        ..CanvasNoteData::default()
    };
    fit_image_node_to_intrinsic_aspect(&mut result, width as f32, height as f32);
    let (x, y) = generated_canvas_result_position(
        Some(&source),
        result.width,
        result.height,
        result_index,
        total_count,
    );
    (result.x, result.y) = nearest_free_canvas_position(
        target_notes,
        x,
        y,
        result.width,
        result.height,
        None,
    );
    let result_id = result.id.clone();

    target_notes.push(result);
    let _ = connect_nodes(target_links, source_node_id, &result_id);
    insert_canvas_generated_asset(
        &mut store,
        raw_prompt,
        display_prompt,
        mode,
        quality,
        image_model,
        origin,
        conversation_id,
        time,
        &source_path,
        reference_paths,
        width as i32,
        height as i32,
        upscale_done,
    );
    save_local_store(app, &store);
    if target_is_active {
        push_canvas_notes(app, &store);
        let state = app.global::<AppState>();
        state.set_canvas_can_undo(context.canvas_history.borrow().can_undo());
        state.set_canvas_can_redo(context.canvas_history.borrow().can_redo());
    }
    push_generations(app, &store);
    Ok(source_path)
}

#[allow(clippy::too_many_arguments)]
fn insert_canvas_generated_asset(
    store: &mut Store,
    raw_prompt: &str,
    display_prompt: &str,
    mode: &str,
    quality: &str,
    image_model: &str,
    origin: &str,
    conversation_id: &str,
    time: &str,
    source_path: &str,
    reference_paths: &[String],
    width: i32,
    height: i32,
    upscale_done: bool,
) {
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        title: short_text(raw_prompt, 18),
        category: "other".to_string(),
        kind: mode.to_string(),
        time: time.to_string(),
        prompt: display_generation_prompt(display_prompt),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality.to_string(),
        model: image_model.to_string(),
        origin: origin.to_string(),
        width,
        height,
        source_path: source_path.to_string(),
        reference_paths: reference_paths.to_vec(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done,
        is_new: true,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    store.assets.insert(0, item);
}

fn replace_canvas_generation_placeholder(
    notes: &mut [CanvasNoteData],
    source_node_id: &str,
    loading_node_id: &str,
    source_path: &str,
    image_width: f32,
    image_height: f32,
    result_index: i32,
) -> bool {
    if result_index != 0 || loading_node_id != source_node_id {
        return false;
    }
    let Some(source_index) = notes.iter().position(|note| {
        note.id == source_node_id && note.kind == "image" && note.image_path.trim().is_empty()
    }) else {
        return false;
    };

    let (desired_x, desired_y, width, height, source_id) = {
        let source = &mut notes[source_index];
        let center_x = source.x + source.width / 2.0;
        let center_y = source.y + source.height / 2.0;
        source.content.clear();
        source.image_path = source_path.to_string();
        source.width = 340.0;
        source.height = 250.0;
        fit_image_node_to_intrinsic_aspect(source, image_width, image_height);
        source.selected = false;
        (
            center_x - source.width / 2.0,
            center_y - source.height / 2.0,
            source.width,
            source.height,
            source.id.clone(),
        )
    };
    let (x, y) = nearest_free_canvas_position(
        notes,
        desired_x,
        desired_y,
        width,
        height,
        Some(&source_id),
    );
    notes[source_index].x = x;
    notes[source_index].y = y;
    true
}

pub(super) fn discard_canvas_generation_placeholder(
    state: &AppState,
    destination: &GenerationDestination,
) {
    let GenerationDestination::Canvas { source_node_id } = destination else {
        return;
    };
    if state.get_canvas_generation_loading_node_id().as_str() != source_node_id {
        return;
    }
    state.set_canvas_generation_loading_node_id("".into());
    state.invoke_remove_canvas_node(source_node_id.clone().into());
}

pub(super) fn canvas_workspace_id_for_source(
    store: &Store,
    source_node_id: &str,
) -> Option<String> {
    let active_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    if store
        .canvas_notes
        .iter()
        .any(|note| note.id == source_node_id)
    {
        return Some(active_workspace_id.clone());
    }
    store
        .canvas_workspaces
        .iter()
        .find(|(workspace_id, workspace)| {
            *workspace_id != &active_workspace_id
                && workspace
                    .notes
                    .iter()
                    .any(|note| note.id == source_node_id)
        })
        .map(|(workspace_id, _)| workspace_id.clone())
}

pub(super) fn upsert_stream_failure_card(generations: &mut Vec<AssetData>, card: AssetData) {
    if card.delivery_recoverable {
        generations.retain(|existing| existing.id != card.id);
    }
    generations.insert(0, card);
}

pub(super) fn add_stream_failure_item(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    raw_prompt: &str,
    category: &str,
    mode: &str,
    ratio: &str,
    quality: &str,
    image_model: &str,
    origin: &str,
    conversation_id: &str,
    reason: &str,
    time: &str,
    reference_paths: &[String],
    failed_asset_id: Option<&str>,
) -> String {
    let asset_id = failed_asset_id
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let delivery_recoverable = failed_asset_id.is_some();
    let mut store_mut = store.borrow_mut();
    reveal_prompt_history_entry(&mut store_mut, raw_prompt);
    upsert_stream_failure_card(
        &mut store_mut.generations,
        AssetData {
            id: asset_id.clone(),
            conversation_id: conversation_id.to_string(),
            title: short_text(raw_prompt, 18),
            category: category.to_string(),
            kind: mode.to_string(),
            time: time.to_string(),
            prompt: raw_prompt.to_string(),
            ratio: ratio.to_string(),
            quality: quality.to_string(),
            model: image_model.to_string(),
            origin: origin.to_string(),
            width: 0,
            height: 0,
            source_path: "failed".to_string(),
            reference_paths: reference_paths.to_vec(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable,
            delivery_downloading: false,
        },
    );
    store_mut.notifications.insert(
        0,
        NotificationData {
            id: Uuid::new_v4().to_string(),
            title: format!("Generation failed: {}", short_text(raw_prompt, 24)),
            model: image_model.to_string(),
            time: time.to_string(),
            reason: reason.to_string(),
            success: false,
            read: false,
        },
    );
    save_local_store(app, &store_mut);
    push_all(app, &store_mut);
    asset_id
}

pub(super) fn restore_stream_inputs(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    category: &str,
    original_references: Vec<ReferenceData>,
    original_quote: QuoteContext,
) {
    let state = app.global::<AppState>();
    let mut store_mut = store.borrow_mut();
    if current_workspace_category(app) == category {
        state.set_quote_title(original_quote.title.into());
        state.set_quote_prompt(original_quote.prompt.into());
        state.set_quote_ratio(original_quote.ratio.into());
        state.set_quote_quality(original_quote.quality.into());
        state.set_quote_width(original_quote.width);
        state.set_quote_height(original_quote.height);
    }
    *references_for_category_mut(&mut store_mut.references, category) = original_references;
    save_local_store(app, &store_mut);
    push_all(app, &store_mut);
}

pub(super) fn set_stream_final_status(
    context: &AppContext,
    app: &AppWindow,
    category: &str,
    success_count: i32,
    failed_count: i32,
    failure_reason: Option<&str>,
) {
    if failed_count <= 0 {
        set_generation_status_for_category(context, app, category, "生成成功");
    } else if success_count > 0 {
        let status = failure_reason
            .filter(|reason| !reason.trim().is_empty())
            .map(|reason| format!("部分生成失败：{reason}"))
            .unwrap_or_else(|| "部分生成失败".to_string());
        set_generation_status_for_category(context, app, category, &status);
    } else {
        set_generation_status_for_category(
            context,
            app,
            category,
            failure_reason
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or("生成失败"),
        );
    }
}
