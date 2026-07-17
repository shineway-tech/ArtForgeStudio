use super::*;
use std::collections::BTreeSet;

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
        client_request_id: request_id.clone(),
        local_task_id: local_task_id.clone(),
        server_task_id: String::new(),
        raw_prompt: raw_prompt.clone(),
        generation_prompt: generation_prompt.clone(),
        category: category.clone(),
        mode: mode.clone(),
        ratio: ratio.clone(),
        quality: quality.clone(),
        model_code: model_code.clone(),
        conversation_id: conversation_id.clone(),
        count,
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

    state.set_prompt("".into());
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
                for file_id in &uploaded { api.delete_reference(file_id); }
                if error.is_insufficient_credits() {
                    let _ = remove_pending_generation(&request.client_request_id);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次生图，请前往充值".to_string(),
                    });
                    return;
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
        local_task_id,
        Instant::now(),
    );
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
    if app.global::<AppState>().get_session_state().as_str() != "online" {
        return;
    }
    let local_records = load_pending_generations();
    let known_server_ids = local_records.iter()
        .filter(|record| !record.server_task_id.is_empty())
        .map(|record| record.server_task_id.clone())
        .collect::<BTreeSet<_>>();
    for record in local_records {
        if record.schema_version != 1 || record.client_request_id.trim().is_empty() {
            continue;
        }
        if category_is_generating(&context, &record.category) {
            continue;
        }
        resume_pending_generation(app, context.clone(), record);
    }
    recover_server_generation_tasks(app, context, known_server_ids);
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
                if summary.task_type != "image_generation" || known_server_ids.contains(&summary.id) {
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
                    client_request_id: format!("recovered_{}", Uuid::new_v4().simple()),
                    local_task_id: Uuid::new_v4().to_string(),
                    server_task_id: detail.id.clone(),
                    raw_prompt: prompt.clone(),
                    generation_prompt: prompt,
                    category: "character".to_string(),
                    mode: "game".to_string(),
                    ratio,
                    quality: detail.quality.clone(),
                    model_code: detail.model.as_ref().map(|model| model.code.clone()).unwrap_or_default(),
                    conversation_id: Uuid::new_v4().to_string(),
                    count: detail.requested_count.max(1),
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
        match api.create_task(&request) {
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
                            let _ = sender.send(GenerationOutcome::ImageSuccess {
                                bytes,
                                optimized: detail.prompt.clone().unwrap_or_else(|| record.generation_prompt.clone()),
                                time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
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
