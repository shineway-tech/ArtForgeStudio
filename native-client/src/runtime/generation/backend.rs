use super::*;

fn generation_download_staging_path(
    client_request_id: &str,
    item_index: usize,
    file: &TaskOutputFile,
) -> PathBuf {
    let extension = match file.mime_type.as_str() {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };
    app_data_dir().join("delivery-staging").join(format!(
        "{}-{}-{}.{}",
        sanitize_filename(client_request_id),
        item_index,
        sanitize_filename(&file.id),
        extension
    ))
}

fn delivery_confirmation_for_item(
    client_request_id: &str,
    detail: &GenerationTaskDetail,
    item_index: usize,
) -> Option<DeliveryConfirmation> {
    let item = detail.items.iter().find(|item| item.index == item_index)?;
    if item.status != "succeeded" {
        return None;
    }
    let file = item.file.as_ref()?;
    Some(DeliveryConfirmation {
        client_request_id: client_request_id.to_string(),
        item_index,
        task_id: detail.id.clone(),
        file_id: file.id.clone(),
        sha256: file.sha256.clone(),
        size_bytes: file.size_bytes.parse().unwrap_or(0),
        failed_asset_id: None,
    })
}

fn failed_delivery_confirmation_for_item(
    session_scope: &SessionScope,
    client_request_id: &str,
    detail: &GenerationTaskDetail,
    item_index: usize,
    existing_failed_asset_id: Option<&str>,
) -> Result<DeliveryConfirmation> {
    let mut delivery = delivery_confirmation_for_item(client_request_id, detail, item_index)
        .ok_or_else(|| anyhow!("succeeded generation item is missing delivery metadata"))?;
    let failed_asset_id = existing_failed_asset_id
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if !matches!(
        pending_delivery_failed(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            client_request_id,
            &delivery,
            &failed_asset_id,
        ),
        Ok(true)
    ) {
        return Err(anyhow!("pending generation delivery cannot be marked recoverable"));
    }
    delivery.failed_asset_id = Some(failed_asset_id);
    Ok(delivery)
}

fn failed_asset_id_for_delivery(record: &PendingGenerationRecord, file_id: &str) -> Option<String> {
    record
        .deliveries
        .iter()
        .find(|delivery| delivery.file_id == file_id)
        .map(|delivery| delivery.failed_asset_id.clone())
        .filter(|value| !value.trim().is_empty())
}

use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

fn report_unhandled_terminal_failures(
    sender: &mpsc::Sender<GenerationOutcome>,
    detail: &GenerationTaskDetail,
    expected_count: usize,
    handled_success: &BTreeSet<usize>,
    handled_failure: &mut BTreeSet<usize>,
    fallback: &str,
) {
    if !detail.terminal()
        || (detail.failure.is_none() && !detail.status.eq_ignore_ascii_case("failed"))
    {
        return;
    }
    let reason = detail
        .failure
        .as_ref()
        .map(TaskFailure::generation_message)
        .unwrap_or_else(|| fallback.to_string());
    let reported = handled_success.len() + handled_failure.len();
    let missing = expected_count.saturating_sub(reported);
    let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
    for synthetic_index in 0..missing {
        handled_failure.insert(usize::MAX.saturating_sub(synthetic_index));
        let _ = sender.send(GenerationOutcome::ImageFailure {
            reason: reason.clone(),
            time: time.clone(),
            delivery: None,
        });
    }
}

pub(super) fn reference_fingerprints(paths: &[PathBuf]) -> Result<(Vec<String>, Vec<u64>)> {
    let mut sha256 = Vec::with_capacity(paths.len());
    let mut sizes = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = fs::read(path).with_context(|| format!("无法读取参考图 {}", path.display()))?;
        sha256.push(format!("{:x}", Sha256::digest(&bytes)));
        sizes.push(bytes.len() as u64);
    }
    Ok((sha256, sizes))
}

pub(super) fn generation_references_match(record: &PendingGenerationRecord) -> bool {
    if record.reference_paths.is_empty() {
        return true;
    }
    if record.reference_paths.len() != record.reference_sha256.len()
        || record.reference_paths.len() != record.reference_size_bytes.len()
    {
        return false;
    }
    record
        .reference_paths
        .iter()
        .zip(&record.reference_sha256)
        .zip(&record.reference_size_bytes)
        .all(|((path, expected_sha256), expected_size)| {
            let Ok(bytes) = fs::read(path) else {
                return false;
            };
            bytes.len() as u64 == *expected_size
                && format!("{:x}", Sha256::digest(&bytes)).eq_ignore_ascii_case(expected_sha256)
        })
}

pub(super) fn recovered_delivery_path_matches(
    path: &str,
    expected_sha256: &str,
    expected_size_bytes: u64,
) -> bool {
    if path.trim().is_empty() || expected_sha256.trim().is_empty() {
        return false;
    }
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    bytes.len() as u64 == expected_size_bytes
        && format!("{:x}", Sha256::digest(&bytes)).eq_ignore_ascii_case(expected_sha256)
        && image::load_from_memory(&bytes).is_ok()
}

pub(super) fn recovered_delivery_file_matches(delivery: &PendingDeliveryRecord) -> bool {
    recovered_delivery_path_matches(&delivery.local_path, &delivery.sha256, delivery.size_bytes)
}

fn recovered_delivery_ready_for_ack(
    delivery: &PendingDeliveryRecord,
    verified_file_ids: &BTreeSet<String>,
) -> bool {
    !delivery.acknowledged && verified_file_ids.contains(&delivery.file_id)
}

fn sanitize_recovered_delivery_paths_with<F>(
    record: &mut PendingGenerationRecord,
    persist_invalid_file_ids: F,
) -> Result<BTreeSet<String>>
where
    F: FnOnce(&BTreeSet<String>) -> Result<bool>,
{
    let mut verified_file_ids = BTreeSet::new();
    let mut invalid_file_ids = BTreeSet::new();
    for delivery in &record.deliveries {
        if delivery.local_path.trim().is_empty() {
            continue;
        }
        if recovered_delivery_file_matches(delivery) {
            verified_file_ids.insert(delivery.file_id.clone());
        } else if !delivery.acknowledged {
            invalid_file_ids.insert(delivery.file_id.clone());
        }
    }
    if invalid_file_ids.is_empty() {
        return Ok(verified_file_ids);
    }
    if !persist_invalid_file_ids(&invalid_file_ids)? {
        return Err(anyhow!(
            "pending generation delivery is missing or belongs to another session scope"
        ));
    }
    for delivery in &mut record.deliveries {
        if invalid_file_ids.contains(&delivery.file_id) {
            delivery.local_path.clear();
        }
    }
    Ok(verified_file_ids)
}

pub(super) fn sanitize_recovered_delivery_paths(
    record: &mut PendingGenerationRecord,
) -> Result<BTreeSet<String>> {
    let owner_user_id = record.owner_user_id.clone();
    let auth_epoch = record.auth_epoch;
    let client_request_id = record.client_request_id.clone();
    sanitize_recovered_delivery_paths_with(record, |invalid_file_ids| {
        let mut cleared_file_ids = BTreeSet::new();
        let record_matched = update_pending_generation_scoped(
            &owner_user_id,
            auth_epoch,
            &client_request_id,
            |stored| {
                for delivery in &mut stored.deliveries {
                    if invalid_file_ids.contains(&delivery.file_id) {
                        delivery.local_path.clear();
                        cleared_file_ids.insert(delivery.file_id.clone());
                    }
                }
            },
        )?;
        Ok(record_matched && &cleared_file_ids == invalid_file_ids)
    })
}

pub(super) fn clear_recovered_delivery_local_path(
    session_scope: &SessionScope,
    client_request_id: &str,
    file_id: &str,
) -> Result<bool> {
    let mut cleared = false;
    let record_matched = update_pending_generation_scoped(
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
        client_request_id,
        |record| {
            if let Some(delivery) = record
                .deliveries
                .iter_mut()
                .find(|delivery| delivery.file_id == file_id)
            {
                delivery.local_path.clear();
                cleared = true;
            }
        },
    )?;
    Ok(record_matched && cleared)
}

pub(super) fn backend_generation_scope_active(
    backend: &BackendRuntime,
    session_scope: &SessionScope,
) -> bool {
    backend.api.session().is_scope_current(session_scope)
}

