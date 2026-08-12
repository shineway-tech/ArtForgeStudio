use super::*;
use sha2::{Digest, Sha256};

const PROMPT_TASK_RECOVERY_SCHEMA_VERSION: u32 = 1;
const PROMPT_TASK_RETRY_MIN_MS: u64 = 1_000;
const PROMPT_TASK_RETRY_MAX_MS: u64 = 30_000;

#[derive(Clone)]
pub(super) enum PromptResultTarget {
    Composer {
        category: String,
        input: String,
    },
    CustomPrompt {
        session_id: String,
        input: String,
        append_result: bool,
    },
    CanvasNode {
        id: String,
        input: String,
    },
}

pub(super) struct PromptTaskRequest {
    pub(super) model_code: String,
    pub(super) task_type: &'static str,
    pub(super) prompt: String,
    pub(super) target_language: Option<String>,
    pub(super) optimize: bool,
    pub(super) target: PromptResultTarget,
    pub(super) reference_paths: Vec<PathBuf>,
}

enum PromptTaskOutcome {
    Ready(PendingPromptTaskRecord),
    Settled(PendingPromptTaskRecord),
    Failed {
        record: PendingPromptTaskRecord,
        reason: String,
    },
    Suspended {
        record: PendingPromptTaskRecord,
        reason: String,
    },
    SessionEnded {
        record: PendingPromptTaskRecord,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptResultApplication {
    NotApplied,
    AppliedDurably,
    AppliedWithCleanupPending,
    AppliedPendingCustomPromptSave,
}

pub(super) fn wire_prompt_task_recovery_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_apply_recovered_prompt_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            claim_recovered_prompt_result(&app, &context);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_copy_recovered_prompt_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            copy_recovered_prompt_result(&app, &context);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_dismiss_recovered_prompt_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            discard_recovered_prompt_result(&app, &context);
        });
    }
}

pub(super) fn start_backend_prompt_task(
    app: &AppWindow,
    context: AppContext,
    task: PromptTaskRequest,
) {
    let Some(_backend) = context.backend.as_ref() else {
        set_prompt_task_start_failure(app, &task.target, "服务端尚未初始化，请重启客户端后重试");
        return;
    };
    let Some(session_scope) = current_prompt_task_session_scope(&context) else {
        set_prompt_task_start_failure(app, &task.target, "账号信息尚未同步，请稍后重试");
        return;
    };
    let client_request_id = Uuid::new_v4().simple().to_string();
    let activity_kind = prompt_activity_kind(task.task_type, &task.target).to_string();
    let (target_kind, target_id, target_category, target_input, append_result) =
        serialize_prompt_target(&task.target);
    let reference_paths = task
        .reference_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let (reference_sha256, reference_size_bytes) =
        match prompt_reference_fingerprints(&reference_paths) {
            Ok(fingerprints) => fingerprints.into_iter().unzip(),
            Err(reason) => {
                set_prompt_task_start_failure(app, &task.target, &reason);
                return;
            }
        };
    let record = PendingPromptTaskRecord {
        schema_version: PROMPT_TASK_RECOVERY_SCHEMA_VERSION,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id,
        owner_user_id: session_scope.owner_user_id,
        auth_epoch: session_scope.auth_epoch,
        server_task_id: String::new(),
        task_type: task.task_type.to_string(),
        model_code: task.model_code,
        prompt: task.prompt,
        target_language: task.target_language,
        optimize: task.optimize,
        target_kind,
        target_id,
        target_category,
        target_input,
        append_result,
        activity_kind,
        reference_paths,
        reference_sha256,
        reference_size_bytes,
        uploaded_file_ids: Vec::new(),
        result_prompt: String::new(),
        terminal_error: String::new(),
        applied_to_target: false,
        result_committed: false,
    };

    // This is deliberately the first operation: a fixed request ID and the complete request body
    // must survive before uploads or a billable create request can happen.
    if let Err(error) = upsert_pending_prompt_task(record.clone()) {
        set_prompt_task_start_failure(
            app,
            &task.target,
            &format!("无法保存任务恢复信息：{error}"),
        );
        return;
    }
    launch_pending_prompt_task(app, context, record, true);
}

pub(super) fn recover_pending_prompt_tasks(app: &AppWindow, context: AppContext) {
    if app.global::<AppState>().get_session_state().as_str() != "online" {
        return;
    }

    let Some(session_scope) = current_prompt_task_session_scope(&context) else {
        return;
    };
    let mut records = match load_pending_prompt_tasks_checked() {
        Ok(records) => records,
        Err(error) => {
            app.global::<AppState>().set_generation_status(
                format!(
                    "提示词任务恢复文件无法读取，原文件已保留，请勿重复提交付费任务并联系客服：{error}"
                )
                .into(),
            );
            return;
        }
    };
    records.sort_by_key(|record| (record.created_at_epoch_ms, record.client_request_id.clone()));
    for mut record in records {
        if record.owner_user_id != session_scope.owner_user_id {
            continue;
        }
        if !valid_pending_prompt_task(&record) {
            // An older or partially-written record may already refer to a billed server task.
            // Preserve it for support/manual discard instead of silently destroying evidence.
            app.global::<AppState>().set_generation_status(
                "检测到无法自动恢复的提示词任务记录；记录已保留，请联系客服处理".into(),
            );
            continue;
        }
        // A terminal record can still have remote references to clean up. Rebind every valid
        // same-owner record before choosing a terminal/non-terminal recovery path so cleanup and
        // result delivery never launch under the stale epoch from a previous login session.
        if !rebind_prompt_task_epoch(&mut record, &session_scope, |old_record, new_auth_epoch| {
            update_prompt_task_record_scoped(old_record, |pending| {
                pending.auth_epoch = new_auth_epoch;
            })
        })
        .unwrap_or(false)
        {
            continue;
        }
        if prompt_task_completed_unclaimed(&record) && !record.uploaded_file_ids.is_empty() {
            launch_pending_prompt_task(app, context.clone(), record, false);
            continue;
        }
        if prompt_task_completed_unclaimed(&record) {
            if record.result_committed {
                let _ = remove_prompt_task_record_scoped(&record);
                continue;
            }
            match apply_prompt_result_if_target_matches(app, &context, &record) {
                PromptResultApplication::AppliedDurably => {
                    let _ = remove_prompt_task_record_scoped(&record);
                }
                PromptResultApplication::AppliedWithCleanupPending => {}
                PromptResultApplication::AppliedPendingCustomPromptSave
                | PromptResultApplication::NotApplied => {}
            }
            continue;
        }
        let visible = prompt_target_matches(app, &context, &record);
        launch_pending_prompt_task(app, context.clone(), record, visible);
    }
    present_next_recovered_prompt_result(app, &context);
}

