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
            selected_custom_prompts_for_category(&store, &current_workspace_category(app))
        };
        compose_selected_custom_prompts(&input_prompt, &selected_prompts)
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
        let Ok(image) = load_image(&path) else {
            let message = format!("原参考图无法读取，无法再次生成：{}", path.display());
            state.set_viewer_message(message.clone().into());
            state.set_generation_status(message.into());
            return false;
        };
        references.push(ReferenceData {
            id: Uuid::new_v4().to_string(),
            image,
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
    use super::submitted_prompt_for_visible_prompt;

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
}

pub(super) fn compose_selected_custom_prompts(
    input_prompt: &str,
    selected_prompts: &[String],
) -> String {
    let mut parts = selected_prompts
        .iter()
        .map(|prompt| prompt.trim())
        .filter(|prompt| !prompt.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let input_prompt = input_prompt.trim();
    if !input_prompt.is_empty() && input_prompt != "//" {
        parts.push(input_prompt.to_string());
    }
    parts.join("\n\n")
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
    display_prompt: &str,
    time: &str,
    bytes: &[u8],
    reference_paths: &[String],
    upscale_done: bool,
) -> Result<(Image, String, String)> {
    let (bytes, image, width, height) = generated_image_from_bytes(bytes)?;
    let source_path = save_generated_bytes(app, &bytes, raw_prompt)?;
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
        origin: "generation".to_string(),
        width,
        height,
        image,
        source_path: source_path.clone(),
        reference_paths: reference_paths.to_vec(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done,
        is_new: true,
    };
    let conversation_image = item.image.clone();
    let generated_id = item.id.clone();
    let history_prompt = item.prompt.clone();
    let mut store_mut = store.borrow_mut();
    reveal_prompt_history_entry(&mut store_mut, &history_prompt);
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
    Ok((conversation_image, source_path, generated_id))
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
    reference_paths: &[String],
) {
    let mut store_mut = store.borrow_mut();
    reveal_prompt_history_entry(&mut store_mut, raw_prompt);
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
            origin: "generation".to_string(),
            width: 0,
            height: 0,
            image: Image::default(),
            source_path: "failed".to_string(),
            reference_paths: reference_paths.to_vec(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
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
) {
    if failed_count <= 0 {
        set_generation_status_for_category(context, app, category, "生成成功");
    } else if success_count > 0 {
        set_generation_status_for_category(context, app, category, "部分生成失败");
    } else {
        set_generation_status_for_category(context, app, category, "生成失败");
    }
}
