use super::*;

pub(super) fn generation_download_staging_path(
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetryGenerationRecoveryCommitError {
    NewRecovery,
    OldDelivery,
    NewRecoveryRollback,
}

fn commit_retry_generation_recovery_with(
    retry_failed_id: Option<&str>,
    recoverable_delivery_id: Option<&str>,
    persist_new_recovery: impl FnOnce() -> Result<()>,
    abandon_old_delivery: impl FnOnce(&str) -> Result<bool>,
    rollback_new_recovery: impl FnOnce() -> Result<bool>,
    remove_retry_card: impl FnOnce(&str),
) -> std::result::Result<(), RetryGenerationRecoveryCommitError> {
    if persist_new_recovery().is_err() {
        return Err(RetryGenerationRecoveryCommitError::NewRecovery);
    }
    if let Some(failed_asset_id) = recoverable_delivery_id {
        if !matches!(abandon_old_delivery(failed_asset_id), Ok(true)) {
            return match rollback_new_recovery() {
                Ok(true) => Err(RetryGenerationRecoveryCommitError::OldDelivery),
                Ok(false) | Err(_) => Err(RetryGenerationRecoveryCommitError::NewRecoveryRollback),
            };
        }
    }
    if let Some(retry_failed_id) = retry_failed_id {
        remove_retry_card(retry_failed_id);
    }
    Ok(())
}

// TEMP(team-accounts): remove in Task 10 after namespace admission is wired.
pub(super) fn start_backend_generation(
    app: &AppWindow,
    _context: AppContext,
    _raw_prompt: String,
    _create_conversation: bool,
    _retry_failed_id: Option<String>,
    _forced_count: Option<i32>,
    _existing_generation_policy: ExistingGenerationPolicy,
    _destination: GenerationDestination,
) {
    app.global::<AppState>().set_generation_status(
        ApiError::LocalState {
            message: "任务准备失败，请重试".to_owned(),
        }
        .user_message()
        .into(),
    );
}

pub(super) fn start_backend_generation_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
    raw_prompt: String,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
    existing_generation_policy: ExistingGenerationPolicy,
    destination: GenerationDestination,
) {
    let billing_scope = match capture_billing_scope_for_submission(
        context.backend.as_deref(),
        &authority,
        billing_scope,
    ) {
        Ok(scope) => scope,
        Err(error) => {
            app.global::<AppState>()
                .set_generation_status(error.user_message().into());
            return;
        }
    };
    let session_scope = billing_scope.request.session.clone();

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
    let original_references = {
        let store = store.borrow();
        let references = match &destination {
            GenerationDestination::Canvas { .. } => &store.canvas_references,
            GenerationDestination::Gallery => references_for_category(&store.references, &category),
        };
        references
            .iter()
            .take(max_reference_images_for_category(&category))
            .cloned()
            .collect::<Vec<_>>()
    };
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
    let recoverable_delivery_id = retry_failed_id.as_deref().filter(|failed_asset_id| {
        store.borrow().generations.iter().any(|item| {
            item.id == *failed_asset_id
                && item.source_path == "failed"
                && item.delivery_recoverable
        })
    });

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
        schema_version: 2,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        billing_account_group_id: billing_scope.request.account_group_id.clone(),
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
    let recovery_identity = recovery_record.identity();
    let recovery_commit = commit_retry_generation_recovery_with(
        retry_failed_id.as_deref(),
        recoverable_delivery_id,
        || {
            upsert_pending_generation_for_namespace(
                &authority,
                &billing_scope,
                recovery_record.clone(),
            )
        },
        |failed_asset_id| {
            recoverable_delivery_for_failed_asset_for_namespace(&authority, failed_asset_id)
                .and_then(|candidate| match candidate {
                    Some((old, _)) => abandon_pending_delivery_for_namespace(
                        &authority,
                        &old.identity(),
                        failed_asset_id,
                    ),
                    None => Ok(false),
                })
        },
        || remove_pending_generation_for_namespace(&authority, &recovery_identity),
        |retry_failed_id| {
            let mut store = store.borrow_mut();
            store.generations.retain(|item| item.id != retry_failed_id);
            save_local_store(app, &store);
            push_all(app, &store);
        },
    );
    if let Err(error) = recovery_commit {
        state.set_generation_status(
            match error {
                RetryGenerationRecoveryCommitError::NewRecovery => "任务准备失败，请重试",
                RetryGenerationRecoveryCommitError::OldDelivery => {
                    "本地生成恢复记录无法更新，请重启后重试"
                }
                RetryGenerationRecoveryCommitError::NewRecoveryRollback => {
                    "本地生成恢复记录无法回滚，请重启后检查任务状态"
                }
            }
            .into(),
        );
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
            delivery_download_reservations: Vec::new(),
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
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
                    let _ = sender.send(GenerationOutcome::Failure {
                        reason: error.generation_message(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    });
                    return;
                }
            }
            let uploaded_snapshot = uploaded.clone();
            if !matches!(
                apply_generation_patch_for_namespace(
                    &authority,
                    &recovery_identity,
                    GenerationRecoveryPatch::UploadedFileIds(uploaded_snapshot)
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
        let mut detail = match api.create_task_billing(&request, &billing_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &worker_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次生图，请前往充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
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
            apply_generation_patch_for_namespace(
                &authority,
                &recovery_identity,
                GenerationRecoveryPatch::Accepted {
                    server_task_id: task_id_for_record,
                    uploaded_file_ids: uploaded.clone(),
                    clear_reference_inputs: false
                }
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
                    apply_generation_patch_for_namespace(
                        &authority,
                        &recovery_identity,
                        GenerationRecoveryPatch::Terminal {
                            expected_success_count
                        }
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
        Vec::new(),
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

// TEMP(team-accounts): remove in Task 10 after namespace admission is wired.
pub(super) fn start_backend_image_edit(
    app: &AppWindow,
    _context: AppContext,
    _source_path: PathBuf,
    _mask_path: PathBuf,
    _prompt: String,
    _model_code: String,
    _quality: String,
) {
    app.global::<AppState>().set_image_editor_status(
        ApiError::LocalState {
            message: "图片编辑任务准备失败，请重试".to_owned(),
        }
        .user_message()
        .into(),
    );
}

pub(super) fn start_backend_image_edit_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
    source_path: PathBuf,
    mask_path: PathBuf,
    prompt: String,
    model_code: String,
    quality: String,
) {
    let billing_scope = match capture_billing_scope_for_submission(
        context.backend.as_deref(),
        &authority,
        billing_scope,
    ) {
        Ok(scope) => scope,
        Err(error) => {
            app.global::<AppState>()
                .set_image_editor_status(error.user_message().into());
            return;
        }
    };
    let session_scope = billing_scope.request.session.clone();

    let state = app.global::<AppState>();
    let Some(backend) = context.backend.clone() else {
        cleanup_image_edit_input_path(&source_path);
        cleanup_image_edit_input_path(&mask_path);
        state.set_image_editor_generating(false);
        state.set_image_editor_status("服务端尚未初始化，请重启客户端后重试".into());
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
        schema_version: 2,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        billing_account_group_id: billing_scope.request.account_group_id.clone(),
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
    if upsert_pending_generation_for_namespace(&authority, &billing_scope, record.clone()).is_err()
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
            delivery_download_reservations: Vec::new(),
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
        run_generation_with_billing_scope(
            backend,
            authority,
            billing_scope,
            worker_scope,
            record,
            sender,
            cancellations,
        )
    });
    poll_generation_stream(
        app.as_weak(),
        context,
        session_scope,
        Vec::new(),
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

// TEMP(team-accounts): remove in Task 10 after namespace admission is wired.
pub(super) fn start_backend_upscale(
    app: &AppWindow,
    _context: AppContext,
    _scale: u32,
    _quality: String,
) {
    app.global::<AppState>().set_viewer_message(
        ApiError::LocalState {
            message: "放大任务准备失败，请重试".to_owned(),
        }
        .user_message()
        .into(),
    );
}

struct PreparedUpscaleSubmission {
    backend: Arc<BackendRuntime>,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: BillingScope,
    record: PendingGenerationRecord,
}

impl PreparedUpscaleSubmission {
    // Admission persists the exact captured identity before a worker can upload or bill.
    fn new(
        backend: Arc<BackendRuntime>,
        authority: Arc<NamespaceStorageAuthority>,
        billing_scope: &BillingScope,
        record: PendingGenerationRecord,
    ) -> std::result::Result<Self, ApiError> {
        let billing_scope =
            capture_billing_scope_for_submission(Some(&backend), &authority, billing_scope)?;
        if record.task_type != "image_upscale" || record.reference_paths.len() != 1 {
            return Err(ApiError::LocalState {
                message: "放大任务输入不完整，请重新发起任务".into(),
            });
        }
        upsert_pending_generation_for_namespace(&authority, &billing_scope, record.clone())
            .map_err(|error| ApiError::LocalState {
                message: format!("无法保存放大任务恢复记录：{error}"),
            })?;
        Ok(Self {
            backend,
            authority,
            billing_scope,
            record,
        })
    }

    fn run(
        self,
        sender: mpsc::Sender<GenerationOutcome>,
        cancellations: Arc<Mutex<BTreeSet<String>>>,
        source_prompt_for_result: String,
    ) {
        let Self {
            backend,
            authority,
            billing_scope,
            record: recovery_record,
        } = self;
        let worker_scope = billing_scope.request.session.clone();
        let recovery_identity = recovery_record.identity();
        let request_id = recovery_record.client_request_id.clone();
        let reference_path = recovery_record.reference_paths[0].clone();
        let model_code = recovery_record.model_code.clone();
        let generation_prompt = recovery_record.generation_prompt.clone();
        let quality_for_worker = recovery_record.quality.clone();
        let target_width = recovery_record.target_width;
        let target_height = recovery_record.target_height;
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
                    remove_pending_generation_for_namespace(&authority, &recovery_identity),
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
            apply_generation_patch_for_namespace(
                &authority,
                &recovery_identity,
                GenerationRecoveryPatch::UploadedAndReleaseInputs(uploaded_snapshot)
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
        let mut detail = match api.create_upscale_task_billing(&request, &billing_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &worker_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
                    let _ = sender.send(GenerationOutcome::CreditInsufficient {
                        message: "积分不足以支持本次放大，请前往充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &worker_scope);
                    }
                    let _ = remove_pending_generation_for_namespace(&authority, &recovery_identity);
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
            apply_generation_patch_for_namespace(
                &authority,
                &recovery_identity,
                GenerationRecoveryPatch::Accepted {
                    server_task_id: task_id_for_record,
                    uploaded_file_ids: uploaded_for_record,
                    clear_reference_inputs: false
                }
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
                    apply_generation_patch_for_namespace(
                        &authority,
                        &recovery_identity,
                        GenerationRecoveryPatch::Terminal {
                            expected_success_count
                        }
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
    }
}

pub(super) fn start_backend_upscale_with_billing_scope(
    app: &AppWindow,
    context: AppContext,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: &BillingScope,
    scale: u32,
    quality: String,
) {
    let billing_scope = match capture_billing_scope_for_submission(
        context.backend.as_deref(),
        &authority,
        billing_scope,
    ) {
        Ok(scope) => scope,
        Err(error) => {
            app.global::<AppState>()
                .set_viewer_message(error.user_message().into());
            return;
        }
    };
    let session_scope = billing_scope.request.session.clone();

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
        schema_version: 2,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id: request_id.clone(),
        owner_user_id: session_scope.owner_user_id.clone(),
        billing_account_group_id: billing_scope.request.account_group_id.clone(),
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
    let submission = match PreparedUpscaleSubmission::new(
        backend,
        authority,
        &billing_scope,
        recovery_record,
    ) {
        Ok(submission) => submission,
        Err(error) => {
            cleanup_upscale_input_path(&upload_path);
            state.set_viewer_message(error.user_message().into());
            return;
        }
    };

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
            delivery_download_reservations: Vec::new(),
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
    std::thread::spawn(move || {
        submission.run(sender, cancellations, source_prompt_for_result);
    });

    poll_generation_stream(
        app.as_weak(),
        context,
        session_scope,
        Vec::new(),
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
        let canvas = state.get_page().as_str() == "canvas";
        let reference = references_for_context(store, &category, canvas)
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
    configured_output_directory().join("image-edit-inputs")
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
    configured_output_directory().join("upscale-references")
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

fn release_recovered_upscale_inputs_for_namespace(
    record: &mut PendingGenerationRecord,
    authority: &NamespaceStorageAuthority,
) -> bool {
    if record.task_type != "image_upscale" || record.reference_paths.is_empty() {
        return true;
    }
    if !matches!(
        apply_generation_patch_for_namespace(
            authority,
            &record.identity(),
            GenerationRecoveryPatch::ReleaseReferenceInputs
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

// TEMP(team-accounts): remove in Task 10 after namespace admission is wired.
pub(super) fn recover_pending_generations(app: &AppWindow, _context: AppContext) {
    app.global::<AppState>().set_generation_status(
        ApiError::LocalState {
            message: "任务恢复暂不可用，请稍后重试".to_owned(),
        }
        .user_message()
        .into(),
    );
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
    // TEMP(team-accounts): no global bytes confer recovery authority.
    Err(RecoveryError::NamespaceRequired.into())
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

// TEMP(team-accounts): remove in Task 10 after namespace admission is wired.
fn recover_server_generation_tasks(
    app: &AppWindow,
    _context: AppContext,
    _session_scope: SessionScope,
    _known_server_ids: BTreeSet<String>,
) {
    app.global::<AppState>().set_generation_status(
        ApiError::LocalState {
            message: "任务恢复暂不可用，请稍后重试".to_owned(),
        }
        .user_message()
        .into(),
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
    let Some(delivery_download_reservations) =
        reserve_recovered_delivery_downloads(app, &context, &record)
    else {
        return;
    };
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
            delivery_download_reservations: delivery_download_reservations.clone(),
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
        delivery_download_reservations,
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

// TEMP(team-accounts): saved-payer admission is required before recovery replay.
fn run_recovered_generation_worker(
    _backend: Arc<BackendRuntime>,
    _session_scope: SessionScope,
    _record: PendingGenerationRecord,
    _sender: mpsc::Sender<GenerationOutcome>,
    _cancellations: Arc<Mutex<BTreeSet<String>>>,
) {
}

fn run_generation_with_billing_scope(
    backend: Arc<BackendRuntime>,
    authority: Arc<NamespaceStorageAuthority>,
    billing_scope: BillingScope,
    session_scope: SessionScope,
    mut record: PendingGenerationRecord,
    sender: mpsc::Sender<GenerationOutcome>,
    cancellations: Arc<Mutex<BTreeSet<String>>>,
) {
    if capture_billing_scope_for_submission(Some(&backend), &authority, &billing_scope).is_err()
        || record.billing_account_group_id != billing_scope.request.account_group_id
        || record.owner_user_id != session_scope.owner_user_id
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
            apply_generation_patch_for_namespace(
                &authority,
                &record.identity(),
                GenerationRecoveryPatch::ReleaseReferenceInputs
            ),
            Ok(true)
        ) {
            return;
        }
        cleanup_image_edit_record_inputs(&record);
        record.reference_paths.clear();
        record.reference_sha256.clear();
        record.reference_size_bytes.clear();
    }
    if record.task_type == "image_upscale"
        && (!record.server_task_id.is_empty()
            || (!record.reference_paths.is_empty()
                && uploaded.len() >= record.reference_paths.len()))
        && !release_recovered_upscale_inputs_for_namespace(&mut record, &authority)
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
                    apply_generation_patch_for_namespace(
                        &authority,
                        &record.identity(),
                        GenerationRecoveryPatch::UploadedFileIds(snapshot)
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
        && !release_recovered_upscale_inputs_for_namespace(&mut record, &authority)
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
            api.create_upscale_task_billing(&request, &billing_scope)
        } else if task_type == "image_edit" {
            if uploaded.len() != 2 {
                if !matches!(
                    remove_pending_generation_for_namespace(&authority, &record.identity()),
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
            api.create_image_edit_task_billing(&request, &billing_scope)
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
            api.create_task_billing(&request, &billing_scope)
        };
        match created {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    if !matches!(
                        remove_pending_generation_for_namespace(&authority, &record.identity()),
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
                        remove_pending_generation_for_namespace(&authority, &record.identity()),
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
        apply_generation_patch_for_namespace(
            &authority,
            &record.identity(),
            GenerationRecoveryPatch::Accepted {
                server_task_id: server_id_snapshot,
                uploaded_file_ids: uploaded_snapshot,
                clear_reference_inputs: record.task_type == "image_edit"
            }
        ),
        Ok(true)
    ) {
        return;
    }
    cleanup_generation_record_inputs(&record);
    record.reference_paths.clear();
    record.reference_sha256.clear();
    record.reference_size_bytes.clear();
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
                apply_generation_patch_for_namespace(
                    &authority,
                    &record.identity(),
                    GenerationRecoveryPatch::Terminal {
                        expected_success_count: expected
                    }
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

    #[test]
    fn retry_recovery_upsert_failure_keeps_the_old_delivery_recoverable() {
        let events = RefCell::new(Vec::new());
        let abandoned = RefCell::new(false);

        let result = commit_retry_generation_recovery_with(
            Some("failed-card"),
            Some("failed-card"),
            || {
                events.borrow_mut().push("upsert-new-recovery");
                Err(anyhow!("new recovery persistence failed"))
            },
            |_| {
                *abandoned.borrow_mut() = true;
                events.borrow_mut().push("abandon-old-delivery");
                Ok(true)
            },
            || {
                events.borrow_mut().push("rollback-new-recovery");
                Ok(true)
            },
            |_| events.borrow_mut().push("remove-old-card"),
        );

        assert!(result.is_err());
        assert!(!*abandoned.borrow());
        assert_eq!(events.into_inner(), vec!["upsert-new-recovery"]);
    }

    #[test]
    fn committed_retry_abandons_the_old_delivery_immediately_before_removing_the_card() {
        let events = RefCell::new(Vec::new());

        let result = commit_retry_generation_recovery_with(
            Some("failed-card"),
            Some("failed-card"),
            || {
                events.borrow_mut().push("upsert-new-recovery");
                Ok(())
            },
            |failed_asset_id| {
                assert_eq!(failed_asset_id, "failed-card");
                events.borrow_mut().push("abandon-old-delivery");
                Ok(true)
            },
            || {
                events.borrow_mut().push("rollback-new-recovery");
                Ok(true)
            },
            |failed_asset_id| {
                assert_eq!(failed_asset_id, "failed-card");
                events.borrow_mut().push("remove-old-card");
            },
        );

        assert!(result.is_ok());
        assert_eq!(
            events.into_inner(),
            vec![
                "upsert-new-recovery",
                "abandon-old-delivery",
                "remove-old-card",
            ]
        );
    }

    #[test]
    fn retry_recovery_abandonment_failure_rolls_back_new_recovery_and_keeps_old_card() {
        let events = RefCell::new(Vec::new());
        let new_recovery_persisted = std::cell::Cell::new(false);
        let old_card_removed = std::cell::Cell::new(false);

        let result = commit_retry_generation_recovery_with(
            Some("failed-card"),
            Some("failed-card"),
            || {
                events.borrow_mut().push("persist-new-recovery");
                new_recovery_persisted.set(true);
                Ok(())
            },
            |failed_asset_id| {
                assert_eq!(failed_asset_id, "failed-card");
                events.borrow_mut().push("abandon-old-delivery");
                Err(anyhow!("old delivery persistence failed"))
            },
            || {
                events.borrow_mut().push("rollback-new-recovery");
                new_recovery_persisted.set(false);
                Ok(true)
            },
            |_| {
                events.borrow_mut().push("remove-old-card");
                old_card_removed.set(true);
            },
        );

        assert_eq!(
            result,
            Err(RetryGenerationRecoveryCommitError::OldDelivery)
        );
        assert!(!new_recovery_persisted.get());
        assert!(!old_card_removed.get());
        assert_eq!(
            events.into_inner(),
            vec![
                "persist-new-recovery",
                "abandon-old-delivery",
                "rollback-new-recovery",
            ]
        );
    }

    #[test]
    fn retry_recovery_reports_when_rollback_persistence_also_fails() {
        let events = RefCell::new(Vec::new());
        let old_card_removed = std::cell::Cell::new(false);

        let result = commit_retry_generation_recovery_with(
            Some("failed-card"),
            Some("failed-card"),
            || {
                events.borrow_mut().push("persist-new-recovery");
                Ok(())
            },
            |_| {
                events.borrow_mut().push("abandon-old-delivery");
                Ok(false)
            },
            || {
                events.borrow_mut().push("rollback-new-recovery");
                Err(anyhow!("rollback persistence failed"))
            },
            |_| {
                events.borrow_mut().push("remove-old-card");
                old_card_removed.set(true);
            },
        );

        assert_eq!(
            result,
            Err(RetryGenerationRecoveryCommitError::NewRecoveryRollback)
        );
        assert!(!old_card_removed.get());
        assert_eq!(
            events.into_inner(),
            vec![
                "persist-new-recovery",
                "abandon-old-delivery",
                "rollback-new-recovery",
            ]
        );
    }

    fn failed_generation_task(code: &str, message: &str) -> GenerationTaskDetail {
        GenerationTaskDetail {
            id: "failed-task".to_string(),
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".to_string(),
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
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".to_string(),
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
            schema_version: 2,
            created_at_epoch_ms: Local::now().timestamp_millis(),
            client_request_id: "delivery_test_request".to_string(),
            owner_user_id: "delivery-test-user".to_string(),
            billing_account_group_id: "22222222-2222-4222-8222-222222222222".to_owned(),
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

/// Captures one admitted payer lease before file preparation or worker creation.
/// Current selection is deliberately absent from this boundary.
pub(super) fn capture_billing_scope_for_submission(
    backend: Option<&BackendRuntime>,
    authority: &NamespaceStorageAuthority,
    billing_scope: &BillingScope,
) -> std::result::Result<BillingScope, ApiError> {
    let session = &billing_scope.request.session;
    let canonical_owner =
        api::uuid_path_segment(&session.owner_user_id).is_ok_and(|id| id == session.owner_user_id);
    let canonical_payer = api::uuid_path_segment(&billing_scope.request.account_group_id)
        .is_ok_and(|id| id == billing_scope.request.account_group_id);
    if !canonical_owner
        || !canonical_payer
        || authority.user_public_id() != session.owner_user_id
        || authority.lease().auth_epoch != session.auth_epoch
        || !backend.is_some_and(|backend| backend.api.session().is_scope_current(session))
    {
        return Err(ApiError::LocalState {
            message: "登录状态已变化，请重新发起任务".to_owned(),
        });
    }
    Ok(billing_scope.clone())
}

#[cfg(test)]
pub(in crate::runtime) mod billing_capture_test_support {
    use super::*;
    use crate::runtime::test_support::MemoryRefreshTokenStore;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    pub(in crate::runtime) const OWNER: &str = "11111111-1111-4111-8111-111111111111";
    pub(in crate::runtime) const PAYER: &str = "22222222-2222-4222-8222-222222222222";
    pub(in crate::runtime) const OTHER: &str = "33333333-3333-4333-8333-333333333333";
    pub(in crate::runtime) struct Fixture {
        pub(in crate::runtime) root: tempfile::TempDir,
        pub(in crate::runtime) authority: Arc<NamespaceStorageAuthority>,
        pub(in crate::runtime) scope: BillingScope,
        pub(in crate::runtime) backend: Arc<BackendRuntime>,
        pub(in crate::runtime) context: AppContext,
    }
    pub(in crate::runtime) fn fixture(base_url: &str) -> Fixture {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let session = Arc::new(SessionManager::new(Arc::new(
            MemoryRefreshTokenStore::default(),
        )));
        let session_scope = session
            .install_tokens_for_user(
                &TokenSet {
                    access_token: "capture-access".into(),
                    access_expires_in_seconds: 1800,
                    refresh_token: "capture-refresh".into(),
                    refresh_expires_at: "2099-01-01T00:00:00Z".into(),
                    token_type: "X-Token".into(),
                },
                OWNER,
            )
            .unwrap();
        let scope = BillingScope {
            request: GroupRequestScope {
                session: session_scope.clone(),
                account_group_id: PAYER.into(),
            },
            context_epoch: 17,
        };
        let root_capability = Arc::new(NamespaceFs::open_data_root(root.path()).unwrap());
        let lease = NamespaceLease {
            namespace: UserNamespace::new(root.path(), OWNER).unwrap(),
            auth_epoch: session_scope.auth_epoch,
            namespace_epoch: 1,
        };
        let authority =
            Arc::new(NamespaceStorageAuthority::open(root_capability.clone(), &lease).unwrap());
        let backend = Arc::new(BackendRuntime {
            api: ApiClient::new(
                ApiClientConfig {
                    base_url: reqwest::Url::parse(base_url).unwrap(),
                    app_version: "fixture".into(),
                    timeout: Duration::from_secs(2),
                },
                DeviceIdentity {
                    id: OTHER.into(),
                    name: "fixture".into(),
                    platform: "macos".into(),
                },
                session,
            )
            .unwrap(),
        });
        let context = AppContext {
            data_root_capability: Some(root_capability),
            backend: Some(backend.clone()),
            current_user_id: Arc::new(Mutex::new(Some(OWNER.into()))),
            account_snapshot_scope: Arc::new(Mutex::new(Some(session_scope))),
            ..Default::default()
        };
        Fixture {
            root,
            authority,
            scope,
            backend,
            context,
        }
    }
    pub(in crate::runtime) fn listener() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        (listener, url)
    }
    pub(in crate::runtime) struct Captured {
        pub(in crate::runtime) request: String,
        pub(in crate::runtime) document: Value,
    }
    pub(in crate::runtime) fn read_request(stream: &mut TcpStream) -> String {
        String::from_utf8(read_request_bytes(stream)).unwrap()
    }
    pub(in crate::runtime) fn read_request_bytes(stream: &mut TcpStream) -> Vec<u8> {
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0u8; 4096];
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0, "request ended before headers/body");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|length| length.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    return bytes;
                }
            }
        }
    }
    pub(in crate::runtime) fn capture(
        listener: TcpListener,
        authority: Arc<NamespaceStorageAuthority>,
        filename: &'static str,
    ) -> (mpsc::Sender<()>, std::thread::JoinHandle<Captured>) {
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => {
                        panic!("billable dispatch did not reach recording transport: {error}")
                    }
                }
            };
            let request = read_request(&mut stream);
            let key = ManagedFileKey::new(ManagedUserArea::Recovery, filename).unwrap();
            let mut file = authority.open_existing_regular(&key).unwrap();
            let mut bytes = Vec::new();
            authority.read_regular_to(&mut file, &mut bytes).unwrap();
            let document = serde_json::from_slice(&bytes).unwrap();
            let body = r#"{"request_id":"capture-rejected","data":null,"error":{"code":"invalid_parameter","message":"fixture-stop","details":null},"meta":null}"#;
            write!(stream,"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            Captured { request, document }
        });
        (release_tx, worker)
    }
    pub(in crate::runtime) fn assert_capture(captured: &Captured, vector: &str) {
        let headers = captured
            .request
            .split("\r\n\r\n")
            .next()
            .unwrap()
            .to_lowercase();
        assert!(headers.contains(&format!("x-account-group-id: {PAYER}")));
        assert!(headers.contains("x-token: capture-access"));
        let body: Value =
            serde_json::from_str(captured.request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let row = &captured.document[vector][0];
        assert_eq!(captured.document["schema_version"], 2);
        assert_eq!(row["schema_version"], 2);
        assert_eq!(row["owner_user_id"], OWNER);
        assert_eq!(row["billing_account_group_id"], PAYER);
        assert_eq!(row["client_request_id"], body["client_request_id"]);
        assert!(!row["client_request_id"].as_str().unwrap().is_empty());
    }
    pub(in crate::runtime) fn corrupt(authority: &NamespaceStorageAuthority, filename: &str) {
        let key = ManagedFileKey::new(ManagedUserArea::Recovery, filename).unwrap();
        let mut file = authority.create_new_regular(&key).unwrap();
        authority
            .write_new_regular_from(&mut file, &mut &b"invalid-owned-fixture"[..])
            .unwrap();
        authority.sync_regular(&mut file).unwrap();
    }
    pub(in crate::runtime) fn assert_no_request(listener: &TcpListener) {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_millis(150);
        while Instant::now() < deadline {
            assert!(
                matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock),
                "unexpected dispatch"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    pub(in crate::runtime) fn generation_record(
        scope: &BillingScope,
        task_type: &str,
    ) -> PendingGenerationRecord {
        PendingGenerationRecord {
            schema_version: 2,
            created_at_epoch_ms: 1,
            client_request_id: "0123456789abcdef0123456789abcdef".into(),
            owner_user_id: scope.request.session.owner_user_id.clone(),
            billing_account_group_id: scope.request.account_group_id.clone(),
            auth_epoch: scope.request.session.auth_epoch,
            local_task_id: "local".into(),
            server_task_id: String::new(),
            raw_prompt: "fixture prompt".into(),
            generation_prompt: "fixture prompt".into(),
            task_type: task_type.into(),
            category: "other".into(),
            mode: "game".into(),
            ratio: "1:1".into(),
            quality: "2K".into(),
            model_code: "fixture-model".into(),
            conversation_id: OTHER.into(),
            count: 1,
            target_width: 2048,
            target_height: 2048,
            create_conversation: false,
            reference_paths: Vec::new(),
            reference_sha256: Vec::new(),
            reference_size_bytes: Vec::new(),
            lineage_reference_paths: Vec::new(),
            uploaded_file_ids: if task_type == "image_edit" {
                vec![OTHER.into(), OWNER.into()]
            } else {
                vec![OTHER.into()]
            },
            deliveries: Vec::new(),
            terminal: false,
            expected_success_count: 0,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }
    pub(in crate::runtime) fn assert_generation_worker<T: Send + 'static>(
        task_type: &str,
        run: impl FnOnce(
                Arc<BackendRuntime>,
                Arc<NamespaceStorageAuthority>,
                BillingScope,
                SessionScope,
                PendingGenerationRecord,
                mpsc::Sender<T>,
            ) + Send
            + 'static,
    ) {
        let (listener, url) = listener();
        let mut fixture = fixture(&url);
        let captured_scope = capture_billing_scope_for_submission(
            Some(&fixture.backend),
            &fixture.authority,
            &fixture.scope,
        )
        .unwrap();
        let record = generation_record(&captured_scope, task_type);
        upsert_pending_generation_for_namespace(
            &fixture.authority,
            &captured_scope,
            record.clone(),
        )
        .unwrap();
        let (release, transport) = capture(
            listener,
            fixture.authority.clone(),
            "pending-generations.json",
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        let (sender, _receiver) = mpsc::channel();
        let backend = fixture.backend.clone();
        let authority = fixture.authority.clone();
        let session = captured_scope.request.session.clone();
        let worker = std::thread::spawn(move || {
            run(backend, authority, captured_scope, session, record, sender)
        });
        release.send(()).unwrap();
        let observed = transport.join().unwrap();
        assert_capture(&observed, "generations");
        worker.join().unwrap();
    }
}

#[cfg(test)]
mod billing_capture_tests {
    use super::billing_capture_test_support::*;
    use super::*;
    #[test]
    fn billing_capture_recorder_waits_for_delayed_fragmented_request() {
        use std::io::Write;
        use std::net::TcpStream;

        let (listener, _url) = listener();
        listener.set_nonblocking(true).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let (mut accepted, _) = listener.accept().unwrap();
        // Exercise the inherited macOS mode explicitly on every host.
        accepted.set_nonblocking(true).unwrap();
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            // Controlled network delay: the reader must survive both a missing
            // first byte and a body that has not arrived in its entirety yet.
            std::thread::sleep(Duration::from_millis(150));
            client.write_all(b"POST /fixture HTTP/1.1\r\nContent-Length: 4\r\n\r\nA\0")?;
            std::thread::sleep(Duration::from_millis(50));
            client.write_all(b"\xffB")
        });
        let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read_request_bytes(&mut accepted)
        }));
        // Keep the accepted socket alive and join the writer before asserting,
        // including when the old reader panics during the required RED run.
        let writer_result = writer.join();
        drop(accepted);
        drop(listener);
        writer_result.expect("socket writer must settle").unwrap();
        assert_eq!(
            observed.expect("recorder must wait for delayed request bytes"),
            b"POST /fixture HTTP/1.1\r\nContent-Length: 4\r\n\r\nA\0\xffB"
        );
    }

    fn upscale_input_record(fixture: &Fixture) -> PendingGenerationRecord {
        let path = fixture.root.path().join("upscale-source.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255]))
            .save(&path)
            .unwrap();
        let (sha256, sizes) = reference_fingerprints(std::slice::from_ref(&path)).unwrap();
        let mut record = generation_record(&fixture.scope, "image_upscale");
        record.reference_paths = vec![path.display().to_string()];
        record.reference_sha256 = sha256;
        record.reference_size_bytes = sizes;
        record.lineage_reference_paths = vec!["retained-lineage".into()];
        record.uploaded_file_ids.clear();
        record.generation_prompt = "actual upscale generation prompt".into();
        record
    }

    fn upscale_document(authority: &NamespaceStorageAuthority) -> Value {
        let key =
            ManagedFileKey::new(ManagedUserArea::Recovery, "pending-generations.json").unwrap();
        let mut file = authority.open_existing_regular(&key).unwrap();
        let mut bytes = Vec::new();
        authority.read_regular_to(&mut file, &mut bytes).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn billing_capture_actual_upscale_submission_persists_before_upload_and_billing() {
        use std::io::Write;
        let (listener, url) = listener();
        let mut fixture = fixture(&url);
        let record = upscale_input_record(&fixture);
        let original = serde_json::to_value(&record).unwrap();
        let submission = PreparedUpscaleSubmission::new(
            fixture.backend.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            record,
        )
        .unwrap();
        assert_eq!(
            upscale_document(&fixture.authority)["generations"][0],
            original
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        let authority = fixture.authority.clone();
        let transport = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let mut billed = None;
            for step in 0..5 {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) => panic!("actual upscale request {step} missing: {error}"),
                    }
                };
                let bytes = read_request_bytes(&mut stream);
                let request = String::from_utf8_lossy(&bytes);
                let success = serde_json::json!({"request_id":"fixture", "data":{}, "error":null, "meta":null});
                let (status, response) = match step {
                    0 => {
                        assert!(request.starts_with("POST /v1/uploads/references HTTP/"));
                        assert!(request.to_lowercase().contains("x-token: capture-access"));
                        assert!(!request.to_lowercase().contains("x-account-group-id:"));
                        assert_eq!(upscale_document(&authority)["generations"][0], original, "initial identity and inputs must be durable before the first network request");
                        (
                            "200 OK",
                            serde_json::json!({"request_id":"fixture", "data":{"file":{"id":OTHER}, "upload":{"method":"POST", "url":format!("{url}fixture-upload"), "fields":{}, "file_field":"file"}}, "error":null, "meta":null}),
                        )
                    }
                    1 => {
                        assert!(request.starts_with("POST /fixture-upload HTTP/"));
                        assert!(request.contains("multipart/form-data"));
                        assert!(bytes
                            .windows(8)
                            .any(|window| window == b"\x89PNG\r\n\x1a\n"));
                        ("200 OK", success)
                    }
                    2 => {
                        assert!(request.starts_with(&format!(
                            "POST /v1/uploads/references/{OTHER}/complete HTTP/"
                        )));
                        ("200 OK", success)
                    }
                    3 => {
                        assert!(request.starts_with("POST /v1/generation/tasks HTTP/"));
                        let document = upscale_document(&authority);
                        let row = &document["generations"][0];
                        assert_eq!(row["reference_paths"], serde_json::json!([]));
                        assert_eq!(row["reference_sha256"], serde_json::json!([]));
                        assert_eq!(row["reference_size_bytes"], serde_json::json!([]));
                        assert_eq!(
                            row["lineage_reference_paths"],
                            original["lineage_reference_paths"]
                        );
                        assert_eq!(row["uploaded_file_ids"], serde_json::json!([OTHER]));
                        let captured = Captured {
                            request: String::from_utf8(bytes).unwrap(),
                            document,
                        };
                        assert_capture(&captured, "generations");
                        let body: Value = serde_json::from_str(
                            captured.request.split("\r\n\r\n").nth(1).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(body["task_type"], "image_upscale");
                        assert_eq!(body["prompt"], original["generation_prompt"]);
                        assert_eq!(body["model_code"], original["model_code"]);
                        assert_eq!(body["quality"], original["quality"]);
                        assert_eq!(body["target_width"], original["target_width"]);
                        assert_eq!(body["target_height"], original["target_height"]);
                        assert_eq!(body["reference_file_ids"], serde_json::json!([OTHER]));
                        billed = Some(captured);
                        (
                            "400 Bad Request",
                            serde_json::json!({"request_id":"fixture-stop", "data":null, "error":{"code":"invalid_parameter", "message":"fixture-stop", "details":null}, "meta":null}),
                        )
                    }
                    4 => {
                        assert!(request
                            .starts_with(&format!("DELETE /v1/uploads/references/{OTHER} HTTP/")));
                        ("200 OK", success)
                    }
                    _ => unreachable!(),
                };
                let body = response.to_string();
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            billed.expect("actual billable upscale request must have been observed")
        });
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            submission.run(
                sender,
                Arc::new(Mutex::new(BTreeSet::new())),
                "display prompt".into(),
            )
        });
        let _observed = transport.join().unwrap();
        worker.join().unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            GenerationOutcome::Failure { .. }
        ));
        assert!(load_pending_generations_for_namespace(&fixture.authority)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn billing_capture_actual_upscale_storage_failure_prevents_dispatch() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let record = upscale_input_record(&fixture);
        corrupt(&fixture.authority, "pending-generations.json");
        assert!(matches!(
            PreparedUpscaleSubmission::new(
                fixture.backend.clone(),
                fixture.authority.clone(),
                &fixture.scope,
                record
            ),
            Err(ApiError::LocalState { .. })
        ));
        assert_no_request(&listener);
    }

    #[test]
    fn billing_capture_actual_upscale_scope_failure_prevents_dispatch() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let record = upscale_input_record(&fixture);
        let mut wrong_scope = fixture.scope.clone();
        wrong_scope.request.session.auth_epoch += 1;
        assert!(matches!(
            PreparedUpscaleSubmission::new(
                fixture.backend.clone(),
                fixture.authority.clone(),
                &wrong_scope,
                record
            ),
            Err(ApiError::LocalState { .. })
        ));
        assert!(load_pending_generations_for_namespace(&fixture.authority)
            .unwrap()
            .is_empty());
        assert_no_request(&listener);
    }

    #[test]
    fn billing_capture_actual_upscale_expired_worker_preserves_record_without_dispatch() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let submission = PreparedUpscaleSubmission::new(
            fixture.backend.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            upscale_input_record(&fixture),
        )
        .unwrap();
        let before = upscale_document(&fixture.authority);
        fixture.backend.api.session().clear().unwrap();
        let (sender, receiver) = mpsc::channel();
        submission.run(
            sender,
            Arc::new(Mutex::new(BTreeSet::new())),
            "display prompt".into(),
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(upscale_document(&fixture.authority), before);
        assert_no_request(&listener);
    }

    #[test]
    fn billing_capture_actual_upscale_invalid_input_prevents_dispatch() {
        let (listener, url) = listener();
        let fixture = fixture(&url);
        let mut record = upscale_input_record(&fixture);
        record.reference_paths.clear();
        assert!(matches!(
            PreparedUpscaleSubmission::new(
                fixture.backend.clone(),
                fixture.authority.clone(),
                &fixture.scope,
                record
            ),
            Err(ApiError::LocalState { .. })
        ));
        assert!(load_pending_generations_for_namespace(&fixture.authority)
            .unwrap()
            .is_empty());
        assert_no_request(&listener);
    }

    fn app() -> AppWindow {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_session_state("online".into());
        app.global::<AppState>()
            .set_image_model("fixture-model".into());
        app.global::<AppState>().set_quality("1K".into());
        app
    }
    #[test]
    fn billing_capture_generation_start_persists_identity_before_real_dispatch() {
        let app = app();
        let (listener, url) = listener();
        let mut fixture = fixture(&url);
        let (release, transport) = capture(
            listener,
            fixture.authority.clone(),
            "pending-generations.json",
        );
        start_backend_generation_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            "fixture prompt".into(),
            false,
            None,
            Some(1),
            ExistingGenerationPolicy::KeepExisting,
            GenerationDestination::Gallery,
        );
        fixture.scope.request.account_group_id = OTHER.into();
        fixture.scope.context_epoch += 1;
        release.send(()).unwrap();
        let observed = transport.join().unwrap();
        assert_capture(&observed, "generations");
    }
    #[test]
    fn billing_capture_generation_storage_and_scope_failure_prevent_dispatch() {
        let app = app();
        let (listener, url) = listener();
        let fixture = fixture(&url);
        corrupt(&fixture.authority, "pending-generations.json");
        start_backend_generation_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &fixture.scope,
            "fixture prompt".into(),
            false,
            None,
            Some(1),
            ExistingGenerationPolicy::KeepExisting,
            GenerationDestination::Gallery,
        );
        assert!(fixture.context.generations.active.borrow().is_empty());
        let mut wrong = fixture.scope.clone();
        wrong.request.session.auth_epoch += 1;
        start_backend_generation_with_billing_scope(
            &app,
            fixture.context.clone(),
            fixture.authority.clone(),
            &wrong,
            "fixture prompt".into(),
            false,
            None,
            Some(1),
            ExistingGenerationPolicy::KeepExisting,
            GenerationDestination::Gallery,
        );
        assert!(fixture.context.generations.active.borrow().is_empty());
        assert_no_request(&listener);
    }
    #[test]
    fn billing_capture_image_edit_worker_keeps_persisted_payer() {
        assert_generation_worker(
            "image_edit",
            |backend, authority, scope, session, record, sender| {
                run_generation_with_billing_scope(
                    backend,
                    authority,
                    scope,
                    session,
                    record,
                    sender,
                    Arc::new(Mutex::new(BTreeSet::new())),
                )
            },
        );
    }
    #[test]
    fn billing_capture_alternate_upscale_worker_keeps_persisted_payer() {
        assert_generation_worker(
            "image_upscale",
            |backend, authority, scope, session, record, sender| {
                run_generation_with_billing_scope(
                    backend,
                    authority,
                    scope,
                    session,
                    record,
                    sender,
                    Arc::new(Mutex::new(BTreeSet::new())),
                )
            },
        );
    }
}