fn launch_pending_prompt_task(
    app: &AppWindow,
    context: AppContext,
    record: PendingPromptTaskRecord,
    visible: bool,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let record_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !backend.api.session().is_scope_current(&record_scope) {
        return;
    }
    let active_key = format!("{}:{}", record.client_request_id, record.auth_epoch);
    {
        let mut active = context
            .active_prompt_task_requests
            .lock()
            .unwrap_or_else(|value| value.into_inner());
        if !active.insert(active_key.clone()) {
            return;
        }
    }
    if visible {
        set_prompt_task_activity(app, &record, true);
    }

    let active = context.active_prompt_task_requests.clone();
    let (sender, receiver) = mpsc::channel::<PromptTaskOutcome>();
    let worker_record = record.clone();
    std::thread::spawn(move || {
        let outcome = run_pending_prompt_task(&backend, worker_record);
        active
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .remove(&active_key);
        let _ = sender.send(outcome);
    });
    poll_pending_prompt_task(
        app.as_weak(),
        context,
        record,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn run_pending_prompt_task(
    backend: &BackendRuntime,
    mut record: PendingPromptTaskRecord,
) -> PromptTaskOutcome {
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    let api = GenerationApi::new(backend.api.clone());
    if prompt_task_completed_unclaimed(&record) {
        match cleanup_prompt_task_references(&api, &record.uploaded_file_ids, &session_scope) {
            Ok(true) => {
                match update_prompt_task_record_scoped(&record, |pending| {
                    pending.uploaded_file_ids.clear();
                }) {
                    Ok(true) => record.uploaded_file_ids.clear(),
                    Ok(false) => return prompt_task_scope_suspended(record),
                    Err(_) => {}
                }
            }
            Ok(false) => {}
            Err(_) => return prompt_task_session_ended(record),
        }
        if record.result_committed {
            if record.uploaded_file_ids.is_empty() {
                return match remove_prompt_task_record_scoped(&record) {
                    Ok(true) => PromptTaskOutcome::Settled(record),
                    Ok(false) => prompt_task_scope_suspended(record),
                    Err(_) => PromptTaskOutcome::Settled(record),
                };
            }
            return PromptTaskOutcome::Settled(record);
        }
        // Once the terminal result is durable, remote cleanup is best-effort. Keeping the file
        // IDs on disk lets a later recovery retry cleanup without withholding the paid result.
        return PromptTaskOutcome::Ready(record);
    }
    if record.uploaded_file_ids.len() > record.reference_paths.len() {
        return fail_prompt_task(
            &api,
            &session_scope,
            record,
            "提示词任务的参考图恢复信息无效，请重新提交".to_string(),
        );
    }

    while record.uploaded_file_ids.len() < record.reference_paths.len() {
        let reference_index = record.uploaded_file_ids.len();
        let path = PathBuf::from(&record.reference_paths[reference_index]);
        if !path.is_file() {
            return fail_prompt_task(
                &api,
                &session_scope,
                record,
                "参考图文件已不存在，请重新选择后提交".to_string(),
            );
        }
        if !prompt_reference_file_matches(&record, reference_index) {
            return fail_prompt_task(
                &api,
                &session_scope,
                record,
                "参考图内容已发生变化，请重新选择后提交".to_string(),
            );
        }
        let mut retry_ms = PROMPT_TASK_RETRY_MIN_MS;
        let file_id = loop {
            match api.upload_reference_scoped(&path, &session_scope) {
                Ok(file_id) => break file_id,
                Err(error) if prompt_task_api_error_is_transient(&error) => {
                    std::thread::sleep(Duration::from_millis(retry_ms));
                    retry_ms = next_prompt_task_retry_ms(retry_ms);
                }
                Err(error) if prompt_task_api_error_requires_login(&error) => {
                    return prompt_task_session_ended(record);
                }
                Err(error) => {
                    return fail_prompt_task(&api, &session_scope, record, error.user_message())
                }
            }
        };
        record.uploaded_file_ids.push(file_id);
        let uploaded = record.uploaded_file_ids.clone();
        match update_prompt_task_record_scoped(&record, |pending| {
            pending.uploaded_file_ids = uploaded;
        }) {
            Ok(true) => {}
            Ok(false) => return prompt_task_scope_suspended(record),
            Err(error) => {
                return fail_prompt_task(
                    &api,
                    &session_scope,
                    record,
                    format!("无法保存参考图上传恢复信息：{error}"),
                )
            }
        }
    }

    let mut detail = if record.server_task_id.is_empty() {
        let request = prompt_task_create_request(&record);
        let mut retry_ms = PROMPT_TASK_RETRY_MIN_MS;
        let detail = loop {
            match api.create_task_scoped(&request, &session_scope) {
                Ok(detail) => break detail,
                Err(error) if prompt_task_api_error_is_transient(&error) => {
                    // A timed-out response may still have created and billed the task. Replaying the
                    // exact body with the same client request ID is therefore mandatory.
                    std::thread::sleep(Duration::from_millis(retry_ms));
                    retry_ms = next_prompt_task_retry_ms(retry_ms);
                }
                Err(error) if prompt_task_api_error_requires_login(&error) => {
                    return prompt_task_session_ended(record);
                }
                Err(error) => {
                    return fail_prompt_task(&api, &session_scope, record, error.user_message())
                }
            }
        };
        record.server_task_id = detail.id.clone();
        let server_task_id = record.server_task_id.clone();
        match update_prompt_task_record_scoped(&record, |pending| {
            pending.server_task_id = server_task_id;
        }) {
            Ok(true) => {}
            Ok(false) => return prompt_task_scope_suspended(record),
            Err(error) => {
                // The fixed request body and idempotency key remain on disk. Recovery can safely
                // replay create and rediscover the same server task instead of charging twice.
                return PromptTaskOutcome::Suspended {
                    record,
                    reason: format!("无法保存服务端任务编号，稍后将安全重试：{error}"),
                };
            }
        }
        detail
    } else {
        let mut retry_ms = PROMPT_TASK_RETRY_MIN_MS;
        loop {
            match api.task_scoped(&record.server_task_id, &session_scope) {
                Ok(detail) => break detail,
                Err(error) if prompt_task_api_error_is_transient(&error) => {
                    std::thread::sleep(Duration::from_millis(retry_ms));
                    retry_ms = next_prompt_task_retry_ms(retry_ms);
                }
                Err(error) if prompt_task_api_error_requires_login(&error) => {
                    return prompt_task_session_ended(record);
                }
                Err(error) => {
                    return fail_prompt_task(&api, &session_scope, record, error.user_message())
                }
            }
        }
    };

    let mut retry_ms = PROMPT_TASK_RETRY_MIN_MS;
    loop {
        if detail.terminal() {
            if matches!(detail.status.as_str(), "completed" | "partially_completed") {
                let Some(result_prompt) = detail
                    .result_prompt
                    .as_deref()
                    .map(normalize_prompt_task_result)
                    .filter(|value| !value.trim().is_empty())
                else {
                    let terminal_error =
                        "服务端任务已结束但未返回可用的提示词结果；任务记录已保留，请联系客服处理"
                            .to_string();
                    record.terminal_error = terminal_error.clone();
                    match update_prompt_task_record_scoped(&record, |pending| {
                        pending.terminal_error = terminal_error;
                    }) {
                        Ok(true) => {}
                        Ok(false) => return prompt_task_scope_suspended(record),
                        Err(error) => {
                            return PromptTaskOutcome::Suspended {
                                record,
                                reason: format!("任务结果异常信息暂时无法保存：{error}"),
                            }
                        }
                    }
                    match cleanup_prompt_task_references(
                        &api,
                        &record.uploaded_file_ids,
                        &session_scope,
                    ) {
                        Ok(true) => {
                            match update_prompt_task_record_scoped(&record, |pending| {
                                pending.uploaded_file_ids.clear();
                            }) {
                                Ok(true) => record.uploaded_file_ids.clear(),
                                Ok(false) => return prompt_task_scope_suspended(record),
                                Err(_) => {}
                            }
                        }
                        Ok(false) => {}
                        Err(_) => return prompt_task_session_ended(record),
                    }
                    return PromptTaskOutcome::Ready(record);
                };
                record.result_prompt = result_prompt.clone();
                match update_prompt_task_record_scoped(&record, |pending| {
                    pending.result_prompt = result_prompt;
                }) {
                    Ok(true) => {}
                    Ok(false) => return prompt_task_scope_suspended(record),
                    Err(error) => {
                        return PromptTaskOutcome::Suspended {
                            record,
                            reason: format!("提示词结果暂时无法保存，稍后将继续恢复：{error}"),
                        }
                    }
                }
                match cleanup_prompt_task_references(
                    &api,
                    &record.uploaded_file_ids,
                    &session_scope,
                ) {
                    Ok(true) => {
                        match update_prompt_task_record_scoped(&record, |pending| {
                            pending.uploaded_file_ids.clear();
                        }) {
                            Ok(true) => record.uploaded_file_ids.clear(),
                            Ok(false) => return prompt_task_scope_suspended(record),
                            Err(_) => {}
                        }
                    }
                    Ok(false) => {}
                    Err(_) => return prompt_task_session_ended(record),
                }
                return PromptTaskOutcome::Ready(record);
            }
            let reason = detail
                .failure
                .map(|failure| failure.message)
                .unwrap_or_else(|| "服务端提示词任务执行失败".to_string());
            return fail_prompt_task(&api, &session_scope, record, reason);
        }

        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        match api.task_scoped(&record.server_task_id, &session_scope) {
            Ok(next) => {
                detail = next;
                retry_ms = PROMPT_TASK_RETRY_MIN_MS;
            }
            Err(error) if prompt_task_api_error_is_transient(&error) => {
                std::thread::sleep(Duration::from_millis(retry_ms));
                retry_ms = next_prompt_task_retry_ms(retry_ms);
            }
            Err(error) if prompt_task_api_error_requires_login(&error) => {
                return prompt_task_session_ended(record);
            }
            Err(error) => {
                return fail_prompt_task(&api, &session_scope, record, error.user_message())
            }
        }
    }
}

fn poll_pending_prompt_task(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    expected_record: PendingPromptTaskRecord,
    receiver: Rc<RefCell<Option<mpsc::Receiver<PromptTaskOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => {
                    slot.take();
                    Some(outcome)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(PromptTaskOutcome::Suspended {
                        record: expected_record.clone(),
                        reason: "提示词任务意外中断，恢复记录已保留，请稍后重新登录恢复"
                            .to_string(),
                    })
                }
            }
        };
        let Some(outcome) = outcome else {
            if receiver.borrow().is_some() {
                poll_pending_prompt_task(app_weak, context, expected_record, receiver);
            }
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        context
            .active_prompt_task_requests
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .remove(&format!(
                "{}:{}",
                expected_record.client_request_id, expected_record.auth_epoch
            ));
        match outcome {
            PromptTaskOutcome::Ready(expected_record) => {
                let Some(record) = load_pending_prompt_tasks()
                    .into_iter()
                    .find(|record| prompt_task_record_identity_matches(record, &expected_record))
                else {
                    return;
                };
                if !prompt_task_scope_matches_context(&context, &record) {
                    return;
                }
                clear_prompt_task_activity_if_owned(&app, &record);
                match apply_prompt_result_if_target_matches(&app, &context, &record) {
                    PromptResultApplication::AppliedDurably => {
                        let _ = remove_prompt_task_record_scoped(&record);
                    }
                    PromptResultApplication::AppliedWithCleanupPending => {}
                    PromptResultApplication::AppliedPendingCustomPromptSave => {}
                    PromptResultApplication::NotApplied => {
                        present_next_recovered_prompt_result(&app, &context);
                    }
                }
            }
            PromptTaskOutcome::Settled(record) => {
                if prompt_task_scope_matches_context(&context, &record) {
                    clear_prompt_task_activity_if_owned(&app, &record);
                }
            }
            PromptTaskOutcome::Failed { record, reason } => {
                if !prompt_task_scope_matches_context(&context, &record) {
                    return;
                }
                clear_prompt_task_activity_if_owned(&app, &record);
                if prompt_target_matches(&app, &context, &record) {
                    set_prompt_task_failure(&app, &record, &reason);
                }
            }
            PromptTaskOutcome::Suspended { record, reason } => {
                if !prompt_task_scope_matches_context(&context, &record) {
                    return;
                }
                clear_prompt_task_activity_if_owned(&app, &record);
                if prompt_target_matches(&app, &context, &record) {
                    set_prompt_task_failure(&app, &record, &reason);
                }
            }
            PromptTaskOutcome::SessionEnded { record, reason } => {
                let session_scope = SessionScope {
                    owner_user_id: record.owner_user_id.clone(),
                    auth_epoch: record.auth_epoch,
                };
                if terminal_auth_scope_matches_context(&context, &session_scope) {
                    sign_out_locally(&app, &context, true, Some(record.auth_epoch));
                    return;
                }
                if !prompt_task_scope_matches_context(&context, &record) {
                    return;
                }
                clear_prompt_task_activity_if_owned(&app, &record);
                if prompt_target_matches(&app, &context, &record) {
                    set_prompt_task_failure(&app, &record, &reason);
                }
            }
        }
    });
}

