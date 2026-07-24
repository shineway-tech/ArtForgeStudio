use super::*;
use std::collections::BTreeSet;

const PENDING_SUBMISSION_RECOVERY_TTL_MS: i64 = 15 * 60 * 1000;

#[derive(Clone)]
struct UpscaleSource {
    title: String,
    category: String,
    kind: String,
    prompt: String,
    conversation_id: String,
    source_path: String,
    width: i32,
    height: i32,
}

pub(super) fn start_backend_generation(
    app: &AppWindow,
    context: AppContext,
    raw_prompt: String,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let store = context.store.clone();
    let state = app.global::<AppState>();
    let model_code = state.get_image_model().to_string();
    if model_code.trim().is_empty() {
        state.set_generation_status("服务端没有可用的图像模型".into());
        return;
    }
    let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
    if category_is_generating(&context, &category) {
        stop_generation(app, &context);
        return;
    }
    let ratio = resolve_ratio_for_category(
        &category,
        &state.get_ratio().to_string(),
        &raw_prompt,
        &state.get_quote_ratio().to_string(),
    );
    let quality = state.get_quality().to_string();
    if !ensure_membership_quality_allowed(&state, &quality) {
        return;
    }
    let count = forced_count.unwrap_or_else(|| {
        if category == "action-sequence" { 1 } else { state.get_count().clamp(1, 4) }
    });
    let mode = state.get_mode().to_string();
    let original_references = references_for_category(&store.borrow().references, &category)
        .iter()
        .take(max_reference_images_for_category(&category))
        .cloned()
        .collect::<Vec<_>>();
    let reference_paths = original_references
        .iter()
        .map(|item| PathBuf::from(&item.source_path))
        .collect::<Vec<_>>();
    let quote = QuoteContext {
        title: state.get_quote_title().to_string(),
        prompt: state.get_quote_prompt().to_string(),
        ratio: state.get_quote_ratio().to_string(),
        quality: state.get_quote_quality().to_string(),
        width: state.get_quote_width(),
        height: state.get_quote_height(),
    };
    let controls = PromptControls {
        category: category.clone(),
        creation: normalize_creation_mode_for_category(&category, &state.get_creation_mode().to_string()),
        style: state.get_style_mode().to_string(),
        view: state.get_view_mode().to_string(),
        weather: state.get_weather_mode().to_string(),
        time: state.get_time_mode().to_string(),
        light: state.get_light_mode().to_string(),
    };
    let language = if state.get_translate_prompt() || state.get_language().as_str() == "en" {
        PromptLanguage::English
    } else {
        PromptLanguage::Chinese
    };
    let generation_prompt = build_generation_prompt(
        &raw_prompt,
        &controls,
        &quote,
        &category,
        &ratio,
        &quality,
        language,
    );

    if let Some(retry_failed_id) = retry_failed_id.as_deref() {
        let mut store = store.borrow_mut();
        store.generations.retain(|item| item.id != retry_failed_id);
        save_local_store(app, &store);
        push_all(app, &store);
    }

    let conversation_id = if create_conversation {
        Uuid::new_v4().to_string()
    } else {
        let current = state.get_current_conversation_id().to_string();
        if current.trim().is_empty() { Uuid::new_v4().to_string() } else { current }
    };
    let local_task_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().simple().to_string();
    let recovery_record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        local_task_id: local_task_id.clone(),
        server_task_id: String::new(),
        raw_prompt: raw_prompt.clone(),
        generation_prompt: generation_prompt.clone(),
        task_type: "image_generation".to_string(),
        category: category.clone(),
        mode: mode.clone(),
        ratio: ratio.clone(),
        quality: quality.clone(),
        model_code: model_code.clone(),
        conversation_id: conversation_id.clone(),
        count,
        target_width: 0,
        target_height: 0,
        create_conversation,
        reference_paths: reference_paths.iter().map(|path| path.display().to_string()).collect(),
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
    };
    if upsert_pending_generation(recovery_record).is_err() {
        state.set_generation_status("任务准备失败，请重试".into());
        return;
    }
    insert_active_generation(&context, ActiveGeneration {
        task_id: local_task_id.clone(),
        client_request_id: Some(request_id.clone()),
        server_task_id: None,
        category: category.clone(),
        conversation_id: conversation_id.clone(),
        prompt: raw_prompt.clone(),
        credit_cost: 0,
        total_count: count,
        loading_count: count,
        completed_count: 0,
        success_count: 0,
        failed_count: 0,
        progress: 1,
        eta: 0,
    });
    set_generation_status_for_category(&context, app, &category, "正在上传参考图...");
    sync_generation_state_for_current_category(&context, app);
    navigate_to(app, "generation");

    state.set_quote_title("".into());
    state.set_quote_prompt("".into());
    state.set_quote_ratio("".into());
    state.set_quote_quality("".into());
    {
        let mut store = store.borrow_mut();
        references_for_category_mut(&mut store.references, &category).clear();
        push_references(app, &store);
    }
    if create_conversation {
        let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
        conversations.insert(0, ConversationItem {
            id: conversation_id.clone().into(),
            title: short_text(&raw_prompt, 10).into(),
            image: Image::default(),
            loading: true,
        });
        state.set_conversations(ModelRc::new(VecModel::from(conversations)));
        state.set_current_conversation_id(conversation_id.clone().into());
    }

    let quality_for_worker = quality.clone();
    let aspect_ratio = api_aspect_ratio(&ratio);
    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let cancellations = context.cancelled_generation_requests.clone();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        let mut uploaded = Vec::new();
        for path in reference_paths {
            match api.upload_reference(&path) {
                Ok(file_id) => uploaded.push(file_id),
                Err(error) => {
                    for file_id in &uploaded { api.delete_reference(file_id); }
                    let _ = remove_pending_generation(&request_id);
                    let _ = sender.send(GenerationOutcome::Failure {
                        reason: error.generation_message(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                    return;
                }
            }
            let uploaded_snapshot = uploaded.clone();
            let _ = update_pending_generation(&request_id, |record| {
                record.uploaded_file_ids = uploaded_snapshot;
            });
            if generation_cancel_requested(&cancellations, &request_id) {
                cleanup_cancelled_generation(&api, &request_id, &uploaded, None, &cancellations);
                return;
            }
        }
        if generation_cancel_requested(&cancellations, &request_id) {
            cleanup_cancelled_generation(&api, &request_id, &uploaded, None, &cancellations);
            return;
        }
        let request = CreateGenerationTask {
            client_request_id: request_id,
            task_type: "image_generation".to_string(),
            model_code,
            prompt: generation_prompt.clone(),
            quality: Some(quality_for_worker.clone()),
            count: Some(count),
            aspect_ratio: Some(aspect_ratio),
            reference_file_ids: Some(uploaded.clone()),
            target_language: None,
        };
        let mut detail = match api.create_task(&request) {
            Ok(detail) => detail,
            Err(error) => {
                if error.is_insufficient_credits() {
                    for file_id in &uploaded { api.delete_reference(file_id); }
                    let _ = remove_pending_generation(&request.client_request_id);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次生图，请前往充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded { api.delete_reference(file_id); }
                    let _ = remove_pending_generation(&request.client_request_id);
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: error.generation_message(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        };
        let task_id = detail.id.clone();
        if generation_cancel_requested(&cancellations, &request.client_request_id) {
            cleanup_cancelled_generation(
                &api,
                &request.client_request_id,
                &uploaded,
                Some(&task_id),
                &cancellations,
            );
            return;
        }
        let task_id_for_record = task_id.clone();
        let _ = update_pending_generation(&request.client_request_id, |record| {
            record.server_task_id = task_id_for_record;
            record.uploaded_file_ids = uploaded.clone();
        });
        if sender.send(GenerationOutcome::Accepted { task_id: task_id.clone() }).is_err() {
            let _ = api.cancel(&task_id);
            return;
        }
        let mut handled_success = BTreeSet::new();
        let mut handled_failure = BTreeSet::new();
        loop {
            if generation_cancel_requested(&cancellations, &request.client_request_id) {
                cleanup_cancelled_generation(
                    &api,
                    &request.client_request_id,
                    &[],
                    Some(&task_id),
                    &cancellations,
                );
                return;
            }
            let _ = sender.send(GenerationOutcome::Progress { percent: detail.progress_percent });
            for item in &detail.items {
                if item.status == "succeeded" && !handled_success.contains(&item.index) {
                    if let Some(file) = item.file.as_ref() {
                        match api.download_verified(file) {
                            Ok(bytes) => {
                                handled_success.insert(item.index);
                                let _ = sender.send(GenerationOutcome::ImageSuccess {
                                    bytes,
                                    optimized: detail.prompt.clone().unwrap_or_else(|| generation_prompt.clone()),
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                    upscale_done: false,
                                    delivery: Some(DeliveryConfirmation {
                                        client_request_id: request.client_request_id.clone(),
                                        item_index: item.index,
                                        task_id: task_id.clone(),
                                        file_id: file.id.clone(),
                                        sha256: file.sha256.clone(),
                                        size_bytes: file.size_bytes.parse().unwrap_or(0),
                                    }),
                                });
                            }
                            Err(error) if detail.terminal() => {
                                handled_failure.insert(item.index);
                                let _ = sender.send(GenerationOutcome::ImageFailure {
                                    reason: error.generation_message(),
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                });
                            }
                            Err(_) => {}
                        }
                    }
                } else if matches!(item.status.as_str(), "failed" | "cancelled")
                    && handled_failure.insert(item.index)
                {
                    let reason = item.failure.as_ref().map(|failure| failure.message.clone())
                        .unwrap_or_else(|| "服务端未能生成该图片".to_string());
                    let _ = sender.send(GenerationOutcome::ImageFailure {
                        reason,
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                }
            }
            if detail.terminal() {
                let expected_success_count = detail.success_count.max(0) as usize;
                let _ = update_pending_generation(&request.client_request_id, |record| {
                    record.terminal = true;
                    record.expected_success_count = expected_success_count;
                });
                let _ = sender.send(GenerationOutcome::Finished);
                return;
            }
            std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
            detail = match api.task(&task_id) {
                Ok(detail) => detail,
                Err(error) => {
                    let _ = sender.send(GenerationOutcome::Failure {
                        reason: error.generation_message(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                    return;
                }
            };
        }
    });
    poll_generation_stream(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
        raw_prompt,
        category,
        mode,
        ratio,
        quality,
        state.get_image_model().to_string(),
        conversation_id,
        create_conversation,
        original_references,
        quote,
        true,
        local_task_id,
        Instant::now(),
    );
}

pub(super) fn start_backend_upscale(
    app: &AppWindow,
    context: AppContext,
    scale: u32,
    quality: String,
) {
    let state = app.global::<AppState>();
    if state.get_viewer_processing() {
        return;
    }
    if state.get_viewer_upscale_done() {
        state.set_viewer_message(
            processing_done_message(
                app,
                ProcessImageMode::Upscale {
                    scale: 2,
                    target_long_edge: 2048,
                },
            )
            .into(),
        );
        return;
    }
    if !require_online_operation(app, "清晰放大") {
        return;
    }
    let Some(backend) = context.backend.clone() else {
        state.set_viewer_message("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let model_code = state.get_image_model().to_string();
    if model_code.trim().is_empty() {
        state.set_viewer_message("服务端没有可用的图像模型".into());
        return;
    }

    let source = {
        let store = context.store.borrow();
        upscale_source_for_viewer(app, &store)
    };
    let Some(source) = source else {
        state.set_viewer_message("未找到要放大的图片".into());
        return;
    };
    if category_is_generating(&context, &source.category) {
        state.set_viewer_message("当前分类已有生成任务，请稍后再放大".into());
        return;
    }
    let Some((source_width, source_height)) = viewer_source_dimensions(&state, &source) else {
        state.set_viewer_message("图片尺寸不可用，无法放大".into());
        return;
    };
    let selected_quality = if quality.eq_ignore_ascii_case("4K") { "4K" } else { "2K" }.to_string();
    let target_long_edge = upscale_quality_long_edge(&selected_quality);
    if source_width.max(source_height) > target_long_edge {
        let message = if target_long_edge >= 4096 {
            "当前图片尺寸已超过 4K，暂不支持继续放大"
        } else {
            "当前图片已超过 2K，请选择 4K 放大"
        };
        state.set_viewer_message(message.into());
        return;
    }
    let (target_width, target_height) = upscale_dimensions(
        source_width,
        source_height,
        scale.clamp(2, 4),
        target_long_edge,
    );
    let billing_quality = quality_for_target_dimensions(target_width, target_height);
    if !ensure_membership_quality_allowed(&state, &billing_quality) {
        return;
    }
    let upload_path = match upscale_upload_path(app, &state, &source) {
        Ok(path) => path,
        Err(error) => {
            state.set_viewer_message(format!("放大任务准备失败：{error}").into());
            return;
        }
    };

    let request_id = Uuid::new_v4().simple().to_string();
    let local_task_id = Uuid::new_v4().to_string();
    let conversation_id = source.conversation_id.clone();
    let display_prompt = if source.prompt.trim().is_empty() {
        source.title.clone()
    } else {
        source.prompt.clone()
    };
    let raw_prompt = format!(
        "{} 清晰放大{}X",
        if source.title.trim().is_empty() { "图片" } else { source.title.trim() },
        scale.clamp(2, 4),
    );
    let generation_prompt = build_upscale_prompt(
        &display_prompt,
        target_width,
        target_height,
        scale.clamp(2, 4),
        &billing_quality,
    );
    let ratio = ratio_from_actual_dimensions(target_width as i32, target_height as i32);
    let reference_path = upload_path.display().to_string();
    let recovery_record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        local_task_id: local_task_id.clone(),
        server_task_id: String::new(),
        raw_prompt: raw_prompt.clone(),
        generation_prompt: generation_prompt.clone(),
        task_type: "image_upscale".to_string(),
        category: source.category.clone(),
        mode: source.kind.clone(),
        ratio: ratio.clone(),
        quality: billing_quality.clone(),
        model_code: model_code.clone(),
        conversation_id: conversation_id.clone(),
        count: 1,
        target_width,
        target_height,
        create_conversation: false,
        reference_paths: vec![reference_path.clone()],
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
    };
    if upsert_pending_generation(recovery_record).is_err() {
        state.set_viewer_message("放大任务准备失败，请重试".into());
        return;
    }

    insert_active_generation(&context, ActiveGeneration {
        task_id: local_task_id.clone(),
        client_request_id: Some(request_id.clone()),
        server_task_id: None,
        category: source.category.clone(),
        conversation_id: conversation_id.clone(),
        prompt: raw_prompt.clone(),
        credit_cost: 0,
        total_count: 1,
        loading_count: 1,
        completed_count: 0,
        success_count: 0,
        failed_count: 0,
        progress: 1,
        eta: 0,
    });
    state.set_viewer_processing(true);
    state.set_viewer_processing_progress(0);
    state.set_viewer_processing_label("正在提交放大任务".into());
    state.set_upscale_open(false);
    state.set_viewer_open(false);
    state.set_viewer_processing(false);
    state.set_viewer_processing_progress(0);
    set_generation_status_for_category(&context, app, &source.category, "正在上传原图...");
    sync_generation_state_for_current_category(&context, app);
    navigate_to(app, "generation");

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let cancellations = context.cancelled_generation_requests.clone();
    let source_prompt_for_result = display_prompt.clone();
    let source_category = source.category.clone();
    let quality_for_worker = billing_quality.clone();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        let mut uploaded = Vec::new();
        match api.upload_reference(&PathBuf::from(&reference_path)) {
            Ok(file_id) => uploaded.push(file_id),
            Err(error) => {
                let _ = remove_pending_generation(&request_id);
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: error.generation_message(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
        let uploaded_snapshot = uploaded.clone();
        let _ = update_pending_generation(&request_id, |record| {
            record.uploaded_file_ids = uploaded_snapshot;
        });
        if generation_cancel_requested(&cancellations, &request_id) {
            cleanup_cancelled_generation(&api, &request_id, &uploaded, None, &cancellations);
            return;
        }
        let request = CreateUpscaleGenerationTask {
            client_request_id: request_id.clone(),
            task_type: "image_upscale".to_string(),
            model_code,
            prompt: generation_prompt,
            quality: quality_for_worker,
            reference_file_ids: uploaded.clone(),
            target_width,
            target_height,
        };
        let mut detail = match api.create_upscale_task(&request) {
            Ok(detail) => detail,
            Err(error) => {
                if error.is_insufficient_credits() {
                    for file_id in &uploaded { api.delete_reference(file_id); }
                    let _ = remove_pending_generation(&request.client_request_id);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次放大，请前往充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded { api.delete_reference(file_id); }
                    let _ = remove_pending_generation(&request.client_request_id);
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: error.generation_message(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        };
        let task_id = detail.id.clone();
        if generation_cancel_requested(&cancellations, &request.client_request_id) {
            cleanup_cancelled_generation(
                &api,
                &request.client_request_id,
                &uploaded,
                Some(&task_id),
                &cancellations,
            );
            return;
        }
        let task_id_for_record = task_id.clone();
        let uploaded_for_record = uploaded.clone();
        let _ = update_pending_generation(&request.client_request_id, |record| {
            record.server_task_id = task_id_for_record;
            record.uploaded_file_ids = uploaded_for_record;
        });
        if sender.send(GenerationOutcome::Accepted { task_id: task_id.clone() }).is_err() {
            let _ = api.cancel(&task_id);
            return;
        }
        let mut handled_success = BTreeSet::new();
        let mut handled_failure = BTreeSet::new();
        loop {
            if generation_cancel_requested(&cancellations, &request.client_request_id) {
                cleanup_cancelled_generation(
                    &api,
                    &request.client_request_id,
                    &[],
                    Some(&task_id),
                    &cancellations,
                );
                return;
            }
            let _ = sender.send(GenerationOutcome::Progress { percent: detail.progress_percent });
            for item in &detail.items {
                if item.status == "succeeded" && !handled_success.contains(&item.index) {
                    if let Some(file) = item.file.as_ref() {
                        match api.download_verified(file) {
                            Ok(bytes) => {
                                handled_success.insert(item.index);
                                let _ = sender.send(GenerationOutcome::ImageSuccess {
                                    bytes,
                                    optimized: source_prompt_for_result.clone(),
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                    upscale_done: true,
                                    delivery: Some(DeliveryConfirmation {
                                        client_request_id: request.client_request_id.clone(),
                                        item_index: item.index,
                                        task_id: task_id.clone(),
                                        file_id: file.id.clone(),
                                        sha256: file.sha256.clone(),
                                        size_bytes: file.size_bytes.parse().unwrap_or(0),
                                    }),
                                });
                            }
                            Err(error) if detail.terminal() => {
                                handled_failure.insert(item.index);
                                let _ = sender.send(GenerationOutcome::ImageFailure {
                                    reason: error.generation_message(),
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                });
                            }
                            Err(_) => {}
                        }
                    }
                } else if matches!(item.status.as_str(), "failed" | "cancelled")
                    && handled_failure.insert(item.index)
                {
                    let reason = item.failure.as_ref().map(|failure| failure.message.clone())
                        .unwrap_or_else(|| "服务端未能放大该图片".to_string());
                    let _ = sender.send(GenerationOutcome::ImageFailure {
                        reason,
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                }
            }
            if detail.terminal() {
                let expected_success_count = detail.success_count.max(0) as usize;
                let _ = update_pending_generation(&request.client_request_id, |record| {
                    record.terminal = true;
                    record.expected_success_count = expected_success_count;
                });
                let _ = sender.send(GenerationOutcome::Finished);
                return;
            }
            std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
            detail = match api.task(&task_id) {
                Ok(detail) => detail,
                Err(error) => {
                    let _ = sender.send(GenerationOutcome::Failure {
                        reason: error.generation_message(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                    return;
                }
            };
        }
    });

    poll_generation_stream(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
        raw_prompt,
        source_category,
        source.kind,
        ratio,
        billing_quality,
        state.get_image_model().to_string(),
        conversation_id,
        false,
        vec![],
        QuoteContext {
            title: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            width: 0,
            height: 0,
        },
        false,
        local_task_id,
        Instant::now(),
    );
}

fn upscale_source_for_viewer(app: &AppWindow, store: &Store) -> Option<UpscaleSource> {
    let state = app.global::<AppState>();
    let id = state.get_viewer_id().to_string();
    let source = state.get_viewer_source().to_string();
    if source == "reference" {
        let category = resolve_category(&state.get_asset_type().to_string(), "");
        let reference = references_for_category(&store.references, &category)
            .iter()
            .find(|item| item.id == id)?;
        return Some(UpscaleSource {
            title: "参考图".to_string(),
            category,
            kind: state.get_mode().to_string(),
            prompt: state.get_viewer_prompt().to_string(),
            conversation_id: String::new(),
            source_path: reference.source_path.clone(),
            width: 0,
            height: 0,
        });
    }
    let item = viewer_item(store, &id, &source)?;
    Some(UpscaleSource {
        title: item.title.clone(),
        category: item.category.clone(),
        kind: item.kind.clone(),
        prompt: item.prompt.clone(),
        conversation_id: item.conversation_id.clone(),
        source_path: item.source_path.clone(),
        width: item.width,
        height: item.height,
    })
}

fn viewer_source_dimensions(state: &AppState, source: &UpscaleSource) -> Option<(u32, u32)> {
    if source.width > 0 && source.height > 0 {
        return Some((source.width as u32, source.height as u32));
    }
    let viewer_width = state.get_viewer_width();
    let viewer_height = state.get_viewer_height();
    if viewer_width > 0 && viewer_height > 0 {
        return Some((viewer_width as u32, viewer_height as u32));
    }
    let buffer = state.get_viewer_image().to_rgba8()?;
    if buffer.width() == 0 || buffer.height() == 0 {
        None
    } else {
        Some((buffer.width(), buffer.height()))
    }
}

fn quality_for_target_dimensions(width: u32, height: u32) -> String {
    let long_edge = width.max(height);
    if long_edge <= 1024 {
        "1K".to_string()
    } else if long_edge <= 2048 {
        "2K".to_string()
    } else {
        "4K".to_string()
    }
}

fn ensure_membership_quality_allowed(state: &AppState, requested_quality: &str) -> bool {
    let max_quality = normalized_quality(&state.get_membership_max_quality().to_string());
    let requested_quality = normalized_quality(requested_quality);
    if membership_allows_quality(max_quality, requested_quality) {
        return true;
    }

    let plan_name = state.get_membership_plan_name().to_string();
    let plan_label = if plan_name.trim().is_empty() {
        "当前会员"
    } else {
        plan_name.trim()
    };
    state.set_quality_restricted_message(
        format!(
            "{}最高支持 {} 图片，请升级会员后使用 {} 图片。",
            plan_label, max_quality, requested_quality
        )
        .into(),
    );
    state.set_quality_restricted_open(true);
    false
}

fn upscale_upload_path(
    app: &AppWindow,
    state: &AppState,
    source: &UpscaleSource,
) -> Result<PathBuf> {
    let trimmed = source.source_path.trim();
    if !trimmed.is_empty() && trimmed != "failed" && trimmed != "asset" {
        let path = PathBuf::from(trimmed);
        if path.is_file() {
            return Ok(path);
        }
    }
    let buffer = state
        .get_viewer_image()
        .to_rgba8()
        .ok_or_else(|| anyhow!("图片数据不可上传"))?;
    let width = buffer.width();
    let height = buffer.height();
    let rgba = image::RgbaImage::from_raw(width, height, buffer.as_bytes().to_vec())
        .ok_or_else(|| anyhow!("图片数据不可上传"))?;
    let bytes = encode_png_rgba(&rgba, width, height)?;
    let dir = output_dir_path(app).join("upscale-references");
    fs::create_dir_all(&dir)?;
    let stem = sanitize_filename(&format!("{}-upscale-source", source.title));
    let path = unique_path(dir.join(format!(
        "{}-{}.png",
        Local::now().format("%Y%m%d%H%M%S%3f"),
        stem,
    )));
    atomic_write_file(&path, &bytes)?;
    Ok(path)
}

fn build_upscale_prompt(
    original_prompt: &str,
    target_width: u32,
    target_height: u32,
    scale: u32,
    quality: &str,
) -> String {
    let source_hint = if original_prompt.trim().is_empty() {
        "无额外原始描述".to_string()
    } else {
        format!("原始描述：{}", original_prompt.trim())
    };
    format!(
        "请基于参考图进行清晰放大和细节增强，保持原图构图、主体、颜色、材质和整体风格不变，不新增主体，不改变画面比例。放大倍率：{}X，目标清晰度：{}，输出尺寸必须为 {}x{}。{}",
        scale.clamp(2, 4),
        quality,
        target_width,
        target_height,
        source_hint,
    )
}

fn generation_cancel_requested(
    cancellations: &Arc<Mutex<BTreeSet<String>>>,
    client_request_id: &str,
) -> bool {
    cancellations
        .lock()
        .map(|items| items.contains(client_request_id))
        .unwrap_or(false)
}

fn cleanup_cancelled_generation(
    api: &GenerationApi,
    client_request_id: &str,
    uploaded_file_ids: &[String],
    server_task_id: Option<&str>,
    cancellations: &Arc<Mutex<BTreeSet<String>>>,
) {
    if let Some(task_id) = server_task_id {
        let _ = api.cancel(task_id);
    } else {
        for file_id in uploaded_file_ids {
            api.delete_reference(file_id);
        }
    }
    let _ = remove_pending_generation(client_request_id);
    if let Ok(mut items) = cancellations.lock() {
        items.remove(client_request_id);
    }
}

pub(super) fn recover_pending_generations(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if state.get_session_state().as_str() != "online" {
        return;
    }
    let local_records = load_pending_generations();
    let known_server_ids = local_records.iter()
        .filter(|record| !record.server_task_id.is_empty())
        .map(|record| record.server_task_id.clone())
        .collect::<BTreeSet<_>>();
    let now_epoch_ms = Local::now().timestamp_millis();
    for record in local_records {
        if record.schema_version != 1 || record.client_request_id.trim().is_empty() {
            let _ = remove_pending_generation(&record.client_request_id);
            continue;
        }
        if record.server_task_id.is_empty()
            && !ensure_membership_quality_allowed(&state, &record.quality)
        {
            let _ = remove_pending_generation(&record.client_request_id);
            continue;
        }
        if pending_submission_recovery_expired(&record, now_epoch_ms) {
            let _ = remove_pending_generation(&record.client_request_id);
            continue;
        }
        if category_is_generating(&context, &record.category) {
            continue;
        }
        resume_pending_generation(app, context.clone(), record);
    }
    recover_server_generation_tasks(app, context, known_server_ids);
}

fn pending_submission_recovery_expired(
    record: &PendingGenerationRecord,
    now_epoch_ms: i64,
) -> bool {
    if !record.server_task_id.is_empty() {
        return false;
    }
    record.created_at_epoch_ms <= 0
        || now_epoch_ms.saturating_sub(record.created_at_epoch_ms)
            > PENDING_SUBMISSION_RECOVERY_TTL_MS
}

fn recover_server_generation_tasks(
    app: &AppWindow,
    context: AppContext,
    known_server_ids: BTreeSet<String>,
) {
    let Some(backend) = context.backend.clone() else { return; };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        let mut recovered = Vec::new();
        for status in ["queued", "processing", "completed", "partially_completed"] {
            let Ok(summaries) = api.list_tasks(status) else { continue; };
            for summary in summaries {
                if !matches!(summary.task_type.as_str(), "image_generation" | "image_upscale")
                    || known_server_ids.contains(&summary.id) {
                    continue;
                }
                let Ok(detail) = api.task(&summary.id) else { continue; };
                if detail.terminal() && !detail.items.iter().any(|item| {
                    item.file.as_ref().and_then(|file| file.download_url.as_ref()).is_some()
                }) {
                    continue;
                }
                let prompt = detail.prompt.clone().unwrap_or_else(|| "恢复的生成任务".to_string());
                let ratio = detail
                    .request
                    .get("aspect_ratio")
                    .and_then(Value::as_str)
                    .map(client_ratio_from_api)
                    .unwrap_or_else(|| "1:1".to_string());
                recovered.push(PendingGenerationRecord {
                    schema_version: 1,
                    created_at_epoch_ms: Local::now().timestamp_millis(),
                    client_request_id: format!("recovered_{}", Uuid::new_v4().simple()),
                    local_task_id: Uuid::new_v4().to_string(),
                    server_task_id: detail.id.clone(),
                    raw_prompt: prompt.clone(),
                    generation_prompt: prompt,
                    task_type: summary.task_type.clone(),
                    category: "character".to_string(),
                    mode: "game".to_string(),
                    ratio,
                    quality: detail.quality.clone(),
                    model_code: detail.model.as_ref().map(|model| model.code.clone()).unwrap_or_default(),
                    conversation_id: Uuid::new_v4().to_string(),
                    count: detail.requested_count.max(1),
                    target_width: detail
                        .request
                        .get("target_width")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                    target_height: detail
                        .request
                        .get("target_height")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                    create_conversation: true,
                    reference_paths: vec![],
                    uploaded_file_ids: vec![],
                    deliveries: vec![],
                    terminal: detail.terminal(),
                    expected_success_count: detail.success_count.max(0) as usize,
                });
            }
        }
        let _ = sender.send(recovered);
    });
    poll_server_generation_recovery(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_server_generation_recovery(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<RefCell<Option<mpsc::Receiver<Vec<PendingGenerationRecord>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = receiver.borrow().as_ref().and_then(|rx| rx.try_recv().ok());
        let Some(records) = result else {
            poll_server_generation_recovery(app_weak, context, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else { return; };
        for record in records {
            let _ = upsert_pending_generation(record.clone());
            if !category_is_generating(&context, &record.category) {
                resume_pending_generation(&app, context.clone(), record);
            }
        }
    });
}

fn resume_pending_generation(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
) {
    let Some(backend) = context.backend.clone() else { return; };
    let saved_count = record.deliveries.iter()
        .filter(|item| !item.local_path.is_empty() && Path::new(&item.local_path).is_file())
        .count() as i32;
    insert_active_generation(&context, ActiveGeneration {
        task_id: record.local_task_id.clone(),
        client_request_id: Some(record.client_request_id.clone()),
        server_task_id: (!record.server_task_id.is_empty()).then(|| record.server_task_id.clone()),
        category: record.category.clone(),
        conversation_id: record.conversation_id.clone(),
        prompt: record.raw_prompt.clone(),
        credit_cost: 0,
        total_count: record.count,
        loading_count: (record.count - saved_count).max(0),
        completed_count: saved_count,
        success_count: saved_count,
        failed_count: 0,
        progress: if saved_count > 0 { 50 } else { 1 },
        eta: 0,
    });
    let state = app.global::<AppState>();
    if record.create_conversation
        && !state.get_conversations().iter().any(|item| item.id.as_str() == record.conversation_id)
    {
        let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
        conversations.insert(0, ConversationItem {
            id: record.conversation_id.clone().into(),
            title: short_text(&record.raw_prompt, 10).into(),
            image: Image::default(),
            loading: true,
        });
        state.set_conversations(ModelRc::new(VecModel::from(conversations)));
    }
    set_generation_status_for_category(&context, app, &record.category, "正在恢复未完成任务...");
    sync_generation_state_for_current_category(&context, app);

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let worker_record = record.clone();
    let cancellations = context.cancelled_generation_requests.clone();
    std::thread::spawn(move || {
        run_recovered_generation_worker(backend, worker_record, sender, cancellations)
    });
    poll_generation_stream(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
        record.raw_prompt,
        record.category,
        record.mode,
        record.ratio,
        record.quality,
        record.model_code,
        record.conversation_id,
        record.create_conversation,
        vec![],
        QuoteContext {
            title: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            width: 0,
            height: 0,
        },
        true,
        record.local_task_id,
        Instant::now(),
    );
}

fn run_recovered_generation_worker(
    backend: Arc<BackendRuntime>,
    mut record: PendingGenerationRecord,
    sender: mpsc::Sender<GenerationOutcome>,
    cancellations: Arc<Mutex<BTreeSet<String>>>,
) {
    let api = GenerationApi::new(backend.api.clone());
    let mut uploaded = record.uploaded_file_ids.clone();
    for path in record.reference_paths.iter().skip(uploaded.len()) {
        match api.upload_reference(Path::new(path)) {
            Ok(file_id) => {
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                let _ = update_pending_generation(&record.client_request_id, |item| {
                    item.uploaded_file_ids = snapshot;
                });
                if generation_cancel_requested(&cancellations, &record.client_request_id) {
                    cleanup_cancelled_generation(
                        &api,
                        &record.client_request_id,
                        &uploaded,
                        None,
                        &cancellations,
                    );
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复参考图上传失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
    }
    let task_type = if record.task_type.trim().is_empty() {
        "image_generation"
    } else {
        record.task_type.as_str()
    };
    let aspect_ratio = api_aspect_ratio(&record.ratio);
    if generation_cancel_requested(&cancellations, &record.client_request_id) {
        cleanup_cancelled_generation(
            &api,
            &record.client_request_id,
            &uploaded,
            None,
            &cancellations,
        );
        return;
    }
    let mut detail = if record.server_task_id.is_empty() {
        let created = if task_type == "image_upscale" {
            let request = CreateUpscaleGenerationTask {
                client_request_id: record.client_request_id.clone(),
                task_type: "image_upscale".to_string(),
                model_code: record.model_code.clone(),
                prompt: record.generation_prompt.clone(),
                quality: record.quality.clone(),
                reference_file_ids: uploaded.clone(),
                target_width: record.target_width,
                target_height: record.target_height,
            };
            api.create_upscale_task(&request)
        } else {
            let request = CreateGenerationTask {
                client_request_id: record.client_request_id.clone(),
                task_type: "image_generation".to_string(),
                model_code: record.model_code.clone(),
                prompt: record.generation_prompt.clone(),
                quality: Some(record.quality.clone()),
                count: Some(record.count),
                aspect_ratio: Some(aspect_ratio),
                reference_file_ids: Some(uploaded.clone()),
                target_language: None,
            };
            api.create_task(&request)
        };
        match created {
            Ok(detail) => detail,
            Err(error) => {
                if error.is_insufficient_credits() {
                    for file_id in &uploaded { api.delete_reference(file_id); }
                    let _ = remove_pending_generation(&record.client_request_id);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次生图，请前往充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded { api.delete_reference(file_id); }
                    let _ = remove_pending_generation(&record.client_request_id);
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务提交失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
    } else {
        match api.task(&record.server_task_id) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务查询失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
    };
    if generation_cancel_requested(&cancellations, &record.client_request_id) {
        cleanup_cancelled_generation(
            &api,
            &record.client_request_id,
            &uploaded,
            Some(&detail.id),
            &cancellations,
        );
        return;
    }
    record.server_task_id = detail.id.clone();
    let server_task_id = detail.id.clone();
    let uploaded_snapshot = uploaded.clone();
    let server_id_snapshot = server_task_id.clone();
    let _ = update_pending_generation(&record.client_request_id, |item| {
        item.server_task_id = server_id_snapshot;
        item.uploaded_file_ids = uploaded_snapshot;
    });
    let _ = sender.send(GenerationOutcome::Accepted { task_id: server_task_id.clone() });

    let mut handled_success = record.deliveries.iter()
        .filter(|item| !item.local_path.is_empty() && Path::new(&item.local_path).is_file())
        .map(|item| item.item_index)
        .collect::<BTreeSet<_>>();
    let mut handled_failure = BTreeSet::new();
    for delivery in &record.deliveries {
        if delivery.acknowledged || delivery.local_path.is_empty() || !Path::new(&delivery.local_path).is_file() {
            continue;
        }
        if api.acknowledge_delivery(
            &server_task_id,
            &delivery.file_id,
            &delivery.sha256,
            delivery.size_bytes,
        ).is_ok() {
            let _ = pending_delivery_acknowledged(&record.client_request_id, &delivery.file_id);
        }
    }

    loop {
        if generation_cancel_requested(&cancellations, &record.client_request_id) {
            cleanup_cancelled_generation(
                &api,
                &record.client_request_id,
                &[],
                Some(&server_task_id),
                &cancellations,
            );
            return;
        }
        let _ = sender.send(GenerationOutcome::Progress { percent: detail.progress_percent });
        for item in &detail.items {
            if item.status == "succeeded" && !handled_success.contains(&item.index) {
                if let Some(file) = item.file.as_ref() {
                    match api.download_verified(file) {
                        Ok(bytes) => {
                            handled_success.insert(item.index);
                            let optimized = if record.task_type == "image_upscale" {
                                record.raw_prompt.clone()
                            } else {
                                detail.prompt.clone().unwrap_or_else(|| record.generation_prompt.clone())
                            };
                            let _ = sender.send(GenerationOutcome::ImageSuccess {
                                bytes,
                                optimized,
                                time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                upscale_done: record.task_type == "image_upscale",
                                delivery: Some(DeliveryConfirmation {
                                    client_request_id: record.client_request_id.clone(),
                                    item_index: item.index,
                                    task_id: server_task_id.clone(),
                                    file_id: file.id.clone(),
                                    sha256: file.sha256.clone(),
                                    size_bytes: file.size_bytes.parse().unwrap_or(0),
                                }),
                            });
                        }
                        Err(error) if detail.terminal() => {
                            handled_failure.insert(item.index);
                            let _ = sender.send(GenerationOutcome::ImageFailure {
                                reason: error.generation_message(),
                                time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                            });
                        }
                        Err(_) => {}
                    }
                }
            } else if matches!(item.status.as_str(), "failed" | "cancelled")
                && handled_failure.insert(item.index)
            {
                let _ = sender.send(GenerationOutcome::ImageFailure {
                    reason: item.failure.as_ref().map(|value| value.message.clone())
                        .unwrap_or_else(|| "服务端未能生成该图片".to_string()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
            }
        }
        if detail.terminal() {
            let expected = detail.success_count.max(0) as usize;
            let _ = update_pending_generation(&record.client_request_id, |item| {
                item.terminal = true;
                item.expected_success_count = expected;
            });
            let _ = sender.send(GenerationOutcome::Finished);
            return;
        }
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        detail = match api.task(&server_task_id) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务轮询失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        };
    }
}