#[derive(Clone)]
struct UpscaleSource {
    title: String,
    category: String,
    kind: String,
    prompt: String,
    conversation_id: String,
    source_path: String,
    reference_paths: Vec<String>,
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
    existing_generation_policy: ExistingGenerationPolicy,
    destination: GenerationDestination,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let Some(session_scope) = current_generation_session_scope(&context) else {
        app.global::<AppState>()
            .set_generation_status("登录状态已变化，请重新发起生成".into());
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
        match existing_generation_policy {
            ExistingGenerationPolicy::StopExisting => stop_generation(app, &context),
            ExistingGenerationPolicy::KeepExisting => {
                set_generation_status_for_category(
                    &context,
                    app,
                    &category,
                    "当前分类已有生成任务，已保留正在进行中的任务",
                );
                sync_generation_state_for_current_category(&context, app);
                push_generations(app, &store.borrow());
                if destination == GenerationDestination::Gallery {
                    navigate_to_with_store(app, &store.borrow(), "generation");
                }
            }
        }
        return;
    }
    let ratio = resolve_ratio_for_category(
        &category,
        &state.get_ratio().to_string(),
        &raw_prompt,
        &state.get_quote_ratio().to_string(),
    );
    let quality = state.get_quality().to_string();
    let count = forced_count.unwrap_or_else(|| state.get_count().clamp(1, 4));
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
    let generation_reference_paths = reference_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let (reference_sha256, reference_size_bytes) = match reference_fingerprints(&reference_paths) {
        Ok(fingerprints) => fingerprints,
        Err(error) => {
            state.set_generation_status(format!("参考图校验失败：{error}").into());
            return;
        }
    };
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
        creation: normalize_creation_mode_for_category(
            &category,
            &state.get_creation_mode().to_string(),
        ),
        style: state.get_style_mode().to_string(),
        view: state.get_view_mode().to_string(),
        weather: state.get_weather_mode().to_string(),
        time: state.get_time_mode().to_string(),
        light: state.get_light_mode().to_string(),
    };
    let deep_english = state
        .get_deep_optimization_applied_english()
        .trim()
        .to_string();
    let deep_chinese = state
        .get_deep_optimization_applied_chinese()
        .trim()
        .to_string();
    let uses_deep_english = !deep_english.is_empty()
        && (raw_prompt.trim() == deep_english || raw_prompt.trim().ends_with(&deep_english));
    let display_prompt = if uses_deep_english && !deep_chinese.is_empty() {
        let prefix = raw_prompt
            .trim()
            .strip_suffix(&deep_english)
            .unwrap_or_default();
        format!("{prefix}{deep_chinese}")
    } else {
        raw_prompt.clone()
    };
    let language = if uses_deep_english
        || state.get_translate_prompt()
        || state.get_language().as_str() == "en"
    {
        PromptLanguage::English
    } else {
        PromptLanguage::Chinese
    };
    let generation_prompt = build_generation_prompt(
        &raw_prompt,
        &state.get_negative_prompt().to_string(),
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
        if current.trim().is_empty() {
            Uuid::new_v4().to_string()
        } else {
            current
        }
    };
    let local_task_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().simple().to_string();
    let recovery_record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        auth_epoch: session_scope.auth_epoch,
        local_task_id: local_task_id.clone(),
        server_task_id: String::new(),
        raw_prompt: display_prompt.clone(),
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
        reference_paths: generation_reference_paths.clone(),
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: generation_reference_paths.clone(),
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: match &destination {
            GenerationDestination::Canvas { source_node_id } => source_node_id.clone(),
            GenerationDestination::Gallery => String::new(),
        },
        canvas_ui_extraction: false,
    };
    if upsert_pending_generation_scoped(
        recovery_record.clone(),
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    )
    .is_err()
    {
        state.set_generation_status("任务准备失败，请重试".into());
        return;
    }
    insert_active_generation(
        &context,
        ActiveGeneration {
            task_id: local_task_id.clone(),
            client_request_id: Some(request_id.clone()),
            server_task_id: None,
            category: category.clone(),
            conversation_id: conversation_id.clone(),
            prompt: display_prompt.clone(),
            credit_cost: 0,
            total_count: count,
            loading_count: count,
            completed_count: 0,
            success_count: 0,
            failed_count: 0,
            last_failure_reason: None,
            progress: 1,
            eta: 0,
            latest_success_id: None,
            session_scope: session_scope.clone(),
            destination: destination.clone(),
        },
    );
    set_generation_status_for_category(&context, app, &category, "正在优化并上传参考图...");
    sync_generation_state_for_current_category(&context, app);
    if destination == GenerationDestination::Gallery {
        navigate_to_with_store(app, &context.store.borrow(), "generation");
    }

    if destination == GenerationDestination::Gallery {
        state.set_quote_title("".into());
        state.set_quote_prompt("".into());
        state.set_quote_ratio("".into());
        state.set_quote_quality("".into());
    }
    if create_conversation {
        let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
        conversations.insert(
            0,
            ConversationItem {
                id: conversation_id.clone().into(),
                title: short_text(&display_prompt, 10).into(),
                image: Image::default(),
                loading: true,
            },
        );
        state.set_conversations(ModelRc::new(VecModel::from(conversations)));
        state.set_current_conversation_id(conversation_id.clone().into());
    }

    let quality_for_worker = quality.clone();
    let aspect_ratio = api_aspect_ratio(&ratio);
    let display_prompt_for_worker = display_prompt.clone();
    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let cancellations = context.cancelled_generation_requests.clone();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        if !backend_generation_scope_active(&backend, &worker_scope) {
            return;
        }
        if !generation_references_match(&recovery_record) {
            let _ = sender.send(GenerationOutcome::Failure {
                reason: "参考图内容已变化，任务已暂停，请重新发起".to_string(),
                time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
            });
            return;
        }
        let mut uploaded = Vec::new();
        for path in reference_paths {
            match api.upload_reference_scoped(&path, &worker_scope) {
                Ok(file_id) => uploaded.push(file_id),
                Err(error) => {
                    if !backend_generation_scope_active(&backend, &worker_scope) {
                        return;
                    }
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request_id,
                    );
                    let _ = sender.send(GenerationOutcome::Failure {
                        reason: error.generation_message(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                    return;
                }
            }
            let uploaded_snapshot = uploaded.clone();
            if !matches!(
                update_pending_generation_scoped(
                    &worker_scope.owner_user_id,
                    worker_scope.auth_epoch,
                    &request_id,
                    |record| record.uploaded_file_ids = uploaded_snapshot,
                ),
                Ok(true)
            ) {
                if let Some(file_id) = uploaded.last() {
                    let _ = api.delete_reference_scoped(file_id, &worker_scope);
                }
                return;
            }
            if generation_cancel_requested(&cancellations, &request_id) {
                cleanup_cancelled_generation(
                    &backend,
                    &api,
                    &worker_scope,
                    &request_id,
                    &uploaded,
                    None,
                    &cancellations,
                );
                return;
            }
        }
        if generation_cancel_requested(&cancellations, &request_id) {
            cleanup_cancelled_generation(
                &backend,
                &api,
                &worker_scope,
                &request_id,
                &uploaded,
                None,
                &cancellations,
            );
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
        let mut detail = match api.create_task_scoped(&request, &worker_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &worker_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request.client_request_id,
                    );
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次生图，请前往充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request.client_request_id,
                    );
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
                &backend,
                &api,
                &worker_scope,
                &request.client_request_id,
                &uploaded,
                Some(&task_id),
                &cancellations,
            );
            return;
        }
        let task_id_for_record = task_id.clone();
        if !matches!(
            update_pending_generation_scoped(
                &worker_scope.owner_user_id,
                worker_scope.auth_epoch,
                &request.client_request_id,
                |record| {
                    record.server_task_id = task_id_for_record;
                    record.uploaded_file_ids = uploaded.clone();
                },
            ),
            Ok(true)
        ) {
            return;
        }
        if sender
            .send(GenerationOutcome::Accepted {
                task_id: task_id.clone(),
            })
            .is_err()
        {
            let _ = api.cancel_scoped(&task_id, &worker_scope);
            return;
        }
        let mut handled_success = BTreeSet::new();
        let mut handled_failure = BTreeSet::new();
        loop {
            if !backend_generation_scope_active(&backend, &worker_scope) {
                return;
            }
            if generation_cancel_requested(&cancellations, &request.client_request_id) {
                cleanup_cancelled_generation(
                    &backend,
                    &api,
                    &worker_scope,
                    &request.client_request_id,
                    &[],
                    Some(&task_id),
                    &cancellations,
                );
                return;
            }
            let _ = sender.send(GenerationOutcome::Progress {
                percent: detail.progress_percent,
            });
            for item in &detail.items {
                if item.status == "succeeded" && !handled_success.contains(&item.index) {
                    if let Some(file) = item.file.as_ref() {
                        let local_path = generation_download_staging_path(
                            &request.client_request_id,
                            item.index,
                            file,
                        );
                        match api.download_verified_to_path_scoped(file, &worker_scope, &local_path)
                        {
                            Ok(()) => {
                                handled_success.insert(item.index);
                                if sender
                                    .send(GenerationOutcome::ImageSuccess {
                                        local_path: local_path.display().to_string(),
                                        display_prompt: display_prompt_for_worker.clone(),
                                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                        upscale_done: false,
                                        delivery: delivery_confirmation_for_item(
                                            &request.client_request_id,
                                            &detail,
                                            item.index,
                                        ),
                                    })
                                    .is_err()
                                {
                                    let _ = fs::remove_file(local_path);
                                    return;
                                }
                            }
                            Err(error) if detail.terminal() => {
                                handled_failure.insert(item.index);
                                let (reason, delivery) = match failed_delivery_confirmation_for_item(
                                    &worker_scope,
                                    &request.client_request_id,
                                    &detail,
                                    item.index,
                                    None,
                                ) {
                                    Ok(delivery) => (error.generation_message(), Some(delivery)),
                                    Err(_) => (
                                        "本地生成恢复记录无法安全更新，已暂停交付，请重启后重试"
                                            .to_string(),
                                        None,
                                    ),
                                };
                                let _ = sender.send(GenerationOutcome::ImageFailure {
                                    reason,
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                    delivery,
                                });
                            }
                            Err(_) => {}
                        }
                    }
                } else if matches!(item.status.as_str(), "failed" | "cancelled")
                    && handled_failure.insert(item.index)
                {
                    let reason = item
                        .failure
                        .as_ref()
                        .map(TaskFailure::generation_message)
                        .unwrap_or_else(|| "服务端未能生成该图片".to_string());
                    let _ = sender.send(GenerationOutcome::ImageFailure {
                        reason,
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                        delivery: None,
                    });
                }
            }
            if detail.terminal() {
                report_unhandled_terminal_failures(
                    &sender,
                    &detail,
                    count.max(1) as usize,
                    &handled_success,
                    &mut handled_failure,
                    "服务端未能生成该图片",
                );
                let expected_success_count = detail.success_count.max(0) as usize;
                if !matches!(
                    update_pending_generation_scoped(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request.client_request_id,
                        |record| {
                            record.terminal = true;
                            record.expected_success_count = expected_success_count;
                        },
                    ),
                    Ok(true)
                ) {
                    return;
                }
                let _ = sender.send(GenerationOutcome::Finished);
                return;
            }
            std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
            detail = match api.task_scoped(&task_id, &worker_scope) {
                Ok(detail) => detail,
                Err(error) => {
                    if !backend_generation_scope_active(&backend, &worker_scope) {
                        return;
                    }
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
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
        display_prompt,
        category,
        mode,
        ratio,
        quality,
        state.get_image_model().to_string(),
        "generation".to_string(),
        conversation_id,
        create_conversation,
        generation_reference_paths,
        original_references,
        quote,
        destination == GenerationDestination::Gallery,
        local_task_id,
        Instant::now(),
    );
}

pub(super) fn start_backend_image_edit(
    app: &AppWindow,
    context: AppContext,
    source_path: PathBuf,
    mask_path: PathBuf,
    prompt: String,
    model_code: String,
    quality: String,
) {
    let state = app.global::<AppState>();
    let Some(backend) = context.backend.clone() else {
        cleanup_image_edit_input_path(&source_path);
        cleanup_image_edit_input_path(&mask_path);
        state.set_image_editor_generating(false);
        state.set_image_editor_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let Some(session_scope) = current_generation_session_scope(&context) else {
        cleanup_image_edit_input_path(&source_path);
        cleanup_image_edit_input_path(&mask_path);
        state.set_image_editor_generating(false);
        state.set_image_editor_status("登录状态已变化，请重新发起编辑".into());
        return;
    };
    let viewer_id = state.get_viewer_id().to_string();
    let viewer_source = state.get_viewer_source().to_string();
    let original = viewer_item(&context.store.borrow(), &viewer_id, &viewer_source).cloned();
    let category = original
        .as_ref()
        .map(|item| item.category.clone())
        .unwrap_or_else(|| resolve_category(&state.get_asset_type().to_string(), &prompt));
    if category_is_generating(&context, &category) {
        cleanup_image_edit_input_path(&source_path);
        cleanup_image_edit_input_path(&mask_path);
        state.set_image_editor_generating(false);
        state.set_image_editor_status("当前分类已有生成任务，请稍后再编辑".into());
        return;
    }
    let mode = original
        .as_ref()
        .map(|item| item.kind.clone())
        .unwrap_or_else(|| state.get_mode().to_string());
    let conversation_id = original
        .as_ref()
        .map(|item| item.conversation_id.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let lineage_reference_paths = original
        .as_ref()
        .map(|item| {
            let source = item.source_path.trim();
            if !source.is_empty() && source != "failed" && Path::new(source).is_file() {
                vec![source.to_string()]
            } else {
                item.reference_paths
                    .iter()
                    .filter(|path| Path::new(path).is_file())
                    .cloned()
                    .collect()
            }
        })
        .unwrap_or_else(|| {
            references_for_category(&context.store.borrow().references, &category)
                .iter()
                .find(|reference| reference.id == viewer_id)
                .map(|reference| vec![reference.source_path.clone()])
                .unwrap_or_default()
        });
    let width = state.get_image_editor_source_width().max(1) as u32;
    let height = state.get_image_editor_source_height().max(1) as u32;
    let ratio = ratio_from_actual_dimensions(width as i32, height as i32);
    let request_id = Uuid::new_v4().simple().to_string();
    let local_task_id = Uuid::new_v4().to_string();
    let source_path_text = source_path.display().to_string();
    let mask_path_text = mask_path.display().to_string();
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints(&[source_path.clone(), mask_path.clone()]) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                cleanup_image_edit_input_path(&source_path);
                cleanup_image_edit_input_path(&mask_path);
                state.set_image_editor_generating(false);
                state.set_image_editor_status(format!("图片编辑输入校验失败：{error}").into());
                return;
            }
        };
    let record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        auth_epoch: session_scope.auth_epoch,
        local_task_id: local_task_id.clone(),
        server_task_id: String::new(),
        raw_prompt: prompt.clone(),
        generation_prompt: prompt.clone(),
        task_type: "image_edit".to_string(),
        category: category.clone(),
        mode: mode.clone(),
        ratio: ratio.clone(),
        quality: quality.clone(),
        model_code: model_code.clone(),
        conversation_id: conversation_id.clone(),
        count: 1,
        target_width: width,
        target_height: height,
        create_conversation: false,
        reference_paths: vec![source_path_text.clone(), mask_path_text],
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: lineage_reference_paths.clone(),
        uploaded_file_ids: Vec::new(),
        deliveries: Vec::new(),
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: String::new(),
        canvas_ui_extraction: false,
    };
    if upsert_pending_generation_scoped(
        record.clone(),
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    )
    .is_err()
    {
        cleanup_image_edit_record_inputs(&record);
        state.set_image_editor_generating(false);
        state.set_image_editor_status("图片编辑任务准备失败，请重试".into());
        return;
    }
    insert_active_generation(
        &context,
        ActiveGeneration {
            task_id: local_task_id.clone(),
            client_request_id: Some(request_id),
            server_task_id: None,
            category: category.clone(),
            conversation_id: conversation_id.clone(),
            prompt: prompt.clone(),
            credit_cost: state.get_image_editor_estimated_credit_cost(),
            total_count: 1,
            loading_count: 1,
            completed_count: 0,
            success_count: 0,
            failed_count: 0,
            last_failure_reason: None,
            progress: 1,
            eta: 0,
            latest_success_id: None,
            session_scope: session_scope.clone(),
            destination: GenerationDestination::Gallery,
        },
    );
    set_generation_status_for_category(&context, app, &category, "正在上传原图和遮罩...");
    sync_generation_state_for_current_category(&context, app);
    state.set_image_editor_generating(false);
    navigate_to_with_store(app, &context.store.borrow(), "generation");

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let cancellations = context.cancelled_generation_requests.clone();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        run_recovered_generation_worker(backend, worker_scope, record, sender, cancellations)
    });
    poll_generation_stream(
        app.as_weak(),
        context,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
        prompt,
        category,
        mode,
        ratio,
        quality,
        model_code,
        "image_edit".to_string(),
        conversation_id,
        false,
        lineage_reference_paths,
        Vec::new(),
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
    let Some(session_scope) = current_generation_session_scope(&context) else {
        state.set_viewer_message("登录状态已变化，请重新发起放大".into());
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
    let selected_quality = if quality.eq_ignore_ascii_case("4K") {
        "4K"
    } else {
        "2K"
    }
    .to_string();
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
        if source.title.trim().is_empty() {
            "图片"
        } else {
            source.title.trim()
        },
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
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints(std::slice::from_ref(&upload_path)) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                cleanup_upscale_input_path(&upload_path);
                state.set_viewer_message(format!("放大输入校验失败：{error}").into());
                return;
            }
        };
    let recovery_record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        auth_epoch: session_scope.auth_epoch,
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
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: source.reference_paths.clone(),
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: String::new(),
        canvas_ui_extraction: false,
    };
    if upsert_pending_generation_scoped(
        recovery_record.clone(),
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    )
    .is_err()
    {
        cleanup_upscale_input_path(&upload_path);
        state.set_viewer_message("放大任务准备失败，请重试".into());
        return;
    }

    insert_active_generation(
        &context,
        ActiveGeneration {
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
            last_failure_reason: None,
            progress: 1,
            eta: 0,
            latest_success_id: None,
            session_scope: session_scope.clone(),
            destination: GenerationDestination::Gallery,
        },
    );
    state.set_viewer_processing(true);
    state.set_viewer_processing_progress(0);
    state.set_viewer_processing_label("正在提交放大任务".into());
    state.set_upscale_open(false);
    state.set_viewer_open(false);
    state.set_viewer_processing(false);
    state.set_viewer_processing_progress(0);
    set_generation_status_for_category(&context, app, &source.category, "正在上传原图...");
    sync_generation_state_for_current_category(&context, app);
    navigate_to_with_store(app, &context.store.borrow(), "generation");

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let cancellations = context.cancelled_generation_requests.clone();
    let source_prompt_for_result = display_prompt.clone();
    let source_category = source.category.clone();
    let source_reference_paths = source.reference_paths.clone();
    let quality_for_worker = billing_quality.clone();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        if !backend_generation_scope_active(&backend, &worker_scope)
            || !generation_references_match(&recovery_record)
        {
            return;
        }
        let mut uploaded = Vec::new();
        match api.upload_reference_scoped(&PathBuf::from(&reference_path), &worker_scope) {
            Ok(file_id) => uploaded.push(file_id),
            Err(error) => {
                if !backend_generation_scope_active(&backend, &worker_scope) {
                    return;
                }
                if matches!(
                    remove_pending_generation_scoped(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request_id,
                    ),
                    Ok(true)
                ) {
                    cleanup_upscale_input_path(Path::new(&reference_path));
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: error.generation_message(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
        let uploaded_snapshot = uploaded.clone();
        if !matches!(
            update_pending_generation_scoped(
                &worker_scope.owner_user_id,
                worker_scope.auth_epoch,
                &request_id,
                |record| {
                    record.uploaded_file_ids = uploaded_snapshot;
                    record.reference_paths.clear();
                    record.reference_sha256.clear();
                    record.reference_size_bytes.clear();
                },
            ),
            Ok(true)
        ) {
            if let Some(file_id) = uploaded.last() {
                let _ = api.delete_reference_scoped(file_id, &worker_scope);
            }
            return;
        }
        // The remote file id is now durable in the recovery record, so a restart no longer
        // needs the local managed upload input.
        cleanup_upscale_input_path(Path::new(&reference_path));
        if generation_cancel_requested(&cancellations, &request_id) {
            cleanup_cancelled_generation(
                &backend,
                &api,
                &worker_scope,
                &request_id,
                &uploaded,
                None,
                &cancellations,
            );
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
        let mut detail = match api.create_upscale_task_scoped(&request, &worker_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &worker_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request.client_request_id,
                    );
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次放大，请前往充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request.client_request_id,
                    );
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
                &backend,
                &api,
                &worker_scope,
                &request.client_request_id,
                &uploaded,
                Some(&task_id),
                &cancellations,
            );
            return;
        }
        let task_id_for_record = task_id.clone();
        let uploaded_for_record = uploaded.clone();
        if !matches!(
            update_pending_generation_scoped(
                &worker_scope.owner_user_id,
                worker_scope.auth_epoch,
                &request.client_request_id,
                |record| {
                    record.server_task_id = task_id_for_record;
                    record.uploaded_file_ids = uploaded_for_record;
                },
            ),
            Ok(true)
        ) {
            return;
        }
        if sender
            .send(GenerationOutcome::Accepted {
                task_id: task_id.clone(),
            })
            .is_err()
        {
            let _ = api.cancel_scoped(&task_id, &worker_scope);
            return;
        }
        let mut handled_success = BTreeSet::new();
        let mut handled_failure = BTreeSet::new();
        loop {
            if !backend_generation_scope_active(&backend, &worker_scope) {
                return;
            }
            if generation_cancel_requested(&cancellations, &request.client_request_id) {
                cleanup_cancelled_generation(
                    &backend,
                    &api,
                    &worker_scope,
                    &request.client_request_id,
                    &[],
                    Some(&task_id),
                    &cancellations,
                );
                return;
            }
            let _ = sender.send(GenerationOutcome::Progress {
                percent: detail.progress_percent,
            });
            for item in &detail.items {
                if item.status == "succeeded" && !handled_success.contains(&item.index) {
                    if let Some(file) = item.file.as_ref() {
                        let local_path = generation_download_staging_path(
                            &request.client_request_id,
                            item.index,
                            file,
                        );
                        match api.download_verified_to_path_scoped(file, &worker_scope, &local_path)
                        {
                            Ok(()) => {
                                handled_success.insert(item.index);
                                if sender
                                    .send(GenerationOutcome::ImageSuccess {
                                        local_path: local_path.display().to_string(),
                                        display_prompt: source_prompt_for_result.clone(),
                                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                        upscale_done: true,
                                        delivery: delivery_confirmation_for_item(
                                            &request.client_request_id,
                                            &detail,
                                            item.index,
                                        ),
                                    })
                                    .is_err()
                                {
                                    let _ = fs::remove_file(local_path);
                                    return;
                                }
                            }
                            Err(error) if detail.terminal() => {
                                handled_failure.insert(item.index);
                                let (reason, delivery) = match failed_delivery_confirmation_for_item(
                                    &worker_scope,
                                    &request.client_request_id,
                                    &detail,
                                    item.index,
                                    None,
                                ) {
                                    Ok(delivery) => (error.generation_message(), Some(delivery)),
                                    Err(_) => (
                                        "本地生成恢复记录无法安全更新，已暂停交付，请重启后重试"
                                            .to_string(),
                                        None,
                                    ),
                                };
                                let _ = sender.send(GenerationOutcome::ImageFailure {
                                    reason,
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                    delivery,
                                });
                            }
                            Err(_) => {}
                        }
                    }
                } else if matches!(item.status.as_str(), "failed" | "cancelled")
                    && handled_failure.insert(item.index)
                {
                    let reason = item
                        .failure
                        .as_ref()
                        .map(TaskFailure::generation_message)
                        .unwrap_or_else(|| "服务端未能放大该图片".to_string());
                    let _ = sender.send(GenerationOutcome::ImageFailure {
                        reason,
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                        delivery: None,
                    });
                }
            }
            if detail.terminal() {
                report_unhandled_terminal_failures(
                    &sender,
                    &detail,
                    1,
                    &handled_success,
                    &mut handled_failure,
                    "服务端未能放大该图片",
                );
                let expected_success_count = detail.success_count.max(0) as usize;
                if !matches!(
                    update_pending_generation_scoped(
                        &worker_scope.owner_user_id,
                        worker_scope.auth_epoch,
                        &request.client_request_id,
                        |record| {
                            record.terminal = true;
                            record.expected_success_count = expected_success_count;
                        },
                    ),
                    Ok(true)
                ) {
                    return;
                }
                let _ = sender.send(GenerationOutcome::Finished);
                return;
            }
            std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
            detail = match api.task_scoped(&task_id, &worker_scope) {
                Ok(detail) => detail,
                Err(error) => {
                    if !backend_generation_scope_active(&backend, &worker_scope) {
                        return;
                    }
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
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
        raw_prompt,
        source_category,
        source.kind,
        ratio,
        billing_quality,
        state.get_image_model().to_string(),
        "generation".to_string(),
        conversation_id,
        false,
        source_reference_paths,
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
            reference_paths: vec![reference.source_path.clone()],
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
        reference_paths: item.reference_paths.clone(),
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
    let source_path = Path::new(source.source_path.trim());
    if source_path.is_file() {
        return inspect_image_dimensions(source_path).ok();
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

fn upscale_upload_path(
    _app: &AppWindow,
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
    // This is a recoverable upload input, not a user work. Keep it in the fixed managed
    // subtree so later cleanup can never reach a user-selected output directory.
    let dir = managed_upscale_input_dir();
    if !ensure_managed_subdirectory(&dir) {
        return Err(anyhow!("无法创建安全的放大暂存目录"));
    }
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
    backend: &BackendRuntime,
    api: &GenerationApi,
    session_scope: &SessionScope,
    client_request_id: &str,
    uploaded_file_ids: &[String],
    server_task_id: Option<&str>,
    cancellations: &Arc<Mutex<BTreeSet<String>>>,
) -> bool {
    if !backend_generation_scope_active(backend, session_scope) {
        return false;
    }
    if let Some(task_id) = server_task_id {
        let _ = api.cancel_scoped(task_id, session_scope);
    } else {
        for file_id in uploaded_file_ids {
            let _ = api.delete_reference_scoped(file_id, session_scope);
        }
    }
    if !backend_generation_scope_active(backend, session_scope)
        || !matches!(
            remove_pending_generation_scoped(
                &session_scope.owner_user_id,
                session_scope.auth_epoch,
                client_request_id,
            ),
            Ok(true)
        )
    {
        return false;
    }
    if let Ok(mut items) = cancellations.lock() {
        items.remove(client_request_id);
    }
    true
}

fn cleanup_image_edit_input_path(path: &Path) {
    let directory = managed_image_edit_input_dir();
    if safe_managed_subdirectory(&directory)
        && is_managed_image_edit_input_path(path)
        && fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_file() && !metadata.file_type().is_symlink()
        })
    {
        let _ = fs::remove_file(path);
    }
}

fn managed_image_edit_input_dir() -> PathBuf {
    app_data_dir().join("out").join("image-edit-inputs")
}

fn is_managed_image_edit_input_path(path: &Path) -> bool {
    let safe_parent = path.parent() == Some(managed_image_edit_input_dir().as_path());
    safe_parent && is_image_edit_input_name(path)
}

fn is_image_edit_input_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| {
            name.ends_with("-source.png")
                || name.contains("-source-") && name.ends_with(".png")
                || name.ends_with("-mask.png")
                || name.contains("-mask-") && name.ends_with(".png")
        })
}

fn managed_upscale_input_dir() -> PathBuf {
    app_data_dir().join("out").join("upscale-references")
}

fn is_managed_upscale_input_path(path: &Path) -> bool {
    path.parent() == Some(managed_upscale_input_dir().as_path()) && is_upscale_input_name(path)
}

fn is_upscale_input_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.contains("-upscale-source") && name.ends_with(".png"))
}