fn prompt_task_create_request(record: &PendingPromptTaskRecord) -> CreateGenerationTask {
    CreateGenerationTask {
        client_request_id: record.client_request_id.clone(),
        task_type: record.task_type.clone(),
        model_code: record.model_code.clone(),
        prompt: record.prompt.clone(),
        quality: None,
        count: None,
        aspect_ratio: None,
        reference_file_ids: (!record.uploaded_file_ids.is_empty())
            .then(|| record.uploaded_file_ids.clone()),
        target_language: record.target_language.clone(),
    }
}

fn prompt_task_api_error_is_transient(error: &ApiError) -> bool {
    error.should_preserve_generation_recovery() || error.code() == Some("request_in_progress")
}

fn prompt_task_api_error_requires_login(error: &ApiError) -> bool {
    matches!(error, ApiError::AuthenticationRequired) || error.is_terminal_session_error()
}

fn prompt_task_session_ended(record: PendingPromptTaskRecord) -> PromptTaskOutcome {
    PromptTaskOutcome::SessionEnded {
        record,
        reason: "登录状态已失效，重新登录后将继续恢复提示词任务".to_string(),
    }
}

fn next_prompt_task_retry_ms(current: u64) -> u64 {
    current
        .saturating_mul(2)
        .clamp(PROMPT_TASK_RETRY_MIN_MS, PROMPT_TASK_RETRY_MAX_MS)
}

