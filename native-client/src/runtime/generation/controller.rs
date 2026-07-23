use super::*;

pub(super) fn start_generation(
    app: &AppWindow,
    context: AppContext,
    override_prompt: Option<String>,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
) {
    let state = app.global::<AppState>();
    let input_prompt = state.get_prompt().trim().to_string();
    let raw_prompt = if !input_prompt.is_empty() {
        input_prompt
    } else {
        override_prompt.unwrap_or_default().trim().to_string()
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
    );
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
    let state = app.global::<AppState>();
    state.set_asset_type(item.category.clone().into());
    state.set_mode(item.kind.clone().into());
    state.set_ratio(item.ratio.clone().into());
    state.set_quality(item.quality.clone().into());
    state.set_count(1);
    state.set_prompt(item.prompt.clone().into());
    if !item.conversation_id.trim().is_empty() {
        state.set_current_conversation_id(item.conversation_id.clone().into());
    }
    start_generation(app, context, Some(item.prompt), false, Some(item.id), Some(1));
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
    set_generation_status_for_category(context, app, &category, "已停止生成");
    sync_generation_state_for_current_category(context, app);
    if !task.prompt.trim().is_empty() {
        state.set_prompt(task.prompt.clone().into());
    }
    finish_conversation_placeholder(&state, &task.conversation_id, None);
    push_references(app, &store.borrow());
    if let Some(client_request_id) = task.client_request_id.as_ref() {
        if let Ok(mut cancellations) = context.cancelled_generation_requests.lock() {
            cancellations.insert(client_request_id.clone());
        }
    }
    if let (Some(backend), Some(server_task_id)) = (context.backend.clone(), task.server_task_id) {
        std::thread::spawn(move || {
            let _ = GenerationApi::new(backend.api.clone()).cancel(&server_task_id);
        });
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
    conversation_id: &str,
    optimized: &str,
    time: &str,
    bytes: &[u8],
    upscale_done: bool,
) -> Result<(Image, String)> {
    let (bytes, image, width, height) = generated_image_from_bytes(bytes)?;
    let source_path = save_generated_bytes(app, &bytes, raw_prompt)?;
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        title: short_text(raw_prompt, 18),
        category: category.to_string(),
        kind: mode.to_string(),
        time: time.to_string(),
        prompt: display_generation_prompt(optimized),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality.to_string(),
        model: image_model.to_string(),
        width,
        height,
        image,
        source_path: source_path.clone(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done,
    };
    let conversation_image = item.image.clone();
    let mut store_mut = store.borrow_mut();
    store_mut.assets.insert(0, item.clone());
    store_mut.generations.insert(0, item);
    store_mut.notifications.insert(
        0,
        NotificationData {
            id: Uuid::new_v4().to_string(),
            title: format!("Generation succeeded: {}", short_text(raw_prompt, 24)),
            model: image_model.to_string(),
            time: time.to_string(),
            reason: String::new(),
            success: true,
            read: false,
        },
    );
    save_local_store(app, &store_mut);
    push_all(app, &store_mut);
    Ok((conversation_image, source_path))
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
    conversation_id: &str,
    reason: &str,
    time: &str,
) {
    let mut store_mut = store.borrow_mut();
    store_mut.generations.insert(
        0,
        AssetData {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            title: short_text(raw_prompt, 18),
            category: category.to_string(),
            kind: mode.to_string(),
            time: time.to_string(),
            prompt: raw_prompt.to_string(),
            ratio: ratio.to_string(),
            quality: quality.to_string(),
            model: image_model.to_string(),
            width: 0,
            height: 0,
            image: Image::default(),
            source_path: "failed".to_string(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
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
}

pub(super) fn restore_stream_inputs(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    raw_prompt: &str,
    category: &str,
    original_references: Vec<ReferenceData>,
    original_quote: QuoteContext,
) {
    let state = app.global::<AppState>();
    let mut store_mut = store.borrow_mut();
    if current_workspace_category(app) == category {
        state.set_prompt(raw_prompt.to_string().into());
        state.set_quote_title(original_quote.title.into());
        state.set_quote_prompt(original_quote.prompt.into());
        state.set_quote_ratio(original_quote.ratio.into());
        state.set_quote_quality(original_quote.quality.into());
        state.set_quote_width(original_quote.width);
        state.set_quote_height(original_quote.height);
    } else {
        set_prompt_draft_for_category(
            &mut store_mut.prompt_drafts,
            category,
            raw_prompt.to_string(),
        );
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
) {
    if failed_count <= 0 {
        set_generation_status_for_category(context, app, category, "生成成功");
    } else if success_count > 0 {
        set_generation_status_for_category(context, app, category, "部分生成失败");
    } else {
        set_generation_status_for_category(context, app, category, "生成失败");
    }
}