fn cleanup_upscale_input_path(path: &Path) {
    let directory = managed_upscale_input_dir();
    if safe_managed_subdirectory(&directory)
        && is_managed_upscale_input_path(path)
        && fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_file() && !metadata.file_type().is_symlink()
        })
    {
        let _ = fs::remove_file(path);
    }
}

fn cleanup_image_edit_record_inputs(record: &PendingGenerationRecord) {
    if record.task_type != "image_edit" {
        return;
    }
    for path in &record.reference_paths {
        cleanup_image_edit_input_path(Path::new(path));
    }
}

fn cleanup_upscale_record_inputs(record: &PendingGenerationRecord) {
    if record.task_type != "image_upscale" {
        return;
    }
    for path in &record.reference_paths {
        cleanup_upscale_input_path(Path::new(path));
    }
}

fn cleanup_generation_record_inputs(record: &PendingGenerationRecord) {
    cleanup_image_edit_record_inputs(record);
    cleanup_upscale_record_inputs(record);
}

fn release_recovered_upscale_inputs(
    record: &mut PendingGenerationRecord,
    session_scope: &SessionScope,
) -> bool {
    if record.task_type != "image_upscale" || record.reference_paths.is_empty() {
        return true;
    }
    if !matches!(
        update_pending_generation_scoped(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            &record.client_request_id,
            |stored| {
                stored.reference_paths.clear();
                stored.reference_sha256.clear();
                stored.reference_size_bytes.clear();
            },
        ),
        Ok(true)
    ) {
        return false;
    }
    cleanup_upscale_record_inputs(record);
    record.reference_paths.clear();
    record.reference_sha256.clear();
    record.reference_size_bytes.clear();
    true
}