fn cleanup_prompt_task_references(
    api: &GenerationApi,
    file_ids: &[String],
    session_scope: &SessionScope,
) -> std::result::Result<bool, ApiError> {
    for file_id in file_ids {
        match api.delete_reference_scoped(file_id, session_scope) {
            Ok(()) => {}
            Err(error) if error.code() == Some("reference_file_in_use") => {}
            Err(ApiError::Http { status: 404, .. }) => {}
            Err(error) => match classify_prompt_reference_cleanup_error(error) {
                Ok(true) => {}
                outcome => return outcome,
            },
        }
    }
    Ok(true)
}

fn classify_prompt_reference_cleanup_error(
    error: ApiError,
) -> std::result::Result<bool, ApiError> {
    if prompt_task_api_error_requires_login(&error) {
        Err(error)
    } else {
        Ok(false)
    }
}

fn update_prompt_task_record_scoped(
    record: &PendingPromptTaskRecord,
    update: impl FnOnce(&mut PendingPromptTaskRecord),
) -> Result<bool> {
    update_pending_prompt_task_scoped(
        &record.owner_user_id,
        record.auth_epoch,
        &record.client_request_id,
        update,
    )
}

fn rebind_prompt_task_epoch(
    record: &mut PendingPromptTaskRecord,
    session_scope: &SessionScope,
    persist: impl FnOnce(&PendingPromptTaskRecord, u64) -> Result<bool>,
) -> Result<bool> {
    if record.owner_user_id != session_scope.owner_user_id {
        return Ok(false);
    }
    if record.auth_epoch == session_scope.auth_epoch {
        return Ok(true);
    }
    let new_auth_epoch = session_scope.auth_epoch;
    if !persist(record, new_auth_epoch)? {
        return Ok(false);
    }
    record.auth_epoch = new_auth_epoch;
    Ok(true)
}

fn remove_prompt_task_record_scoped(record: &PendingPromptTaskRecord) -> Result<bool> {
    remove_pending_prompt_task_scoped(
        &record.owner_user_id,
        record.auth_epoch,
        &record.client_request_id,
    )
}

fn prompt_task_record_identity_matches(
    record: &PendingPromptTaskRecord,
    expected: &PendingPromptTaskRecord,
) -> bool {
    record.client_request_id == expected.client_request_id
        && record.owner_user_id == expected.owner_user_id
        && record.auth_epoch == expected.auth_epoch
}

fn fail_prompt_task(
    api: &GenerationApi,
    session_scope: &SessionScope,
    record: PendingPromptTaskRecord,
    reason: String,
) -> PromptTaskOutcome {
    match cleanup_prompt_task_references(api, &record.uploaded_file_ids, session_scope) {
        Ok(true) => {}
        Ok(false) => return prompt_task_scope_suspended(record),
        Err(_) => return prompt_task_session_ended(record),
    }
    match remove_prompt_task_record_scoped(&record) {
        Ok(true) => PromptTaskOutcome::Failed { record, reason },
        Ok(false) => prompt_task_scope_suspended(record),
        Err(error) => PromptTaskOutcome::Suspended {
            record,
            reason: format!("无法更新任务恢复记录，稍后将继续恢复：{error}"),
        },
    }
}

fn valid_pending_prompt_task(record: &PendingPromptTaskRecord) -> bool {
    record.schema_version == PROMPT_TASK_RECOVERY_SCHEMA_VERSION
        && !record.client_request_id.trim().is_empty()
        && !record.owner_user_id.trim().is_empty()
        && !record.task_type.trim().is_empty()
        && !record.model_code.trim().is_empty()
        && !record.prompt.trim().is_empty()
        && record.reference_paths.len() == record.reference_sha256.len()
        && record.reference_paths.len() == record.reference_size_bytes.len()
        && matches!(
            record.target_kind.as_str(),
            "composer" | "custom_prompt" | "canvas_node"
        )
}

fn prompt_reference_fingerprints(paths: &[String]) -> std::result::Result<Vec<(String, u64)>, String> {
    paths
        .iter()
        .map(|path| {
            let bytes = fs::read(path).map_err(|_| "无法读取参考图，请重新选择后提交".to_string())?;
            Ok((format!("{:x}", Sha256::digest(&bytes)), bytes.len() as u64))
        })
        .collect()
}

fn prompt_reference_file_matches(record: &PendingPromptTaskRecord, index: usize) -> bool {
    let Some(path) = record.reference_paths.get(index) else {
        return false;
    };
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    record.reference_size_bytes.get(index).copied() == Some(bytes.len() as u64)
        && record.reference_sha256.get(index).map(String::as_str)
            == Some(sha256.as_str())
}

fn prompt_task_completed_unclaimed(record: &PendingPromptTaskRecord) -> bool {
    !record.result_prompt.trim().is_empty() || !record.terminal_error.trim().is_empty()
}

fn current_prompt_task_user_id(context: &AppContext) -> Option<String> {
    context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone()
        .filter(|value| !value.trim().is_empty())
}

fn current_prompt_task_session_scope(context: &AppContext) -> Option<SessionScope> {
    let owner_user_id = current_prompt_task_user_id(context)?;
    let session = context.backend.as_ref()?.api.session();
    let scope = SessionScope {
        owner_user_id,
        auth_epoch: session.auth_epoch(),
    };
    session.is_scope_current(&scope).then_some(scope)
}

fn prompt_task_scope_matches_context(
    context: &AppContext,
    record: &PendingPromptTaskRecord,
) -> bool {
    if current_prompt_task_user_id(context).as_deref() != Some(record.owner_user_id.as_str()) {
        return false;
    }
    let scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    context.backend.as_ref().is_some_and(|backend| {
        backend.api.session().is_scope_current(&scope)
    })
}

fn prompt_task_scope_suspended(record: PendingPromptTaskRecord) -> PromptTaskOutcome {
    PromptTaskOutcome::Suspended {
        record,
        reason: "账号已切换，任务将保留给原账号恢复".to_string(),
    }
}

fn serialize_prompt_target(target: &PromptResultTarget) -> (String, String, String, String, bool) {
    match target {
        PromptResultTarget::Composer { category, input } => (
            "composer".to_string(),
            String::new(),
            category.clone(),
            input.clone(),
            false,
        ),
        PromptResultTarget::CustomPrompt {
            session_id,
            input,
            append_result,
        } => (
            "custom_prompt".to_string(),
            session_id.clone(),
            String::new(),
            input.clone(),
            *append_result,
        ),
        PromptResultTarget::CanvasNode { id, input } => (
            "canvas_node".to_string(),
            id.clone(),
            String::new(),
            input.clone(),
            false,
        ),
    }
}

fn prompt_activity_kind(task_type: &str, target: &PromptResultTarget) -> &'static str {
    if task_type == "prompt_translate" {
        "translate"
    } else if task_type == "image_style_analysis"
        && matches!(target, PromptResultTarget::CustomPrompt { .. })
    {
        "custom_style_analysis"
    } else {
        "optimize"
    }
}

fn prompt_target_matches(
    app: &AppWindow,
    context: &AppContext,
    record: &PendingPromptTaskRecord,
) -> bool {
    let state = app.global::<AppState>();
    let canvas_input = if record.target_kind == "canvas_node" {
        context
            .store
            .borrow()
            .canvas_notes
            .iter()
            .find(|node| node.id == record.target_id && node.kind == "text")
            .map(|node| node.content.clone())
    } else {
        None
    };
    let current_reference_paths = if record.task_type == "image_style_analysis" {
        if record.target_kind == "composer" {
            references_for_category(&context.store.borrow().references, &record.target_category)
                .iter()
                .take(MAX_REFERENCE_IMAGES)
                .map(|reference| reference.source_path.clone())
                .collect::<Vec<_>>()
        } else if record.target_kind == "custom_prompt" {
            let model = state.get_custom_prompt_reference_items();
            (0..model.row_count())
                .filter_map(|index| model.row_data(index))
                .map(|item| item.source_path.to_string())
                .take(8)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let editor_matches = prompt_target_matches_snapshot(
        record,
        &current_workspace_category(app),
        state.get_prompt().as_str(),
        state.get_custom_prompt_editor_session_id().as_str(),
        state.get_custom_prompt_input().as_str(),
        canvas_input.as_deref(),
        &current_reference_paths,
    );
    if !editor_matches || record.task_type != "image_style_analysis" {
        return editor_matches;
    }
    let Ok(current_fingerprints) = prompt_reference_fingerprints(&current_reference_paths) else {
        return false;
    };
    let (current_sha256, current_size_bytes): (Vec<_>, Vec<_>) =
        current_fingerprints.into_iter().unzip();
    record.reference_sha256 == current_sha256
        && record.reference_size_bytes == current_size_bytes
}

fn prompt_target_matches_snapshot(
    record: &PendingPromptTaskRecord,
    composer_category: &str,
    composer_input: &str,
    custom_session_id: &str,
    custom_input: &str,
    canvas_input: Option<&str>,
    current_reference_paths: &[String],
) -> bool {
    let editor_matches = match record.target_kind.as_str() {
        "composer" => {
            record.target_category == composer_category
                && (record.target_input == composer_input
                    || (!record.result_prompt.trim().is_empty()
                        && record.result_prompt == composer_input))
        }
        "custom_prompt" => {
            !record.target_id.is_empty()
                && record.target_id == custom_session_id
                && record.target_input == custom_input
        }
        "canvas_node" => canvas_input.is_some_and(|input| {
            input == record.target_input
                || (!record.result_prompt.trim().is_empty() && input == record.result_prompt)
        }),
        _ => false,
    };
    editor_matches
        && (record.task_type != "image_style_analysis"
            || record.reference_paths == current_reference_paths)
}

fn durable_apply_before_result_commit(
    durable_apply: impl FnOnce() -> Result<()>,
    commit: impl FnOnce() -> Result<bool>,
) -> Result<bool> {
    durable_apply()?;
    commit()
}

fn apply_prompt_result_if_target_matches(
    app: &AppWindow,
    context: &AppContext,
    record: &PendingPromptTaskRecord,
) -> PromptResultApplication {
    if current_prompt_task_user_id(context).as_deref() != Some(record.owner_user_id.as_str())
        || record.applied_to_target
        || record.result_committed
        || !record.terminal_error.trim().is_empty()
        || !prompt_target_matches(app, context, record)
        || record.result_prompt.trim().is_empty()
    {
        return PromptResultApplication::NotApplied;
    }
    let state = app.global::<AppState>();
    match record.target_kind.as_str() {
        "composer" => {
            let cleanup_pending = !record.uploaded_file_ids.is_empty();
            let committed = durable_apply_before_result_commit(
                || {
                    state.set_prompt(record.result_prompt.clone().into());
                    store_current_prompt_draft(app, &context.store, &record.target_category);
                    save_local_store_checked(app, &context.store.borrow())
                },
                || {
                    update_prompt_task_record_scoped(record, |pending| {
                        pending.result_committed = true;
                    })
                },
            );
            if !matches!(committed, Ok(true)) {
                state.set_generation_status(
                    "提示词结果已保留，但本地保存或确认失败，请在恢复窗口重试".into(),
                );
                return PromptResultApplication::NotApplied;
            }
            state.set_generation_status(prompt_task_success_message(record).into());
            if cleanup_pending {
                PromptResultApplication::AppliedWithCleanupPending
            } else {
                PromptResultApplication::AppliedDurably
            }
        }
        "custom_prompt" => {
            if !matches!(
                update_prompt_task_record_scoped(record, |pending| {
                    pending.applied_to_target = true;
                }),
                Ok(true)
            ) {
                return PromptResultApplication::NotApplied;
            }
            let value = if record.append_result && !record.target_input.trim().is_empty() {
                format!("{}\n\n{}", record.target_input.trim(), record.result_prompt)
            } else {
                record.result_prompt.clone()
            };
            state.set_custom_prompt_input(value.into());
            state.set_custom_prompt_message(prompt_task_success_message(record).into());
            state.set_custom_prompt_recovered_request_id(
                record.client_request_id.clone().into(),
            );
            PromptResultApplication::AppliedPendingCustomPromptSave
        }
        "canvas_node" => {
            let position = context
                .store
                .borrow()
                .canvas_notes
                .iter()
                .find(|node| node.id == record.target_id && node.kind == "text")
                .map(|node| (node.x, node.y));
            let Some((x, y)) = position else {
                return PromptResultApplication::NotApplied;
            };
            let cleanup_pending = !record.uploaded_file_ids.is_empty();
            let committed = durable_apply_before_result_commit(
                || {
                    state.invoke_update_canvas_node(
                        record.target_id.clone().into(),
                        record.result_prompt.clone().into(),
                        x,
                        y,
                    );
                    let applied = context.store.borrow().canvas_notes.iter().any(|node| {
                        node.id == record.target_id
                            && node.kind == "text"
                            && node.content == record.result_prompt
                    });
                    if !applied {
                        return Err(anyhow!("canvas prompt result was not applied"));
                    }
                    save_local_store_checked(app, &context.store.borrow())
                },
                || {
                    update_prompt_task_record_scoped(record, |pending| {
                        pending.result_committed = true;
                    })
                },
            );
            if !matches!(committed, Ok(true)) {
                state.set_generation_status(
                    "画布提示词结果已保留，但本地保存或确认失败，请在恢复窗口重试".into(),
                );
                return PromptResultApplication::NotApplied;
            }
            state.set_generation_status(prompt_task_success_message(record).into());
            if cleanup_pending {
                PromptResultApplication::AppliedWithCleanupPending
            } else {
                PromptResultApplication::AppliedDurably
            }
        }
        _ => PromptResultApplication::NotApplied,
    }
}

fn prompt_task_success_message(record: &PendingPromptTaskRecord) -> &'static str {
    if record.task_type == "image_style_analysis" {
        "图片风格分析完成"
    } else if record.task_type == "prompt_translate" {
        "提示词翻译完成"
    } else {
        "提示词优化完成"
    }
}

fn set_prompt_task_activity(app: &AppWindow, record: &PendingPromptTaskRecord, active: bool) {
    let state = app.global::<AppState>();
    match record.activity_kind.as_str() {
        "translate" => {
            state.set_translating_prompt(active);
            state.set_translating_prompt_request_id(
                if active {
                    record.client_request_id.clone()
                } else {
                    String::new()
                }
                .into(),
            );
        }
        "custom_style_analysis" => {
            state.set_custom_prompt_analyzing(active);
            state.set_custom_style_analysis_request_id(
                if active {
                    record.client_request_id.clone()
                } else {
                    String::new()
                }
                .into(),
            );
        }
        _ => {
            state.set_optimizing_prompt(active);
            state.set_optimizing_prompt_request_id(
                if active {
                    record.client_request_id.clone()
                } else {
                    String::new()
                }
                .into(),
            );
        }
    }
}

fn clear_prompt_task_activity_if_owned(app: &AppWindow, record: &PendingPromptTaskRecord) {
    let state = app.global::<AppState>();
    let owns_activity = match record.activity_kind.as_str() {
        "translate" => {
            state.get_translating_prompt_request_id().as_str() == record.client_request_id
        }
        "custom_style_analysis" => {
            state.get_custom_style_analysis_request_id().as_str() == record.client_request_id
        }
        _ => state.get_optimizing_prompt_request_id().as_str() == record.client_request_id,
    };
    if owns_activity {
        set_prompt_task_activity(app, record, false);
    }
}

fn set_prompt_task_start_failure(app: &AppWindow, target: &PromptResultTarget, reason: &str) {
    let state = app.global::<AppState>();
    match target {
        PromptResultTarget::CustomPrompt { .. } => {
            state.set_custom_prompt_analyzing(false);
            state.set_optimizing_prompt(false);
            state.set_custom_prompt_message(reason.into());
        }
        _ => {
            state.set_optimizing_prompt(false);
            state.set_translating_prompt(false);
            state.set_generation_status(reason.into());
        }
    }
}

fn set_prompt_task_failure(app: &AppWindow, record: &PendingPromptTaskRecord, reason: &str) {
    let state = app.global::<AppState>();
    if record.target_kind == "custom_prompt" {
        state.set_custom_prompt_message(format!("提示词处理失败：{reason}").into());
    } else {
        state.set_generation_status(format!("提示词处理失败：{reason}").into());
    }
}

fn present_next_recovered_prompt_result(app: &AppWindow, context: &AppContext) {
    let state = app.global::<AppState>();
    if state.get_recovered_prompt_result_open() {
        return;
    }
    let Some(owner_user_id) = current_prompt_task_user_id(context) else {
        clear_recovered_prompt_presentation(&state);
        return;
    };
    let tracked_custom_request_id = state.get_custom_prompt_recovered_request_id().to_string();
    let Some(record) = next_recovered_prompt_record(
        load_pending_prompt_tasks(),
        &owner_user_id,
        &tracked_custom_request_id,
    ) else {
        clear_recovered_prompt_presentation(&state);
        return;
    };
    state.set_recovered_prompt_client_request_id(record.client_request_id.into());
    state.set_recovered_prompt_task_type(record.task_type.into());
    state.set_recovered_prompt_target_kind(record.target_kind.into());
    state.set_recovered_prompt_result(record.result_prompt.into());
    state.set_recovered_prompt_error(record.terminal_error.into());
    state.set_recovered_prompt_result_open(true);
}

fn next_recovered_prompt_record(
    records: Vec<PendingPromptTaskRecord>,
    owner_user_id: &str,
    tracked_custom_request_id: &str,
) -> Option<PendingPromptTaskRecord> {
    if !tracked_custom_request_id.is_empty() {
        return None;
    }
    let mut completed = records
        .into_iter()
        .filter(|record| {
            record.owner_user_id == owner_user_id
                && prompt_task_completed_unclaimed(record)
                && !record.result_committed
        })
        .collect::<Vec<_>>();
    completed.sort_by_key(|record| (record.created_at_epoch_ms, record.client_request_id.clone()));
    completed.into_iter().next()
}

fn clear_recovered_prompt_presentation(state: &AppState) {
    state.set_recovered_prompt_result_open(false);
    state.set_recovered_prompt_client_request_id("".into());
    state.set_recovered_prompt_task_type("".into());
    state.set_recovered_prompt_target_kind("".into());
    state.set_recovered_prompt_result("".into());
    state.set_recovered_prompt_error("".into());
}

fn selected_recovered_prompt_record(
    app: &AppWindow,
    context: &AppContext,
) -> Option<PendingPromptTaskRecord> {
    let request_id = app
        .global::<AppState>()
        .get_recovered_prompt_client_request_id()
        .to_string();
    let owner_user_id = current_prompt_task_user_id(context)?;
    load_pending_prompt_tasks().into_iter().find(|record| {
        record.owner_user_id == owner_user_id
            && record.client_request_id == request_id
            && prompt_task_completed_unclaimed(record)
            && !record.result_committed
    })
}

fn claim_recovered_prompt_result(app: &AppWindow, context: &AppContext) {
    let Some(record) = selected_recovered_prompt_record(app, context) else {
        clear_recovered_prompt_presentation(&app.global::<AppState>());
        present_next_recovered_prompt_result(app, context);
        return;
    };
    if record.result_prompt.trim().is_empty() || !record.terminal_error.trim().is_empty() {
        return;
    }
    let state = app.global::<AppState>();
    if record.target_kind == "custom_prompt" && state.get_custom_prompt_editor_open() {
        if !matches!(
            update_prompt_task_record_scoped(&record, |pending| {
                pending.applied_to_target = true;
            }),
            Ok(true)
        ) {
            state.set_custom_prompt_message("无法确认恢复结果，请稍后重试".into());
            return;
        }
        let current = state.get_custom_prompt_input().to_string();
        let value = if record.append_result && !current.trim().is_empty() {
            format!("{}\n\n{}", current.trim(), record.result_prompt)
        } else {
            record.result_prompt.clone()
        };
        state.set_custom_prompt_input(value.into());
        state.set_custom_prompt_message("已使用恢复的提示词结果，保存后完成领取".into());
        state.set_custom_prompt_recovered_request_id(record.client_request_id.clone().into());
    } else {
        let cleanup_pending = !record.uploaded_file_ids.is_empty();
        let category = current_workspace_category(app);
        let committed = durable_apply_before_result_commit(
            || {
                state.set_prompt(record.result_prompt.clone().into());
                store_current_prompt_draft(app, &context.store, &category);
                save_local_store_checked(app, &context.store.borrow())
            },
            || {
                update_prompt_task_record_scoped(&record, |pending| {
                    pending.result_committed = true;
                })
            },
        );
        if !matches!(committed, Ok(true)) {
            state.set_generation_status(
                "恢复结果已保留，但本地保存或确认失败，请稍后重试".into(),
            );
            return;
        }
        state.set_generation_status("已使用恢复的提示词结果".into());
        if !cleanup_pending {
            let _ = remove_prompt_task_record_scoped(&record);
        }
    }
    clear_recovered_prompt_presentation(&state);
    present_next_recovered_prompt_result(app, context);
}

fn copy_recovered_prompt_result(app: &AppWindow, context: &AppContext) {
    let Some(record) = selected_recovered_prompt_record(app, context) else {
        clear_recovered_prompt_presentation(&app.global::<AppState>());
        present_next_recovered_prompt_result(app, context);
        return;
    };
    let text = if record.terminal_error.trim().is_empty() {
        record.result_prompt.clone()
    } else {
        record.terminal_error.clone()
    };
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        app.global::<AppState>()
            .set_generation_status("无法访问系统剪贴板，请手动选择结果文本".into());
        return;
    };
    if clipboard.set_text(text).is_err() {
        app.global::<AppState>()
            .set_generation_status("复制失败，请手动选择结果文本".into());
        return;
    }
    app.global::<AppState>()
        .set_generation_status("恢复的提示词结果已复制，记录仍会保留".into());
}