pub(super) fn recover_pending_generations(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if state.get_session_state().as_str() != "online" {
        return;
    }
    let Some(session_scope) = current_generation_session_scope(&context) else {
        return;
    };
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let api = GenerationApi::new(backend.api.clone());
    let recovery_candidates = match load_generation_recovery_candidates_checked(
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    ) {
        Ok(records) => records,
        Err(error) => {
            state.set_generation_status(
                format!(
                    "生图任务恢复文件无法读取，原文件已保留；请勿重复提交付费任务并联系客服：{error}"
                )
                .into(),
            );
            return;
        }
    };
    let known_server_ids = recovery_candidates
        .iter()
        .filter(|record| !record.server_task_id.is_empty())
        .map(|record| record.server_task_id.clone())
        .collect::<BTreeSet<_>>();
    let mut local_records = Vec::new();
    for record in recovery_candidates {
        let candidate = match bind_generation_recovery_candidate(&api, &session_scope, record) {
            Ok(candidate) => candidate,
            Err(error) => {
                if !generation_scope_allows_polling(&app.as_weak(), &context, &session_scope) {
                    return;
                }
                state.set_generation_status(
                    format!(
                        "暂时无法核实一条历史生图任务，恢复记录已保留且不会重复提交：{}",
                        error.user_message()
                    )
                    .into(),
                );
                continue;
            }
        };
        if !generation_scope_allows_polling(&app.as_weak(), &context, &session_scope) {
            return;
        }
        if let Some(record) = candidate {
            local_records.push(record);
        }
    }
    if !reconcile_recoverable_delivery_cards(app, &context, &session_scope) {
        return;
    }
    for record in local_records {
        if record.schema_version != 1 || record.client_request_id.trim().is_empty() {
            continue;
        }
        // A create response can be lost after the server has already reserved credits. Always
        // resume with the persisted client_request_id; server idempotency can return the original
        // task. Time alone is never proof that this paid recovery record is safe to delete.
        if record.task_type == "image_watermark_removal" {
            resume_pending_watermark_removal(app, context.clone(), record);
            continue;
        }
        if record.task_type == "image_enhancement" {
            resume_pending_image_enhancement(app, context.clone(), record);
            continue;
        }
        if record.task_type == "image_cutout" {
            resume_pending_image_cutout(app, context.clone(), record);
            continue;
        }
        if record.task_type == "image_colorization" {
            resume_pending_image_colorization(app, context.clone(), record);
            continue;
        }
        if category_is_generating(&context, &record.category) {
            continue;
        }
        resume_pending_generation(app, context.clone(), record);
    }
    recover_server_generation_tasks(app, context, session_scope, known_server_ids);
}