fn discard_recovered_prompt_result(app: &AppWindow, context: &AppContext) {
    if let Some(record) = selected_recovered_prompt_record(app, context) {
        let _ = remove_prompt_task_record_scoped(&record);
    }
    let state = app.global::<AppState>();
    clear_recovered_prompt_presentation(&state);
    present_next_recovered_prompt_result(app, context);
}

pub(super) fn acknowledge_custom_prompt_recovered_result(
    app: &AppWindow,
    context: &AppContext,
) {
    let state = app.global::<AppState>();
    let request_id = state.get_custom_prompt_recovered_request_id().to_string();
    if request_id.is_empty() {
        return;
    }
    let acknowledged = load_pending_prompt_tasks().into_iter().find(|record| {
        record.client_request_id == request_id
            && current_prompt_task_user_id(context).as_deref()
                == Some(record.owner_user_id.as_str())
    }).is_some_and(|record| {
        if record.uploaded_file_ids.is_empty() {
            matches!(remove_prompt_task_record_scoped(&record), Ok(true))
        } else {
            matches!(
                update_prompt_task_record_scoped(&record, |pending| {
                    pending.applied_to_target = false;
                    pending.result_committed = true;
                }),
                Ok(true)
            )
        }
    });
    if acknowledged {
        state.set_custom_prompt_recovered_request_id("".into());
    } else {
        state.set_generation_status(
            "自定义提示词已保存，但恢复记录确认失败；下次出现时可直接丢弃".into(),
        );
    }
}

pub(super) fn release_custom_prompt_recovered_result(
    app: &AppWindow,
    context: &AppContext,
) {
    let state = app.global::<AppState>();
    if !state.get_custom_prompt_recovered_request_id().is_empty() {
        state.set_custom_prompt_recovered_request_id("".into());
    }
    clear_recovered_prompt_presentation(&state);
    present_next_recovered_prompt_result(app, context);
}

pub(super) fn clear_prompt_task_account_state(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_optimizing_prompt(false);
    state.set_translating_prompt(false);
    state.set_custom_prompt_analyzing(false);
    state.set_optimizing_prompt_request_id("".into());
    state.set_translating_prompt_request_id("".into());
    state.set_custom_style_analysis_request_id("".into());
    state.set_custom_prompt_recovered_request_id("".into());
    clear_recovered_prompt_presentation(&state);
}

pub(super) fn normalize_prompt_task_result(raw: &str) -> String {
    let mut candidate = raw.trim().to_string();
    for _ in 0..4 {
        let trimmed = candidate.trim();
        let unwrapped = if trimmed.starts_with('(') && trimmed.ends_with(')') {
            trimmed[1..trimmed.len() - 1].trim()
        } else {
            trimmed
        };
        let Ok(value) = serde_json::from_str::<Value>(unwrapped) else {
            return unwrapped.to_string();
        };
        let Some(decoded) = prompt_text_from_json(&value) else {
            return unwrapped.to_string();
        };
        if decoded.trim() == unwrapped {
            return decoded.trim().to_string();
        }
        candidate = decoded;
    }
    candidate.trim().to_string()
}