fn reconcile_recoverable_delivery_cards(
    app: &AppWindow,
    context: &AppContext,
    session_scope: &SessionScope,
) -> bool {
    let recoverable_ids = match recoverable_failed_asset_ids(
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    ) {
        Ok(ids) => ids,
        Err(_) => {
            let mut store = context.store.borrow_mut();
            for item in &mut store.generations {
                item.delivery_recoverable = false;
                item.delivery_downloading = false;
            }
            push_all(app, &store);
            app.global::<AppState>().set_generation_status(
                "本地生成恢复记录无法安全读取，已暂停恢复下载，请重启后重试".into(),
            );
            return false;
        }
    };
    let mut store = context.store.borrow_mut();
    for item in &mut store.generations {
        item.delivery_recoverable = recoverable_ids.contains(&item.id);
        item.delivery_downloading = false;
    }
    push_all(app, &store);
    true
}

fn bind_generation_recovery_candidate(
    api: &GenerationApi,
    session_scope: &SessionScope,
    mut record: PendingGenerationRecord,
) -> std::result::Result<Option<PendingGenerationRecord>, ApiError> {
    if record.owner_user_id == session_scope.owner_user_id {
        if record.auth_epoch == session_scope.auth_epoch {
            return Ok(Some(record));
        }
        if !record.server_task_id.is_empty() {
            api.task_scoped(&record.server_task_id, session_scope)?;
        }
        let old_epoch = record.auth_epoch;
        if !matches!(
            rebind_pending_generation_epoch(
                &session_scope.owner_user_id,
                old_epoch,
                session_scope.auth_epoch,
                &record.client_request_id,
            ),
            Ok(true)
        ) {
            return Ok(None);
        }
        record.auth_epoch = session_scope.auth_epoch;
        return Ok(Some(record));
    }
    if !record.owner_user_id.is_empty() || record.server_task_id.is_empty() {
        return Ok(None);
    }
    api.task_scoped(&record.server_task_id, session_scope)?;
    if !matches!(
        claim_legacy_pending_generation(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            &record.client_request_id,
            &record.server_task_id,
        ),
        Ok(true)
    ) {
        return Ok(None);
    }
    record.owner_user_id = session_scope.owner_user_id.clone();
    record.auth_epoch = session_scope.auth_epoch;
    Ok(Some(record))
}

const ORPHANED_GENERATION_INPUT_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Default, Deserialize)]
struct GenerationCleanupSnapshot {
    #[serde(default)]
    generations: Vec<PendingGenerationRecord>,
}

fn load_generation_cleanup_snapshot() -> Result<Vec<PendingGenerationRecord>> {
    let path = generation_recovery_path();
    restore_json_backup_if_needed(&path);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    };
    Ok(serde_json::from_str::<GenerationCleanupSnapshot>(&text)
        .context("pending generation cleanup snapshot is invalid")?
        .generations)
}

fn cleanup_path_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn retained_generation_paths(records: &[PendingGenerationRecord]) -> BTreeSet<PathBuf> {
    records
        .iter()
        .flat_map(|record| {
            record.reference_paths.iter().map(String::as_str).chain(
                record
                    .deliveries
                    .iter()
                    .map(|delivery| delivery.local_path.as_str()),
            )
        })
        .filter(|path| !path.trim().is_empty())
        .map(Path::new)
        .map(cleanup_path_identity)
        .collect()
}

fn retained_task_input_paths(
    records: &[PendingGenerationRecord],
    task_type: &str,
) -> BTreeSet<PathBuf> {
    records
        .iter()
        .filter(|record| record.task_type == task_type)
        .flat_map(|record| record.reference_paths.iter())
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .map(|path| cleanup_path_identity(&path))
        .collect()
}

fn cleanup_orphaned_input_directory(
    directory: &Path,
    retained: &BTreeSet<PathBuf>,
    now: std::time::SystemTime,
    managed_name: impl Fn(&Path) -> bool,
) {
    let Ok(directory_metadata) = fs::symlink_metadata(directory) else {
        return;
    };
    if !directory_metadata.file_type().is_dir() || directory_metadata.file_type().is_symlink() {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !managed_name(&path) || retained.contains(&cleanup_path_identity(&path)) {
            continue;
        }
        let stale = fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= ORPHANED_GENERATION_INPUT_GRACE);
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}

fn cleanup_orphaned_image_edit_inputs(_app: &AppWindow, records: &[PendingGenerationRecord]) {
    let directory = managed_image_edit_input_dir();
    if !safe_managed_subdirectory(&directory) {
        return;
    }
    cleanup_orphaned_input_directory(
        &directory,
        &retained_task_input_paths(records, "image_edit"),
        std::time::SystemTime::now(),
        is_image_edit_input_name,
    );
}

fn cleanup_orphaned_upscale_inputs(records: &[PendingGenerationRecord]) {
    let directory = managed_upscale_input_dir();
    if !safe_managed_subdirectory(&directory) {
        return;
    }
    cleanup_orphaned_input_directory(
        &directory,
        &retained_task_input_paths(records, "image_upscale"),
        std::time::SystemTime::now(),
        is_upscale_input_name,
    );
}

pub(super) fn cleanup_generation_transients_at_startup(app: &AppWindow) {
    let records = load_generation_cleanup_snapshot();
    let retained = records
        .as_ref()
        .ok()
        .map(|records| retained_generation_paths(records));
    // System reference-upload cleanup remains safe when recovery JSON is unreadable. App-data
    // cleanup receives None and fails closed so no pending task input can be lost.
    cleanup_stale_generation_transients(retained.as_ref());
    let Ok(records) = records else {
        return;
    };
    cleanup_orphaned_image_edit_inputs(app, &records);
    cleanup_orphaned_upscale_inputs(&records);
}