fn prompt_text_from_json(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(values) => values.iter().find_map(prompt_text_from_json),
        Value::Object(object) => [
            "prompt",
            "result_prompt",
            "optimized_prompt",
            "content",
            "text",
            "chinese_prompt",
        ]
        .iter()
        .find_map(|key| object.get(*key).and_then(prompt_text_from_json)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_record(target_kind: &str) -> PendingPromptTaskRecord {
        PendingPromptTaskRecord {
            schema_version: 1,
            created_at_epoch_ms: 1,
            client_request_id: "fixed-request-id".to_string(),
            owner_user_id: "user-a".to_string(),
            auth_epoch: 7,
            server_task_id: String::new(),
            task_type: "image_style_analysis".to_string(),
            model_code: "style-model".to_string(),
            prompt: "analyze".to_string(),
            target_language: None,
            optimize: true,
            target_kind: target_kind.to_string(),
            target_id: String::new(),
            target_category: "character".to_string(),
            target_input: "original prompt".to_string(),
            append_result: false,
            activity_kind: "optimize".to_string(),
            reference_paths: vec!["/tmp/reference.png".to_string()],
            reference_sha256: vec!["abc".to_string()],
            reference_size_bytes: vec![3],
            uploaded_file_ids: vec!["file-1".to_string()],
            result_prompt: String::new(),
            terminal_error: String::new(),
            applied_to_target: false,
            result_committed: false,
        }
    }

    #[test]
    fn idempotent_replay_uses_the_same_request_id_and_complete_body() {
        let record = pending_record("composer");
        let first = prompt_task_create_request(&record);
        let replay = prompt_task_create_request(&record);
        assert_eq!(first.client_request_id, "fixed-request-id");
        assert_eq!(replay.client_request_id, first.client_request_id);
        assert_eq!(replay.task_type, first.task_type);
        assert_eq!(replay.model_code, first.model_code);
        assert_eq!(replay.prompt, first.prompt);
        assert_eq!(replay.reference_file_ids, first.reference_file_ids);
        assert_eq!(replay.target_language, first.target_language);
    }

    #[test]
    fn terminal_result_with_references_rebinds_before_cleanup_launch() {
        let mut record = pending_record("composer");
        record.result_prompt = "paid result".to_string();
        let new_scope = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 11,
        };
        let persisted = std::cell::Cell::new(false);

        let rebound = rebind_prompt_task_epoch(&mut record, &new_scope, |old_record, new_epoch| {
            assert_eq!(old_record.auth_epoch, 7);
            assert_eq!(new_epoch, 11);
            persisted.set(true);
            Ok(true)
        })
        .unwrap();

        assert!(rebound);
        assert!(persisted.get());
        assert!(prompt_task_completed_unclaimed(&record));
        assert!(!record.uploaded_file_ids.is_empty());
        assert_eq!(
            SessionScope {
                owner_user_id: record.owner_user_id.clone(),
                auth_epoch: record.auth_epoch,
            },
            new_scope
        );
    }

    #[test]
    fn committed_terminal_result_with_references_rebinds_before_cleanup_launch() {
        let mut record = pending_record("composer");
        record.result_prompt = "already saved result".to_string();
        record.result_committed = true;
        let new_scope = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 12,
        };

        let rebound = rebind_prompt_task_epoch(&mut record, &new_scope, |old_record, new_epoch| {
            assert!(old_record.result_committed);
            assert_eq!(old_record.auth_epoch, 7);
            assert_eq!(new_epoch, 12);
            Ok(true)
        })
        .unwrap();

        assert!(rebound);
        assert!(record.result_committed);
        assert!(prompt_task_completed_unclaimed(&record));
        assert!(!record.uploaded_file_ids.is_empty());
        assert_eq!(record.auth_epoch, new_scope.auth_epoch);
    }

    #[test]
    fn composer_result_requires_the_original_category_and_prompt_snapshot() {
        let record = pending_record("composer");
        assert!(prompt_target_matches_snapshot(
            &record,
            "character",
            "original prompt",
            "",
            "",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "scene",
            "original prompt",
            "",
            "",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "new prompt",
            "",
            "",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "original prompt",
            "",
            "",
            None,
            &["/tmp/replaced.png".to_string()],
        ));
    }

    #[test]
    fn custom_prompt_result_requires_the_original_session_and_content() {
        let mut record = pending_record("custom_prompt");
        record.target_id = "session-a".to_string();
        assert!(prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "session-a",
            "original prompt",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "session-b",
            "original prompt",
            None,
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "session-a",
            "edited prompt",
            None,
            &record.reference_paths,
        ));
    }

    #[test]
    fn canvas_result_requires_the_original_node_content() {
        let record = pending_record("canvas_node");
        assert!(prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "",
            "",
            Some("original prompt"),
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "",
            "",
            Some("edited prompt"),
            &record.reference_paths,
        ));
        assert!(!prompt_target_matches_snapshot(
            &record,
            "character",
            "",
            "",
            "",
            None,
            &record.reference_paths,
        ));
    }

    #[test]
    fn durable_but_uncommitted_result_can_be_idempotently_reapplied() {
        let mut composer = pending_record("composer");
        composer.result_prompt = "recovered result".to_string();
        assert!(prompt_target_matches_snapshot(
            &composer,
            "character",
            "recovered result",
            "",
            "",
            None,
            &composer.reference_paths,
        ));

        let mut canvas = pending_record("canvas_node");
        canvas.result_prompt = "recovered result".to_string();
        assert!(prompt_target_matches_snapshot(
            &canvas,
            "character",
            "",
            "",
            "",
            Some("recovered result"),
            &canvas.reference_paths,
        ));
    }

    #[test]
    fn transient_retry_backoff_is_bounded() {
        assert_eq!(next_prompt_task_retry_ms(1_000), 2_000);
        assert_eq!(next_prompt_task_retry_ms(16_000), 30_000);
        assert_eq!(next_prompt_task_retry_ms(30_000), 30_000);
    }

    #[test]
    fn idempotency_request_in_progress_is_retried_without_dropping_recovery() {
        let error = ApiError::Http {
            status: 409,
            code: "request_in_progress".to_string(),
            message: "still processing".to_string(),
            request_id: None,
            details: None,
        };
        assert!(prompt_task_api_error_is_transient(&error));
    }

    #[test]
    fn terminal_reference_cleanup_preserves_the_paid_record_and_requests_sign_out() {
        let mut record = pending_record("composer");
        record.result_prompt = "durable paid result".to_string();
        record.result_committed = true;
        let expected_request_id = record.client_request_id.clone();

        let outcome = prompt_task_session_ended(record);

        match outcome {
            PromptTaskOutcome::SessionEnded { record, .. } => {
                assert_eq!(record.client_request_id, expected_request_id);
                assert!(record.result_committed);
                assert!(!record.uploaded_file_ids.is_empty());
            }
            _ => panic!("terminal cleanup must retain the record and signal session end"),
        }
    }

    #[test]
    fn reference_cleanup_classifies_terminal_errors_separately_from_retryable_failures() {
        let terminal = ApiError::Http {
            status: 401,
            code: "session_invalid".to_string(),
            message: "revoked".to_string(),
            request_id: None,
            details: None,
        };
        let retryable = ApiError::Network {
            message: "offline".to_string(),
            timeout: false,
        };

        assert!(classify_prompt_reference_cleanup_error(terminal).is_err());
        assert_eq!(
            classify_prompt_reference_cleanup_error(retryable).unwrap(),
            false
        );
    }

    #[test]
    fn failed_durable_apply_never_marks_the_result_committed() {
        let commit_called = std::cell::Cell::new(false);

        let result = durable_apply_before_result_commit(
            || Err(anyhow!("disk full")),
            || {
                commit_called.set(true);
                Ok(true)
            },
        );

        assert!(result.is_err());
        assert!(!commit_called.get());
    }

    #[test]
    fn result_commit_happens_only_after_durable_apply_succeeds() {
        let events = RefCell::new(Vec::new());

        let result = durable_apply_before_result_commit(
            || {
                events.borrow_mut().push("durable_apply");
                Ok(())
            },
            || {
                events.borrow_mut().push("commit_marker");
                Ok(true)
            },
        );

        assert_eq!(result.unwrap(), true);
        assert_eq!(*events.borrow(), vec!["durable_apply", "commit_marker"]);
    }

    #[test]
    fn tracked_custom_result_pauses_the_entire_recovery_queue() {
        let mut first = pending_record("custom_prompt");
        first.client_request_id = "custom-first".to_string();
        first.result_prompt = "first result".to_string();
        first.applied_to_target = true;
        let mut second = first.clone();
        second.client_request_id = "custom-second".to_string();
        second.created_at_epoch_ms = 2;
        second.result_prompt = "second result".to_string();
        second.applied_to_target = false;

        assert!(next_recovered_prompt_record(
            vec![first, second],
            "user-a",
            "custom-first",
        )
        .is_none());
    }

    #[test]
    fn clearing_custom_tracking_releases_the_next_recovered_result() {
        let mut second = pending_record("custom_prompt");
        second.client_request_id = "custom-second".to_string();
        second.created_at_epoch_ms = 2;
        second.result_prompt = "second result".to_string();

        let selected = next_recovered_prompt_record(vec![second], "user-a", "")
            .expect("next recovered result");

        assert_eq!(selected.client_request_id, "custom-second");
    }
}