fn recover_server_generation_tasks(
    app: &AppWindow,
    context: AppContext,
    session_scope: SessionScope,
    known_server_ids: BTreeSet<String>,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let (sender, receiver) =
        mpsc::channel::<std::result::Result<Vec<PendingGenerationRecord>, ()>>();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        let mut recovered = Vec::new();
        for status in ["queued", "processing", "completed", "partially_completed"] {
            let summaries = match api.list_tasks_scoped(status, &worker_scope) {
                Ok(summaries) => summaries,
                Err(_) if !backend_generation_scope_active(&backend, &worker_scope) => {
                    let _ = sender.send(Err(()));
                    return;
                }
                Err(_) => continue,
            };
            for summary in summaries {
                if !matches!(
                    summary.task_type.as_str(),
                    "image_generation"
                        | "image_upscale"
                        | "image_edit"
                        | "image_watermark_removal"
                        | "image_cutout"
                        | "image_colorization"
                ) || known_server_ids.contains(&summary.id)
                {
                    continue;
                }
                let detail = match api.task_scoped(&summary.id, &worker_scope) {
                    Ok(detail) => detail,
                    Err(_) if !backend_generation_scope_active(&backend, &worker_scope) => {
                        let _ = sender.send(Err(()));
                        return;
                    }
                    Err(_) => continue,
                };
                if detail.terminal()
                    && !detail.items.iter().any(|item| {
                        item.file
                            .as_ref()
                            .and_then(|file| file.download_url.as_ref())
                            .is_some()
                    })
                {
                    continue;
                }
                let toolbox_enhancement = summary.task_type == "image_upscale"
                    && detail
                        .model
                        .as_ref()
                        .is_some_and(|model| model.code == "aliyun_super_resolution");
                let toolbox_colorization = summary.task_type == "image_colorization"
                    && detail
                        .model
                        .as_ref()
                        .is_some_and(|model| model.code == "aliyun_image_colorization");
                let recovered_task_type = if toolbox_enhancement {
                    "image_enhancement".to_string()
                } else if toolbox_colorization {
                    "image_colorization".to_string()
                } else {
                    summary.task_type.clone()
                };
                let recovered_quality = if toolbox_enhancement {
                    let target_long_edge = detail
                        .request
                        .get("target_width")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        .max(
                            detail
                                .request
                                .get("target_height")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                        );
                    if target_long_edge > 2048 { "4K" } else { "2K" }.to_string()
                } else if summary.task_type == "image_cutout" {
                    detail
                        .request
                        .get("subject_type")
                        .and_then(Value::as_str)
                        .unwrap_or("general")
                        .to_string()
                } else {
                    detail.quality.clone()
                };
                let prompt = detail
                    .prompt
                    .clone()
                    .unwrap_or_else(|| "恢复的生成任务".to_string());
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
                    owner_user_id: worker_scope.owner_user_id.clone(),
                    auth_epoch: worker_scope.auth_epoch,
                    local_task_id: Uuid::new_v4().to_string(),
                    server_task_id: detail.id.clone(),
                    raw_prompt: prompt.clone(),
                    generation_prompt: prompt,
                    task_type: recovered_task_type,
                    category: if summary.task_type == "image_watermark_removal"
                        || summary.task_type == "image_cutout"
                        || summary.task_type == "image_edit"
                        || toolbox_enhancement
                        || toolbox_colorization
                    {
                        "other".to_string()
                    } else {
                        "character".to_string()
                    },
                    mode: "game".to_string(),
                    ratio,
                    quality: recovered_quality,
                    model_code: detail
                        .model
                        .as_ref()
                        .map(|model| model.code.clone())
                        .unwrap_or_default(),
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
                    create_conversation: summary.task_type != "image_watermark_removal"
                        && summary.task_type != "image_cutout"
                        && summary.task_type != "image_edit"
                        && !toolbox_enhancement
                        && !toolbox_colorization,
                    reference_paths: vec![],
                    reference_sha256: vec![],
                    reference_size_bytes: vec![],
                    lineage_reference_paths: vec![],
                    uploaded_file_ids: vec![],
                    deliveries: vec![],
                    terminal: detail.terminal(),
                    expected_success_count: detail.success_count.max(0) as usize,
                    canvas_source_node_id: String::new(),
                    canvas_ui_extraction: false,
                });
            }
        }
        let _ = sender.send(Ok(recovered));
    });
    poll_server_generation_recovery(
        app.as_weak(),
        context,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_server_generation_recovery(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<Vec<PendingGenerationRecord>, ()>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(()))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_server_generation_recovery(app_weak, context, session_scope, receiver);
            return;
        };
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let Ok(records) = outcome else {
            receiver.borrow_mut().take();
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        for record in records {
            if upsert_pending_generation_scoped(
                record.clone(),
                &session_scope.owner_user_id,
                session_scope.auth_epoch,
            )
            .is_err()
            {
                continue;
            }
            if record.task_type == "image_watermark_removal" {
                resume_pending_watermark_removal(&app, context.clone(), record);
                continue;
            }
            if record.task_type == "image_enhancement" {
                resume_pending_image_enhancement(&app, context.clone(), record);
                continue;
            }
            if record.task_type == "image_cutout" {
                resume_pending_image_cutout(&app, context.clone(), record);
                continue;
            }
            if record.task_type == "image_colorization" {
                resume_pending_image_colorization(&app, context.clone(), record);
                continue;
            }
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
    if record.canvas_ui_extraction {
        let _ = remove_pending_generation_scoped(
            &record.owner_user_id,
            record.auth_epoch,
            &record.client_request_id,
        );
        return;
    }
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !generation_scope_matches_context(&context, &session_scope) {
        return;
    }
    let saved_count = record
        .deliveries
        .iter()
        .filter(|item| !item.local_path.is_empty() && Path::new(&item.local_path).is_file())
        .count() as i32;
    let is_canvas_generation = !record.canvas_source_node_id.is_empty();
    insert_active_generation(
        &context,
        ActiveGeneration {
            task_id: record.local_task_id.clone(),
            client_request_id: Some(record.client_request_id.clone()),
            server_task_id: (!record.server_task_id.is_empty())
                .then(|| record.server_task_id.clone()),
            category: record.category.clone(),
            conversation_id: record.conversation_id.clone(),
            prompt: record.raw_prompt.clone(),
            credit_cost: 0,
            total_count: record.count,
            loading_count: (record.count - saved_count).max(0),
            completed_count: saved_count,
            success_count: saved_count,
            failed_count: 0,
            last_failure_reason: None,
            progress: if saved_count > 0 { 50 } else { 1 },
            eta: 0,
            latest_success_id: None,
            session_scope: session_scope.clone(),
            destination: if record.canvas_source_node_id.is_empty() {
                GenerationDestination::Gallery
            } else {
                GenerationDestination::Canvas {
                    source_node_id: record.canvas_source_node_id.clone(),
                }
            },
        },
    );
    let state = app.global::<AppState>();
    if record.create_conversation
        && !state
            .get_conversations()
            .iter()
            .any(|item| item.id.as_str() == record.conversation_id)
    {
        let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
        conversations.insert(
            0,
            ConversationItem {
                id: record.conversation_id.clone().into(),
                title: short_text(&record.raw_prompt, 10).into(),
                image: Image::default(),
                loading: true,
            },
        );
        state.set_conversations(ModelRc::new(VecModel::from(conversations)));
    }
    set_generation_status_for_category(&context, app, &record.category, "正在恢复未完成任务...");
    sync_generation_state_for_current_category(&context, app);

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let worker_record = record.clone();
    let generation_reference_paths = if !record.lineage_reference_paths.is_empty() {
        record.lineage_reference_paths.clone()
    } else if matches!(record.task_type.as_str(), "image_edit" | "image_upscale") {
        Vec::new()
    } else {
        record.reference_paths.clone()
    };
    let result_origin = if record.task_type == "image_edit" {
        "image_edit"
    } else {
        "generation"
    }
    .to_string();
    let cancellations = context.cancelled_generation_requests.clone();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        run_recovered_generation_worker(backend, worker_scope, worker_record, sender, cancellations)
    });
    poll_generation_stream(
        app.as_weak(),
        context,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
        record.raw_prompt,
        record.category,
        record.mode,
        record.ratio,
        record.quality,
        record.model_code,
        result_origin,
        record.conversation_id,
        record.create_conversation,
        generation_reference_paths,
        vec![],
        QuoteContext {
            title: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            width: 0,
            height: 0,
        },
        !is_canvas_generation,
        record.local_task_id,
        Instant::now(),
    );
}

fn run_recovered_generation_worker(
    backend: Arc<BackendRuntime>,
    session_scope: SessionScope,
    mut record: PendingGenerationRecord,
    sender: mpsc::Sender<GenerationOutcome>,
    cancellations: Arc<Mutex<BTreeSet<String>>>,
) {
    if record.owner_user_id != session_scope.owner_user_id
        || record.auth_epoch != session_scope.auth_epoch
        || !backend_generation_scope_active(&backend, &session_scope)
    {
        return;
    }
    let api = GenerationApi::new(backend.api.clone());
    let mut uploaded = record.uploaded_file_ids.clone();
    if record.server_task_id.is_empty() && !generation_references_match(&record) {
        let _ = sender.send(GenerationOutcome::Failure {
            reason: "参考图内容已变化，恢复任务已暂停，请重新发起".to_string(),
            time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        });
        return;
    }
    if record.task_type == "image_edit" && !record.server_task_id.is_empty() {
        if !matches!(
            update_pending_generation_scoped(
                &session_scope.owner_user_id,
                session_scope.auth_epoch,
                &record.client_request_id,
                |item| item.reference_paths.clear(),
            ),
            Ok(true)
        ) {
            return;
        }
        cleanup_image_edit_record_inputs(&record);
        record.reference_paths.clear();
    }
    if record.task_type == "image_upscale"
        && (!record.server_task_id.is_empty()
            || (!record.reference_paths.is_empty()
                && uploaded.len() >= record.reference_paths.len()))
        && !release_recovered_upscale_inputs(&mut record, &session_scope)
    {
        return;
    }
    for path in record.reference_paths.iter().skip(uploaded.len()) {
        let uploaded_reference = if record.task_type == "image_edit" {
            api.upload_prepared_reference_scoped(Path::new(path), &session_scope)
        } else {
            api.upload_reference_scoped(Path::new(path), &session_scope)
        };
        match uploaded_reference {
            Ok(file_id) => {
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                if !matches!(
                    update_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                        |item| item.uploaded_file_ids = snapshot,
                    ),
                    Ok(true)
                ) {
                    if let Some(file_id) = uploaded.last() {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    return;
                }
                if generation_cancel_requested(&cancellations, &record.client_request_id) {
                    if cleanup_cancelled_generation(
                        &backend,
                        &api,
                        &session_scope,
                        &record.client_request_id,
                        &uploaded,
                        None,
                        &cancellations,
                    ) {
                        cleanup_generation_record_inputs(&record);
                    }
                    return;
                }
            }
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复参考图上传失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
    }
    if record.task_type == "image_upscale"
        && !record.reference_paths.is_empty()
        && uploaded.len() >= record.reference_paths.len()
        && !release_recovered_upscale_inputs(&mut record, &session_scope)
    {
        return;
    }
    let task_type = if record.task_type.trim().is_empty() {
        "image_generation"
    } else {
        record.task_type.as_str()
    };
    let aspect_ratio = api_aspect_ratio(&record.ratio);
    if generation_cancel_requested(&cancellations, &record.client_request_id) {
        if cleanup_cancelled_generation(
            &backend,
            &api,
            &session_scope,
            &record.client_request_id,
            &uploaded,
            None,
            &cancellations,
        ) {
            cleanup_generation_record_inputs(&record);
        }
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
            api.create_upscale_task_scoped(&request, &session_scope)
        } else if task_type == "image_edit" {
            if uploaded.len() != 2 {
                if !matches!(
                    remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    ),
                    Ok(true)
                ) {
                    return;
                }
                for file_id in &uploaded {
                    let _ = api.delete_reference_scoped(file_id, &session_scope);
                }
                cleanup_image_edit_record_inputs(&record);
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: "图片编辑恢复数据不完整：缺少原图或遮罩".to_string(),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
            let request = CreateImageEditTask {
                client_request_id: record.client_request_id.clone(),
                task_type: "image_edit".to_string(),
                model_code: record.model_code.clone(),
                prompt: record.generation_prompt.clone(),
                quality: record.quality.clone(),
                aspect_ratio: aspect_ratio.clone(),
                source_file_id: uploaded[0].clone(),
                mask_file_id: uploaded[1].clone(),
            };
            api.create_image_edit_task_scoped(&request, &session_scope)
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
            api.create_task_scoped(&request, &session_scope)
        };
        match created {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    if !matches!(
                        remove_pending_generation_scoped(
                            &session_scope.owner_user_id,
                            session_scope.auth_epoch,
                            &record.client_request_id,
                        ),
                        Ok(true)
                    ) {
                        return;
                    }
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    cleanup_generation_record_inputs(&record);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次生图，请前往充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    if !matches!(
                        remove_pending_generation_scoped(
                            &session_scope.owner_user_id,
                            session_scope.auth_epoch,
                            &record.client_request_id,
                        ),
                        Ok(true)
                    ) {
                        return;
                    }
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    cleanup_generation_record_inputs(&record);
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务提交失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
    } else {
        match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务查询失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        }
    };
    if generation_cancel_requested(&cancellations, &record.client_request_id) {
        if cleanup_cancelled_generation(
            &backend,
            &api,
            &session_scope,
            &record.client_request_id,
            &uploaded,
            Some(&detail.id),
            &cancellations,
        ) {
            cleanup_generation_record_inputs(&record);
        }
        return;
    }
    record.server_task_id = detail.id.clone();
    let server_task_id = detail.id.clone();
    let uploaded_snapshot = uploaded.clone();
    let server_id_snapshot = server_task_id.clone();
    if !matches!(
        update_pending_generation_scoped(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            &record.client_request_id,
            |item| {
                item.server_task_id = server_id_snapshot;
                item.uploaded_file_ids = uploaded_snapshot;
                if item.task_type == "image_edit" {
                    item.reference_paths.clear();
                }
            },
        ),
        Ok(true)
    ) {
        return;
    }
    cleanup_generation_record_inputs(&record);
    record.reference_paths.clear();
    let _ = sender.send(GenerationOutcome::Accepted {
        task_id: server_task_id.clone(),
    });

    let verified_delivery_file_ids = match sanitize_recovered_delivery_paths(&mut record) {
        Ok(file_ids) => file_ids,
        Err(_) => {
            let _ = sender.send(GenerationOutcome::Failure {
                reason: "本地生成恢复记录无法安全更新，已暂停交付，请重启后重试".to_string(),
                time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
            });
            return;
        }
    };
    let mut handled_success = record
        .deliveries
        .iter()
        .filter(|item| verified_delivery_file_ids.contains(&item.file_id))
        .map(|item| item.item_index)
        .collect::<BTreeSet<_>>();
    let mut handled_failure = BTreeSet::new();
    for delivery in &record.deliveries {
        if !recovered_delivery_ready_for_ack(delivery, &verified_delivery_file_ids) {
            continue;
        }
        if api
            .acknowledge_delivery_scoped(
                &server_task_id,
                &delivery.file_id,
                &delivery.sha256,
                delivery.size_bytes,
                &session_scope,
            )
            .is_ok()
        {
            let _ = pending_delivery_acknowledged(
                &session_scope.owner_user_id,
                session_scope.auth_epoch,
                &record.client_request_id,
                &delivery.file_id,
            );
        }
    }

    loop {
        if !backend_generation_scope_active(&backend, &session_scope) {
            return;
        }
        if generation_cancel_requested(&cancellations, &record.client_request_id) {
            cleanup_cancelled_generation(
                &backend,
                &api,
                &session_scope,
                &record.client_request_id,
                &[],
                Some(&server_task_id),
                &cancellations,
            );
            return;
        }
        let _ = sender.send(GenerationOutcome::Progress {
            percent: detail.progress_percent,
        });
        for item in &detail.items {
            if item.status == "succeeded" && !handled_success.contains(&item.index) {
                if let Some(file) = item.file.as_ref() {
                    let local_path = generation_download_staging_path(
                        &record.client_request_id,
                        item.index,
                        file,
                    );
                    match api.download_verified_to_path_scoped(file, &session_scope, &local_path) {
                        Ok(()) => {
                            handled_success.insert(item.index);
                            if sender
                                .send(GenerationOutcome::ImageSuccess {
                                    local_path: local_path.display().to_string(),
                                    display_prompt: record.raw_prompt.clone(),
                                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                    upscale_done: record.task_type == "image_upscale",
                                    delivery: delivery_confirmation_for_item(
                                        &record.client_request_id,
                                        &detail,
                                        item.index,
                                    )
                                    .map(|mut delivery| {
                                        delivery.failed_asset_id =
                                            failed_asset_id_for_delivery(&record, &file.id);
                                        delivery
                                    }),
                                })
                                .is_err()
                            {
                                let _ = fs::remove_file(local_path);
                                return;
                            }
                        }
                        Err(error) if detail.terminal() => {
                            handled_failure.insert(item.index);
                            let existing_failed_asset_id =
                                failed_asset_id_for_delivery(&record, &file.id);
                            let (reason, delivery) = match failed_delivery_confirmation_for_item(
                                &session_scope,
                                &record.client_request_id,
                                &detail,
                                item.index,
                                existing_failed_asset_id.as_deref(),
                            ) {
                                Ok(delivery) => (error.generation_message(), Some(delivery)),
                                Err(_) => (
                                    "本地生成恢复记录无法安全更新，已暂停交付，请重启后重试"
                                        .to_string(),
                                    None,
                                ),
                            };
                            let _ = sender.send(GenerationOutcome::ImageFailure {
                                reason,
                                time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                                delivery,
                            });
                        }
                        Err(_) => {}
                    }
                }
            } else if matches!(item.status.as_str(), "failed" | "cancelled")
                && handled_failure.insert(item.index)
            {
                let _ = sender.send(GenerationOutcome::ImageFailure {
                    reason: item
                        .failure
                        .as_ref()
                        .map(TaskFailure::generation_message)
                        .unwrap_or_else(|| "服务端未能生成该图片".to_string()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    delivery: None,
                });
            }
        }
        if detail.terminal() {
            report_unhandled_terminal_failures(
                &sender,
                &detail,
                record.count.max(1) as usize,
                &handled_success,
                &mut handled_failure,
                "服务端未能生成该图片",
            );
            let expected = detail.success_count.max(0) as usize;
            if !matches!(
                update_pending_generation_scoped(
                    &session_scope.owner_user_id,
                    session_scope.auth_epoch,
                    &record.client_request_id,
                    |item| {
                        item.terminal = true;
                        item.expected_success_count = expected;
                    },
                ),
                Ok(true)
            ) {
                return;
            }
            let _ = sender.send(GenerationOutcome::Finished);
            return;
        }
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        detail = match api.task_scoped(&server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                let _ = sender.send(GenerationOutcome::Failure {
                    reason: format!("恢复任务轮询失败：{}", error.generation_message()),
                    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                });
                return;
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed_generation_task(code: &str, message: &str) -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "failed-task".to_string(),
            status: "failed".to_string(),
            progress_percent: 100,
            success_count: 0,
            failure_count: 2,
            failure: Some(TaskFailure {
                code: code.to_string(),
                message: message.to_string(),
            }),
            prompt: None,
            result_prompt: None,
            request: serde_json::Value::Null,
            model: None,
            quality: "1K".to_string(),
            requested_count: 2,
            task_type: "image_generation".to_string(),
            items: Vec::new(),
        }
    }

    fn completed_task_with_available_file(file_id: &str) -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "completed-task".to_string(),
            status: "completed".to_string(),
            progress_percent: 100,
            success_count: 1,
            failure_count: 0,
            failure: None,
            prompt: None,
            result_prompt: None,
            request: serde_json::Value::Null,
            model: None,
            quality: "1K".to_string(),
            requested_count: 1,
            task_type: "image_generation".to_string(),
            items: vec![GenerationTaskItem {
                index: 0,
                status: "succeeded".to_string(),
                credit_cost: "0".to_string(),
                failure: None,
                file: Some(TaskOutputFile {
                    id: file_id.to_string(),
                    status: "available".to_string(),
                    mime_type: "image/png".to_string(),
                    size_bytes: "3".to_string(),
                    sha256: "abc".to_string(),
                    width: Some(1),
                    height: Some(1),
                    download_url: Some("https://example.invalid/file.png".to_string()),
                }),
            }],
        }
    }

    #[test]
    fn succeeded_item_download_error_keeps_delivery_identity() {
        let detail = completed_task_with_available_file("file-1");
        let delivery = delivery_confirmation_for_item("request-1", &detail, 0).unwrap();

        assert_eq!(delivery.file_id, "file-1");
        assert_eq!(delivery.item_index, 0);
    }

    #[test]
    fn failed_provider_item_has_no_recoverable_delivery() {
        let detail = failed_generation_task("provider_error", "failed");

        assert!(delivery_confirmation_for_item("request-1", &detail, 0).is_none());
    }

    #[test]
    fn task_level_policy_block_reports_every_missing_image() {
        let (sender, receiver) = mpsc::channel();
        let detail = failed_generation_task(
            "content_policy_violation",
            "生成内容违反了关于裸露内容的防护规则",
        );
        let mut handled_failure = BTreeSet::new();

        report_unhandled_terminal_failures(
            &sender,
            &detail,
            2,
            &BTreeSet::new(),
            &mut handled_failure,
            "服务端未能生成该图片",
        );

        let outcomes = receiver.try_iter().collect::<Vec<_>>();
        assert_eq!(outcomes.len(), 2);
        for outcome in outcomes {
            let GenerationOutcome::ImageFailure { reason, .. } = outcome else {
                panic!("expected an image failure");
            };
            assert!(reason.contains("上游安全系统拦截"));
            assert!(reason.contains("不返还积分"));
        }
    }

    fn test_png_bytes() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 255]));
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut output, image::ImageFormat::Png)
            .expect("encode test png");
        output.into_inner()
    }

    fn delivery(file_id: &str, path: &Path, bytes: &[u8]) -> PendingDeliveryRecord {
        PendingDeliveryRecord {
            item_index: 0,
            file_id: file_id.to_string(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size_bytes: bytes.len() as u64,
            local_path: path.display().to_string(),
            acknowledged: false,
            failed_asset_id: String::new(),
            abandoned: false,
        }
    }

    fn recovery_record(deliveries: Vec<PendingDeliveryRecord>) -> PendingGenerationRecord {
        PendingGenerationRecord {
            schema_version: 1,
            created_at_epoch_ms: Local::now().timestamp_millis(),
            client_request_id: "delivery_test_request".to_string(),
            owner_user_id: "delivery-test-user".to_string(),
            auth_epoch: 7,
            local_task_id: "local-task".to_string(),
            server_task_id: "server-task".to_string(),
            raw_prompt: "prompt".to_string(),
            generation_prompt: "prompt".to_string(),
            task_type: "image_generation".to_string(),
            category: "other".to_string(),
            mode: "game".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model_code: "openai_image".to_string(),
            conversation_id: "conversation".to_string(),
            count: 1,
            target_width: 0,
            target_height: 0,
            create_conversation: false,
            reference_paths: vec![],
            reference_sha256: vec![],
            reference_size_bytes: vec![],
            lineage_reference_paths: vec![],
            uploaded_file_ids: vec![],
            deliveries,
            terminal: true,
            expected_success_count: 1,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }

    #[test]
    fn image_edit_cleanup_is_limited_to_managed_input_files() {
        let directory = managed_image_edit_input_dir();
        let managed = directory.join("20260810-source.png");
        let managed_unique = directory.join("20260810-mask-2.png");
        let wrong_parent = app_data_dir().join("20260810-source.png");
        let wrong_name = directory.join("user-image.png");

        assert!(is_managed_image_edit_input_path(&managed));
        assert!(is_managed_image_edit_input_path(&managed_unique));
        assert!(!is_managed_image_edit_input_path(&wrong_parent));
        assert!(!is_managed_image_edit_input_path(&wrong_name));
        assert!(!is_managed_image_edit_input_path(Path::new(
            "/tmp/image-edit-inputs/20260810-source.png"
        )));
    }

    #[test]
    fn orphan_cleanup_retains_pending_inputs_and_ignores_unmanaged_names() {
        let directory = std::env::temp_dir().join(format!(
            "artforge-generation-input-cleanup-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("create cleanup directory");
        let retained = directory.join("20260810-source.png");
        let orphan = directory.join("20260810-mask.png");
        let unmanaged = directory.join("user-image.png");
        fs::write(&retained, b"retained").expect("write retained input");
        fs::write(&orphan, b"orphan").expect("write orphan input");
        fs::write(&unmanaged, b"user").expect("write unmanaged input");

        cleanup_orphaned_input_directory(
            &directory,
            &BTreeSet::from([cleanup_path_identity(&retained)]),
            std::time::SystemTime::now() + ORPHANED_GENERATION_INPUT_GRACE,
            is_image_edit_input_name,
        );

        assert!(retained.is_file());
        assert!(!orphan.exists());
        assert!(unmanaged.is_file());
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn orphan_cleanup_rejects_a_symlinked_input_directory() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "artforge-generation-input-symlink-{}",
            Uuid::new_v4()
        ));
        let target = root.join("user-works");
        let linked = root.join("image-edit-inputs");
        fs::create_dir_all(&target).expect("create target directory");
        let work = target.join("20260810-source.png");
        fs::write(&work, b"user work").expect("write user work");
        symlink(&target, &linked).expect("create input directory symlink");

        cleanup_orphaned_input_directory(
            &linked,
            &BTreeSet::new(),
            std::time::SystemTime::now() + ORPHANED_GENERATION_INPUT_GRACE,
            is_image_edit_input_name,
        );

        assert!(work.is_file());
        fs::remove_file(&linked).expect("remove input directory symlink");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn retained_cleanup_paths_include_every_account_and_delivery() {
        let mut first = recovery_record(Vec::new());
        first.owner_user_id = "user-a".to_string();
        first.task_type = "image_edit".to_string();
        first.reference_paths = vec!["/managed/edit-source.png".to_string()];
        let mut second = recovery_record(vec![PendingDeliveryRecord {
            local_path: "/managed/delivery.png".to_string(),
            ..PendingDeliveryRecord::default()
        }]);
        second.owner_user_id = "user-b".to_string();
        second.client_request_id = "other-request".to_string();
        second.task_type = "image_upscale".to_string();
        second.reference_paths = vec!["/managed/upscale-source.png".to_string()];

        let retained = retained_generation_paths(&[first, second]);

        assert!(retained.contains(Path::new("/managed/edit-source.png")));
        assert!(retained.contains(Path::new("/managed/upscale-source.png")));
        assert!(retained.contains(Path::new("/managed/delivery.png")));
    }

    #[test]
    fn recovered_delivery_requires_matching_size_sha256_and_decodable_image() {
        let directory =
            std::env::temp_dir().join(format!("artforge-delivery-validation-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create delivery validation directory");
        let valid_path = directory.join("valid.png");
        let invalid_path = directory.join("invalid.png");
        let valid_bytes = test_png_bytes();
        let invalid_bytes = b"not an encoded image";
        fs::write(&valid_path, &valid_bytes).expect("write valid delivery");
        fs::write(&invalid_path, invalid_bytes).expect("write invalid delivery");
        let valid_sha256 = format!("{:x}", Sha256::digest(&valid_bytes));
        let invalid_sha256 = format!("{:x}", Sha256::digest(invalid_bytes));

        assert!(recovered_delivery_path_matches(
            valid_path.to_str().expect("valid path"),
            &valid_sha256,
            valid_bytes.len() as u64,
        ));
        assert!(!recovered_delivery_path_matches(
            valid_path.to_str().expect("valid path"),
            &valid_sha256,
            valid_bytes.len() as u64 + 1,
        ));
        assert!(!recovered_delivery_path_matches(
            valid_path.to_str().expect("valid path"),
            &"0".repeat(64),
            valid_bytes.len() as u64,
        ));
        assert!(!recovered_delivery_path_matches(
            invalid_path.to_str().expect("invalid path"),
            &invalid_sha256,
            invalid_bytes.len() as u64,
        ));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn invalid_recovered_delivery_is_cleared_for_redownload_before_ack() {
        let directory =
            std::env::temp_dir().join(format!("artforge-delivery-redownload-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create delivery redownload directory");
        let valid_path = directory.join("valid.png");
        let corrupted_path = directory.join("corrupted.png");
        let valid_bytes = test_png_bytes();
        fs::write(&valid_path, &valid_bytes).expect("write valid delivery");
        fs::write(&corrupted_path, b"truncated").expect("write corrupted delivery");
        let valid = delivery("valid-file", &valid_path, &valid_bytes);
        let invalid = delivery("invalid-file", &corrupted_path, &valid_bytes);
        let mut record = recovery_record(vec![valid, invalid]);

        let verified = sanitize_recovered_delivery_paths_with(&mut record, |file_ids| {
            assert_eq!(file_ids, &BTreeSet::from(["invalid-file".to_string()]));
            Ok(true)
        })
        .expect("sanitize recovered deliveries");

        assert_eq!(verified, BTreeSet::from(["valid-file".to_string()]));
        assert!(!record.deliveries[0].local_path.is_empty());
        assert!(recovered_delivery_file_matches(&record.deliveries[0]));
        assert!(recovered_delivery_ready_for_ack(
            &record.deliveries[0],
            &verified
        ));
        assert!(record.deliveries[1].local_path.is_empty());
        assert!(!recovered_delivery_file_matches(&record.deliveries[1]));
        assert!(!recovered_delivery_ready_for_ack(
            &record.deliveries[1],
            &verified
        ));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn recovered_delivery_fails_closed_when_invalid_path_cannot_be_persisted() {
        let directory =
            std::env::temp_dir().join(format!("artforge-delivery-fail-closed-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create fail-closed directory");
        let corrupted_path = directory.join("corrupted.png");
        let expected_bytes = test_png_bytes();
        fs::write(&corrupted_path, b"truncated").expect("write corrupted delivery");
        let mut record = recovery_record(vec![delivery(
            "invalid-file",
            &corrupted_path,
            &expected_bytes,
        )]);
        let original_path = record.deliveries[0].local_path.clone();

        let result = sanitize_recovered_delivery_paths_with(&mut record, |_| Ok(false));

        assert!(result.is_err());
        assert_eq!(record.deliveries[0].local_path, original_path);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn every_toolbox_recovery_path_uses_shared_delivery_validation() {
        let cutout = include_str!("../callbacks/image_cutout.rs");
        let enhancement = include_str!("../callbacks/image_enhancement.rs");
        let toolbox = include_str!("../callbacks/toolbox.rs");

        assert_eq!(
            cutout
                .matches("sanitize_recovered_delivery_paths(&mut record)")
                .count(),
            1
        );
        assert_eq!(
            enhancement
                .matches("sanitize_recovered_delivery_paths(&mut record)")
                .count(),
            1
        );
        assert_eq!(
            toolbox
                .matches("sanitize_recovered_delivery_paths(&mut record)")
                .count(),
            2
        );
        for callback in [cutout, enhancement, toolbox] {
            assert!(callback.contains("recovered_delivery_path_matches("));
            assert!(callback.contains("clear_recovered_delivery_local_path("));
        }
    }
}
