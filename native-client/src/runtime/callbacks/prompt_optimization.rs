use super::*;

// A deep job belongs to the creator namespace, not the currently selected payer.
// The retained row is the only authority for replay and existing-resource responses.
struct DeepPromptCapture {
    session: SessionScope,
    lease: NamespaceLease,
    authority: Arc<NamespaceStorageAuthority>,
    activity: UserActivityPermit,
}
fn capture_deep_user(context: &AppContext, session: SessionScope) -> std::result::Result<DeepPromptCapture, ApiError> {
    let lease = context.namespace_for(&session)?;
    let activity = context.user_activity.begin_recovery_unit(&lease).map_err(transition_error)?;
    let authority = Arc::new(context.storage_authority_for(&lease)?);
    let backend = context.backend.as_ref().ok_or(ApiError::AuthenticationRequired)?;
    if activity.is_quiescing() || !backend.api.user_work_is_current(&session) { return Err(ApiError::AuthenticationRequired); }
    Ok(DeepPromptCapture { session, lease, authority, activity })
}
fn deep_ui(context: &AppContext, apply: impl FnOnce()) -> bool {
    let Some(session) = current_prompt_optimization_session_scope(context) else { return false; };
    let Ok(lease) = context.namespace_for(&session) else { return false; };
    context.apply_user_completion(&lease, apply).is_ok()
}
// Capture the Store-bound writer; prepare outside the UI latch and enqueue in UI order.
// The existing writer owns the commit guards and the acknowledgement is polled without I/O.
fn persist_deep_projection(app: &AppWindow, context: &AppContext, lease: &NamespaceLease) {
    let Some(writer) = context.store.borrow().private_persistence.clone() else {
        deep_current_error(app, context, &transition_error("用户持久化入口尚未激活，任务恢复记录已保留"));
        return;
    };
    // This preparation acquires ordinary/activity guards outside the non-reentrant UI latch.
    let mut prepared = match writer.prepare_ordered_save() {
        Ok(value) => Some(value),
        Err(error) => { deep_current_error(app, context, &transition_error(error)); return; }
    };
    let queued = context.apply_user_completion(lease, || {
        // Sequence allocation and enqueue happen NOW in UI order, never on a later worker.
        prepared.take().expect("single guarded enqueue").enqueue(local_store_data(app, &context.store.borrow()))
    });
    drop(prepared);
    match queued {
        Ok(Ok(receiver)) => poll_deep_projection_ack(app.as_weak(), context.clone(), lease.clone(), receiver),
        Ok(Err(error)) => deep_current_error(app, context, &transition_error(error)),
        Err(_) => {}
    }
}
fn poll_deep_projection_ack<E: std::fmt::Display + 'static>(
    weak: Weak<AppWindow>, context: AppContext, lease: NamespaceLease,
    receiver: mpsc::Receiver<std::result::Result<(), E>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let message = match receiver.try_recv() {
            Ok(Ok(())) => return,
            Ok(Err(_)) => "本地创作状态未能确认保存，任务恢复记录已保留",
            Err(TryRecvError::Disconnected) => "本地保存确认中断，任务恢复记录已保留",
            Err(TryRecvError::Empty) => {
                poll_deep_projection_ack(weak, context, lease, receiver);
                return;
            }
        };
        let Some(app) = weak.upgrade() else { return; };
        let _ = context.apply_user_completion(&lease, || {
            app.global::<AppState>().set_generation_status(message.into());
        });
    });
}
fn deep_current_error(app: &AppWindow, context: &AppContext, error: &ApiError) {
    let _ = deep_ui(context, || {
        let state = app.global::<AppState>();
        let message = show_credit_rejection(&state, error).unwrap_or_else(|| error.user_message());
        state.set_deep_optimization_error(message.into());
    });
}
fn deep_record_for_job(authority: &NamespaceStorageAuthority, id: &str) -> std::result::Result<PendingPromptOptimizationRecord, ApiError> {
    let records = load_pending_prompt_optimizations_for_namespace(authority).map_err(transition_error)?;
    let mut matches = records.into_iter().filter(|row| row.server_job_id == id);
    let row = matches.next().ok_or_else(|| transition_error("任务缺少已保存的付款账号，原记录已保留"))?;
    if matches.next().is_some() { return Err(transition_error("任务恢复记录不唯一，未发送请求")); }
    Ok(row)
}
fn accept_deep_detail(
    authority: &NamespaceStorageAuthority, session: &SessionScope,
    mut record: PendingPromptOptimizationRecord, detail: PromptOptimizationDetail,
) -> std::result::Result<PromptOptimizationDetail, ApiError> {
    require_saved_group(&record.billing_account_group_id, &detail.billing_account_group_id)?;
    if Uuid::parse_str(&detail.id).ok().is_none_or(|id| id.to_string() != detail.id) {
        return Err(transition_error("服务端任务标识无效，恢复记录已保留"));
    }
    if !record.server_job_id.is_empty() && record.server_job_id != detail.id {
        return Err(transition_error("服务端返回了不同的任务，恢复记录已保留"));
    }
    if let PendingPromptOptimizationOperation::Retry { source_job_id } = &record.operation {
        if record.server_job_id.is_empty() && source_job_id == &detail.id {
            return Err(transition_error("重试未返回新的任务标识，原任务已保留"));
        }
    }
    // Verification precedes every rebind/update. Denial, retirement and exact 426 preserve bytes.
    if record.auth_epoch != session.auth_epoch {
        if !rebind_pending_prompt_optimization_epoch_for_namespace(authority, &record.identity(), session.auth_epoch).map_err(transition_error)? {
            return Err(transition_error("任务恢复记录已变化"));
        }
        record.auth_epoch = session.auth_epoch;
    }
    if record.server_job_id.is_empty()
        && !update_pending_prompt_optimization_job_id_for_namespace(authority, &record.identity(), &detail.id).map_err(transition_error)? {
        return Err(transition_error("任务已接收，但本地确认未完成；请按原请求恢复"));
    }
    Ok(detail)
}
fn replay_or_get_deep(
    backend: &BackendRuntime, authority: &Arc<NamespaceStorageAuthority>,
    session: &SessionScope, record: PendingPromptOptimizationRecord,
) -> std::result::Result<PromptOptimizationDetail, ApiError> {
    let _network = backend.api.begin_user_work(session)?;
    let detail = if record.server_job_id.is_empty() {
        let replay = SavedReplayRequest::deep(authority.clone(), session, &record.client_request_id).map_err(transition_error)?;
        backend.api.replay_saved::<PromptOptimizationDetail>(&replay)?.data
    } else {
        PromptOptimizationApi::new(backend.api.clone()).get_scoped(&record.server_job_id, session)?
    };
    accept_deep_detail(authority, session, record, detail)
}
fn execute_new_deep(
    backend: &BackendRuntime, authority: &Arc<NamespaceStorageAuthority>, manager: &BillingContextManager, billing: &BillingScope,
    operation: PendingPromptOptimizationOperation, key: String,
) -> std::result::Result<PromptOptimizationDetail, ApiError> {
    // An unresolved request always wins over a fresh selected payer. The exact saved type,
    // key/body and payer are replayed; no response class permits inventing a replacement.
    if let Some(saved) = load_pending_prompt_optimizations_for_namespace(authority).map_err(transition_error)?
        .into_iter().find(|row| row.server_job_id.is_empty()) {
        return replay_or_get_deep(backend, authority, &billing.request.session, saved);
    }
    if let PendingPromptOptimizationOperation::Retry { source_job_id } = &operation {
        let _source = deep_record_for_job(authority, source_job_id)?;
    }
    if !manager.is_current(billing) { return Err(ApiError::AuthenticationRequired); }
    let record = PendingPromptOptimizationRecord {
        schema_version: 2, client_request_id: key.clone(), owner_user_id: billing.request.session.owner_user_id.clone(),
        billing_account_group_id: billing.request.account_group_id.clone(), auth_epoch: billing.request.session.auth_epoch,
        server_job_id: String::new(), operation: operation.clone(), presentation_dismissed: false,
    };
    upsert_pending_prompt_optimization_for_namespace(authority, billing, record.clone()).map_err(transition_error)?;
    if !manager.is_current(billing) || !backend.api.user_work_is_current(&billing.request.session) { return Err(ApiError::AuthenticationRequired); }
    let _network = backend.api.begin_user_work(&billing.request.session)?;
    let api = PromptOptimizationApi::new(backend.api.clone());
    let detail = match operation {
        PendingPromptOptimizationOperation::Create { request } => api.create_billing(&request, billing)?,
        PendingPromptOptimizationOperation::Retry { source_job_id } => api.retry_billing(&source_job_id, &key, billing)?,
    };
    accept_deep_detail(authority, &billing.request.session, record, detail)
}
struct DeepPromptJob {
    receiver: mpsc::Receiver<std::result::Result<Option<PromptOptimizationDetail>, ApiError>>,
    worker: Option<std::thread::JoinHandle<()>>,
    marker: String,
    category: String,
    prompt: String,
}
fn spawn_deep_job<F>(
    app: &AppWindow, context: AppContext, capture: DeepPromptCapture,
    effect: PromptRequestEffect, expected_visible: String, work: F,
) where F: FnOnce(&BackendRuntime, &Arc<NamespaceStorageAuthority>, &SessionScope) -> std::result::Result<Option<PromptOptimizationDetail>, ApiError> + Send + 'static {
    if context.prompt_optimization_polling.borrow().as_ref().is_some_and(|key| key.starts_with("work:")) {
        deep_current_error(app, &context, &transition_error("当前任务操作尚未完成，请稍后重试"));
        return;
    }
    let Some(backend) = context.backend.clone() else { return; };
    let marker = format!("work:{}", Uuid::new_v4());
    *context.prompt_optimization_polling.borrow_mut() = Some(marker.clone());
    let category = current_workspace_category(app);
    let prompt = app.global::<AppState>().get_prompt().to_string();
    let DeepPromptCapture { session, lease, authority, activity } = capture;
    let worker_scope = session.clone();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = if activity.is_quiescing() || !backend.api.user_work_is_current(&worker_scope) {
            Err(ApiError::AuthenticationRequired)
        } else { work(&backend, &authority, &worker_scope) };
        let _ = sender.send(result);
        // The permit outlives network work and durable acceptance, including a dropped UI receiver.
        drop(activity);
    });
    poll_deep_job(app.as_weak(), context, session, lease, effect, expected_visible, DeepPromptJob { receiver, worker: Some(worker), marker, category, prompt });
}
fn poll_deep_job(
    app_weak: Weak<AppWindow>, context: AppContext, session: SessionScope, lease: NamespaceLease,
    effect: PromptRequestEffect, expected_visible: String, mut job: DeepPromptJob,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                // Keep draining/reaping the bounded worker even if the UI or namespace retires.
                poll_deep_job(app_weak, context, session, lease, effect, expected_visible, job);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(transition_error("深度优化工作线程提前结束，恢复记录已保留")),
        };
        if let Some(worker) = job.worker.take() {
            if worker.join().is_err() { clear_prompt_optimization_polling_if_matches(&context, &job.marker); return; }
        }
        let Some(app) = app_weak.upgrade() else { clear_prompt_optimization_polling_if_matches(&context, &job.marker); return; };
        if prompt_optimization_scope_disposition(&context, &session) == PromptOptimizationScopeDisposition::CapturedTerminal {
            clear_prompt_optimization_polling_if_matches(&context, &job.marker);
            handle_prompt_optimization_terminal(&app, &context, &session);
            return;
        }
        match result {
            Ok(Some(detail)) => queue_deep_detail(&app, context, lease, session, effect, expected_visible, detail, job.marker, job.category, job.prompt),
            other => {
                clear_prompt_optimization_polling_if_matches(&context, &job.marker);
                let mut retry_read = false;
                let accepted = context.apply_user_completion(&lease, || {
                    let state = app.global::<AppState>();
                    if state.get_deep_optimization_job_id().as_str() != expected_visible { return; }
                    match other {
                        Ok(None) => {
                            state.set_deep_optimization_stage("settings".into());
                            state.set_deep_optimization_original_prompt(state.get_prompt());
                            if !context.store.borrow().legacy_deep_prompt_job_id.is_empty()
                                || context.store.borrow().deep_prompt_pending_requests_by_owner.contains_key(&session.owner_user_id)
                                || context.store.borrow().deep_prompt_jobs_by_owner.contains_key(&session.owner_user_id) {
                                state.set_deep_optimization_error(LEGACY_PROMPT_RECOVERY_PRESERVED_MESSAGE.into());
                            }
                        }
                        Err(error) => {
                            let message = show_credit_rejection(&state, &error).unwrap_or_else(|| error.user_message());
                            state.set_deep_optimization_error(message.into());
                            if expected_visible.is_empty() { state.set_deep_optimization_stage("settings".into()); }
                            retry_read = !expected_visible.is_empty() && matches!(error, ApiError::Network { .. });
                        }
                        Ok(Some(_)) => unreachable!(),
                    }
                });
                if accepted.is_ok() && retry_read {
                    begin_prompt_optimization_polling(&app, context, session, expected_visible);
                }
            }
        }
    });
}



fn queue_deep_detail(
    app: &AppWindow, context: AppContext, lease: NamespaceLease, session: SessionScope,
    effect: PromptRequestEffect, expected_visible: String, detail: PromptOptimizationDetail, marker: String, category: String, visible_prompt: String,
) {
    let Some(writer) = context.store.borrow().private_persistence.clone() else {
        clear_prompt_optimization_polling_if_matches(&context, &marker);
        deep_current_error(app, &context, &transition_error("用户持久化入口尚未激活，任务恢复记录已保留"));
        return;
    };
    let mut prepared = match writer.prepare_ordered_save() {
        Ok(value) => Some(value),
        Err(error) => { clear_prompt_optimization_polling_if_matches(&context, &marker); deep_current_error(app, &context, &transition_error(error)); return; }
    };
    let result = if matches!(effect, PromptRequestEffect::ApplyResult) || detail.status == "completed" {
        detail.final_result.clone().or(detail.result.clone())
    } else { None };
    if result.as_ref().is_some_and(|value| value.chinese_prompt.trim().is_empty() || value.english_prompt.trim().is_empty()) {
        drop(prepared);
        clear_prompt_optimization_polling_if_matches(&context, &marker);
        deep_current_error(app, &context, &transition_error("服务端返回的中英文提示词不完整，恢复记录已保留"));
        return;
    }
    let result = result.filter(|_| current_workspace_category(app) == category
        && app.global::<AppState>().get_prompt().as_str() == visible_prompt
        && (matches!(effect, PromptRequestEffect::ApplyResult)
            || detail.original_prompt.as_deref().unwrap_or(app.global::<AppState>().get_deep_optimization_original_prompt().as_str()) == visible_prompt));
    let queued = context.apply_user_completion(&lease, || {
        if app.global::<AppState>().get_deep_optimization_job_id().as_str() != expected_visible { return None; }
        {
            // Stage only this family in the live Store before enqueue, so later full snapshots
            // include it. Visible result/job success is withheld until the exact writer ack.
            let mut store = context.store.borrow_mut();
            store.deep_prompt_jobs_by_owner.insert(session.owner_user_id.clone(), detail.id.clone());
            if let Some(result) = &result {
                store.deep_prompt_bindings.insert(category.clone(), DeepPromptBinding {
                    chinese: result.chinese_prompt.clone(), english: result.english_prompt.clone(),
                });
                set_prompt_draft_for_category(&mut store.prompt_drafts, &category, result.chinese_prompt.clone());
            }
        }
        // Do not inspect/drop an enqueue error in the latch: it owns unqueued guards.
        Some(prepared.take().expect("single guarded enqueue").enqueue(local_store_data(app, &context.store.borrow())))
    });
    drop(prepared); // A rejected completion never moved the guard-bearing preparation.
    match queued {
        Ok(Some(Ok(receiver))) => poll_deep_detail_ack(app.as_weak(), context, lease, effect, expected_visible,
            category, visible_prompt, detail, result, marker.clone(), receiver),
        Ok(Some(Err(error))) => {
            clear_prompt_optimization_polling_if_matches(&context, &marker);
            deep_current_error(app, &context, &transition_error(error));
        }
        _ => clear_prompt_optimization_polling_if_matches(&context, &marker)
    }
}
fn poll_deep_detail_ack<E: std::fmt::Display + 'static>(
    weak: Weak<AppWindow>, context: AppContext, lease: NamespaceLease,
    effect: PromptRequestEffect, expected_visible: String, category: String, visible_prompt: String,
    detail: PromptOptimizationDetail, result: Option<PromptOptimizationResult>, marker: String,
    receiver: mpsc::Receiver<std::result::Result<(), E>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let success = match receiver.try_recv() {
            Ok(Ok(())) => true,
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => false,
            Err(TryRecvError::Empty) => {
                poll_deep_detail_ack(weak, context, lease, effect, expected_visible, category, visible_prompt, detail, result, marker, receiver);
                return;
            }
        };
        clear_prompt_optimization_polling_if_matches(&context, &marker);
        let Some(app) = weak.upgrade() else { return; };
        let mut published = false;
        let accepted = context.apply_user_completion(&lease, || {
            let state = app.global::<AppState>();
            if state.get_deep_optimization_job_id().as_str() != expected_visible { return; }
            if !success {
                state.set_deep_optimization_error("任务已接收，但本地保存未确认；原恢复记录已保留，请重新打开恢复".into());
                return;
            }
            apply_prompt_optimization_detail(&app, &detail);
            if current_workspace_category(&app) == category && state.get_prompt().as_str() == visible_prompt {
                if let Some(result) = result {
                    // The binding/draft were already included in the acknowledged projection.
                    state.set_prompt(result.chinese_prompt.clone().into());
                    state.set_deep_optimization_applied_chinese(result.chinese_prompt.into());
                    state.set_deep_optimization_applied_english(result.english_prompt.into());
                    state.set_generation_status("深度优化结果已应用，生图时将使用英文版本".into());
                }
            }
            published = true;
        });
        if accepted.is_err() || !published { return; }
        refresh_backend_snapshot(&app, context.clone());
        if matches!(detail.status.as_str(), "queued" | "processing") {
            let session = SessionScope { owner_user_id: lease.namespace.user_public_id().into(), auth_epoch: lease.auth_epoch };
            begin_prompt_optimization_polling(&app, context, session, detail.id);
        }
    });
}


#[derive(Clone, Copy)]
enum DeepDismissAction { Close, New, Restore }
fn dismiss_deep_presentation(app: &AppWindow, context: AppContext, action: DeepDismissAction) {
    if context.prompt_optimization_polling.borrow().as_ref().is_some_and(|key| key.starts_with("work:")) { return; }
    let Some(session) = current_prompt_optimization_session_scope(&context) else { return; };
    let capture = match capture_deep_user(&context, session) {
        Ok(value) => value, Err(error) => { deep_current_error(app, &context, &error); return; }
    };
    let state = app.global::<AppState>();
    let id = state.get_deep_optimization_job_id().to_string();
    let original = state.get_deep_optimization_original_prompt().to_string();
    let category = current_workspace_category(app);
    let prompt = state.get_prompt().to_string();
    if matches!(action, DeepDismissAction::Close) {
        let _ = context.apply_user_completion(&capture.lease, || state.set_deep_optimization_open(false));
    }
    if !id.is_empty() && !matches!(state.get_deep_optimization_stage().as_str(), "complete" | "cancelled" | "failed") {
        if !matches!(action, DeepDismissAction::Close) {
            deep_current_error(app, &context, &transition_error("任务尚未结束，请先暂停或取消；恢复记录已保留"));
        }
        return;
    }
    let marker = format!("work:{}", Uuid::new_v4());
    *context.prompt_optimization_polling.borrow_mut() = Some(marker.clone());
    if id.is_empty() {
        queue_deep_dismiss(app, context, capture.lease, id, action, original, category, prompt, marker);
        return;
    }
    let Some(backend) = context.backend.clone() else {
        clear_prompt_optimization_polling_if_matches(&context, &marker); return;
    };
    let DeepPromptCapture { session, lease, authority, activity } = capture;
    let worker_id = id.clone();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = (|| {
            if activity.is_quiescing() { return Err(ApiError::AuthenticationRequired); }
            let _network = backend.api.begin_user_work(&session)?;
            let saved = deep_record_for_job(&authority, &worker_id)?;
            let detail = PromptOptimizationApi::new(backend.api.clone()).get_scoped(&worker_id, &session)?;
            require_saved_group(&saved.billing_account_group_id, &detail.billing_account_group_id)?;
            if detail.id != worker_id || !matches!(detail.status.as_str(), "completed" | "cancelled" | "failed") {
                return Err(transition_error("任务终态尚未确认，原恢复记录已保留"));
            }
            // Dismissal is local presentation metadata, not a new economic operation.
            // Keep the record's original identity/epoch; the producer validates the active authority.
            if !mark_pending_prompt_optimization_presentation_dismissed_for_namespace(&authority, &saved.identity(), &worker_id).map_err(transition_error)? {
                return Err(transition_error("任务恢复记录已变化，未清除当前结果"));
            }
            Ok(())
        })();
        let _ = sender.send(result);
        drop(activity);
    });
    poll_deep_dismiss(app.as_weak(), context, lease, id, action, original, category, prompt, marker, receiver, Some(worker));
}
fn poll_deep_dismiss(
    weak: Weak<AppWindow>, context: AppContext, lease: NamespaceLease, id: String,
    action: DeepDismissAction, original: String, category: String, prompt: String, marker: String,
    receiver: mpsc::Receiver<std::result::Result<(), ApiError>>, mut worker: Option<std::thread::JoinHandle<()>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = match receiver.try_recv() {
            Ok(value) => value,
            Err(TryRecvError::Empty) => {
                poll_deep_dismiss(weak, context, lease, id, action, original, category, prompt, marker, receiver, worker);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(transition_error("关闭任务的本地确认中断，恢复记录已保留")),
        };
        if let Some(worker) = worker.take() {
            if worker.join().is_err() { clear_prompt_optimization_polling_if_matches(&context, &marker); return; }
        }
        let Some(app) = weak.upgrade() else { clear_prompt_optimization_polling_if_matches(&context, &marker); return; };
        match result {
            Ok(()) => queue_deep_dismiss(&app, context, lease, id, action, original, category, prompt, marker),
            Err(error) => {
                clear_prompt_optimization_polling_if_matches(&context, &marker);
                let _ = context.apply_user_completion(&lease, || app.global::<AppState>().set_deep_optimization_error(error.user_message().into()));
            }
        }
    });
}
fn queue_deep_dismiss(
    app: &AppWindow, context: AppContext, lease: NamespaceLease, id: String,
    action: DeepDismissAction, original: String, category: String, prompt: String, marker: String,
) {
    let Some(writer) = context.store.borrow().private_persistence.clone() else {
        clear_prompt_optimization_polling_if_matches(&context, &marker);
        deep_current_error(app, &context, &transition_error("本地状态尚未激活，恢复记录已保留")); return;
    };
    let mut prepared = match writer.prepare_ordered_save() {
        Ok(value) => Some(value),
        Err(error) => { clear_prompt_optimization_polling_if_matches(&context, &marker); deep_current_error(app, &context, &transition_error(error)); return; }
    };
    let queued = context.apply_user_completion(&lease, || {
        if app.global::<AppState>().get_deep_optimization_job_id().as_str() != id { return None; }
        let owner = lease.namespace.user_public_id();
        {
            let mut store = context.store.borrow_mut();
            remove_prompt_optimization_job_for_owner_if_matches(&mut store, owner, Some(&id));
            if matches!(action, DeepDismissAction::Restore) && !original.trim().is_empty()
                && current_workspace_category(app) == category && app.global::<AppState>().get_prompt().as_str() == prompt {
                store.deep_prompt_bindings.remove(&category);
                set_prompt_draft_for_category(&mut store.prompt_drafts, &category, original.clone());
            }
        }
        Some(prepared.take().expect("single guarded enqueue").enqueue(local_store_data(app, &context.store.borrow())))
    });
    drop(prepared);
    match queued {
        Ok(Some(Ok(receiver))) => poll_deep_dismiss_ack(app.as_weak(), context, lease, id, action, original, category, prompt, marker, receiver),
        Ok(Some(Err(error))) => { clear_prompt_optimization_polling_if_matches(&context, &marker); deep_current_error(app, &context, &transition_error(error)); }
        _ => clear_prompt_optimization_polling_if_matches(&context, &marker),
    }
}
fn poll_deep_dismiss_ack<E: std::fmt::Display + 'static>(
    weak: Weak<AppWindow>, context: AppContext, lease: NamespaceLease, id: String,
    action: DeepDismissAction, original: String, category: String, prompt: String, marker: String,
    receiver: mpsc::Receiver<std::result::Result<(), E>>,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        let success = match receiver.try_recv() {
            Ok(Ok(())) => true,
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => false,
            Err(TryRecvError::Empty) => {
                poll_deep_dismiss_ack(weak, context, lease, id, action, original, category, prompt, marker, receiver);
                return;
            }
        };
        clear_prompt_optimization_polling_if_matches(&context, &marker);
        let Some(app) = weak.upgrade() else { return; };
        let _ = context.apply_user_completion(&lease, || {
            let state = app.global::<AppState>();
            if state.get_deep_optimization_job_id().as_str() != id { return; }
            if !success {
                state.set_deep_optimization_error("本地关闭状态未能确认保存，原记录已保留".into());
                return;
            }
            clear_prompt_optimization_job(&app, &context);
            match action {
                DeepDismissAction::Close => state.set_deep_optimization_open(false),
                DeepDismissAction::New => {
                    state.set_deep_optimization_stage("settings".into());
                    state.set_deep_optimization_original_prompt(state.get_prompt());
                    state.set_deep_optimization_feedback("".into());
                    state.set_deep_optimization_feedback_stable(false);
                    state.set_deep_optimization_stable_feedback_summary("".into());
                    state.set_deep_optimization_result_tab("chinese".into());
                    state.set_deep_optimization_progress(0);
                    state.set_deep_optimization_maximum_credits("".into());
                    state.set_deep_optimization_error("".into());
                }
                DeepDismissAction::Restore => {
                    if !original.trim().is_empty() && current_workspace_category(&app) == category && state.get_prompt().as_str() == prompt {
                        state.set_prompt(original.into());
                        state.set_deep_optimization_applied_chinese("".into());
                        state.set_deep_optimization_applied_english("".into());
                        state.set_generation_status("已恢复并保存深度优化前的提示词".into());
                    }
                }
            }
        });
    });
}
fn visible_deep_recovery_row(rows: &[PendingPromptOptimizationRecord]) -> Option<PendingPromptOptimizationRecord> {
    let ancestors: BTreeSet<&str> = rows.iter().filter_map(|row| match &row.operation {
        PendingPromptOptimizationOperation::Retry { source_job_id } => Some(source_job_id.as_str()),
        _ => None,
    }).collect();
    let visible = |row: &&PendingPromptOptimizationRecord| !row.presentation_dismissed
        && (row.server_job_id.is_empty() || !ancestors.contains(row.server_job_id.as_str()));
    rows.iter().filter(visible).find(|row| row.server_job_id.is_empty())
        .or_else(|| rows.iter().rev().filter(visible).next()).cloned()
}

#[derive(Clone, Copy)]
enum PromptRequestEffect {
    Start,
    Refresh,
    ApplyResult,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptOptimizationScopeDisposition {
    Current,
    CapturedTerminal,
    Stale,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
enum PromptOptimizationRecoveryCandidate {
    Pending(CreatePromptOptimization),
    Owned(String),
    LegacyUnverified(String),
    None,
}

const LEGACY_PROMPT_RECOVERY_PRESERVED_MESSAGE: &str =
    "检测到旧版缺少完整归属或付款账号证明的深度优化记录，已安全保留，未重新计费或绑定";

pub(super) fn wire_prompt_optimization_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_open_deep_optimization(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open_prompt_optimization(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_close_deep_optimization(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            dismiss_deep_presentation(&app, context.clone(), DeepDismissAction::Close);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_start_deep_optimization(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_prompt_optimization(&app, context.clone());
        });
    }

    wire_simple_prompt_action(app, context.clone(), "pause");
    wire_simple_prompt_action(app, context.clone(), "resume");
    wire_simple_prompt_action(app, context.clone(), "cancel");
    wire_simple_prompt_action(app, context.clone(), "retry");

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_continue_deep_optimization(move |with_feedback| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(session_scope) = current_prompt_optimization_session_scope(&context) else {
                clear_prompt_optimization_account_state(&app, &context);
                app.global::<AppState>()
                    .set_deep_optimization_error("登录状态已失效，请重新登录".into());
                return;
            };
            let state = app.global::<AppState>();
            let id = state.get_deep_optimization_job_id().to_string();
            if id.is_empty() {
                return;
            }
            let feedback = if with_feedback {
                let value = state.get_deep_optimization_feedback().trim().to_string();
                (!value.is_empty()).then_some(value)
            } else {
                None
            };
            let feedback_scope = feedback.as_ref().map(|_| {
                if state.get_deep_optimization_feedback_stable() {
                    "stable".to_string()
                } else {
                    "round".to_string()
                }
            });
            if !deep_ui(&context, || {
                state.set_deep_optimization_stage("running".into());
                state.set_deep_optimization_error("".into());
                state.set_deep_optimization_status_message("正在提交下一轮优化...".into());
            }) { return; }
            let request_id = Uuid::new_v4().simple().to_string();
            run_prompt_request_scoped(
                &app,
                context.clone(),
                session_scope,
                PromptRequestEffect::Refresh,
                Some(id.clone()),
                move |api, scope| {
                    api.review_scoped(
                        &id,
                        &request_id,
                        "continue_step",
                        feedback.as_deref(),
                        feedback_scope.as_deref(),
                        &scope,
                    )
                },
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_clear_deep_optimization_stable_feedback(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = app
                .global::<AppState>()
                .get_deep_optimization_job_id()
                .to_string();
            if id.is_empty() {
                return;
            }
            let Some(session_scope) = current_prompt_optimization_session_scope(&context) else {
                clear_prompt_optimization_account_state(&app, &context);
                return;
            };
            let request_id = Uuid::new_v4().simple().to_string();
            run_prompt_request_scoped(
                &app,
                context.clone(),
                session_scope,
                PromptRequestEffect::Refresh,
                Some(id.clone()),
                move |api, scope| {
                    api.review_scoped(
                        &id,
                        &request_id,
                        "clear_stable_feedback",
                        None,
                        None,
                        &scope,
                    )
                },
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_use_deep_optimization_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = app
                .global::<AppState>()
                .get_deep_optimization_job_id()
                .to_string();
            if id.is_empty() {
                return;
            }
            let Some(session_scope) = current_prompt_optimization_session_scope(&context) else {
                clear_prompt_optimization_account_state(&app, &context);
                return;
            };
            let request_id = Uuid::new_v4().simple().to_string();
            run_prompt_request_scoped(
                &app,
                context.clone(),
                session_scope,
                PromptRequestEffect::ApplyResult,
                Some(id.clone()),
                move |api, scope| {
                    api.review_scoped(
                        &id,
                        &request_id,
                        "use_current",
                        None,
                        None,
                        &scope,
                    )
                },
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_keep_original_deep_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = app
                .global::<AppState>()
                .get_deep_optimization_job_id()
                .to_string();
            if id.is_empty() {
                return;
            }
            let Some(session_scope) = current_prompt_optimization_session_scope(&context) else {
                clear_prompt_optimization_account_state(&app, &context);
                return;
            };
            let request_id = Uuid::new_v4().simple().to_string();
            run_prompt_request_scoped(
                &app,
                context.clone(),
                session_scope,
                PromptRequestEffect::Refresh,
                Some(id.clone()),
                move |api, scope| {
                    api.review_scoped(
                        &id,
                        &request_id,
                        "keep_original",
                        None,
                        None,
                        &scope,
                    )
                },
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_begin_new_deep_optimization(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            dismiss_deep_presentation(&app, context.clone(), DeepDismissAction::New);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_invalidate_deep_prompt_binding(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let Some(session) = current_prompt_optimization_session_scope(&context) else { return; };
            let Ok(lease) = context.namespace_for(&session) else { return; };
            if context.apply_user_completion(&lease, || {
                context.store.borrow_mut().deep_prompt_bindings.remove(&current_workspace_category(&app));
            }).is_ok() { persist_deep_projection(&app, &context, &lease); }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_restore_deep_original_prompt(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            dismiss_deep_presentation(&app, context.clone(), DeepDismissAction::Restore);
        });
    }
}

fn wire_simple_prompt_action(app: &AppWindow, context: AppContext, action: &'static str) {
    let state = app.global::<AppState>();
    let app_weak = app.as_weak();
    let handler_context = context.clone();
    let handler = move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let id = app
            .global::<AppState>()
            .get_deep_optimization_job_id()
            .to_string();
        if id.is_empty() {
            return;
        }
        let Some(session_scope) = current_prompt_optimization_session_scope(&handler_context)
        else {
            clear_prompt_optimization_account_state(&app, &handler_context);
            return;
        };
        if action == "retry" {
            retry_prompt_optimization(&app, handler_context.clone(), id);
            return;
        }
        let action_name = action.to_string();
        run_prompt_request_scoped(
            &app,
            handler_context.clone(),
            session_scope,
            PromptRequestEffect::Refresh,
            Some(id.clone()),
            move |api, scope| match action_name.as_str() {
                "pause" => api.pause_scoped(&id, &scope),
                "resume" => api.resume_scoped(&id, &scope),
                "cancel" => api.cancel_scoped(&id, &scope),
                _ => Err(transition_error("未知任务操作")),
            },
        );
    };
    match action {
        "pause" => state.on_pause_deep_optimization(handler),
        "resume" => state.on_resume_deep_optimization(handler),
        "cancel" => state.on_cancel_deep_optimization(handler),
        _ => state.on_retry_deep_optimization(handler),
    }
}

fn open_prompt_optimization(app: &AppWindow, context: AppContext) {
    if !require_online_operation(app, "深度优化") { return; }
    let Some(session) = current_prompt_optimization_session_scope(&context) else { return; };
    if !deep_ui(&context, || {
        app.global::<AppState>().set_deep_optimization_open(true);
        app.global::<AppState>().set_deep_optimization_error("".into());
    }) { return; }
    recover_prompt_optimization_scoped(app, context, session);
}

fn start_prompt_optimization(app: &AppWindow, context: AppContext) {
    if context.prompt_optimization_polling.borrow().as_ref().is_some_and(|key| key.starts_with("work:")) { return; }
    if !require_online_operation(app, "深度优化") { return; }
    let Some(session) = current_prompt_optimization_session_scope(&context) else { return; };
    let capture = match capture_deep_user(&context, session) {
        Ok(value) => value, Err(error) => { deep_current_error(app, &context, &error); return; }
    };
    if context.store.borrow().deep_prompt_pending_requests_by_owner.contains_key(&capture.session.owner_user_id) {
        deep_current_error(app, &context, &transition_error(LEGACY_PROMPT_RECOVERY_PRESERVED_MESSAGE));
        return;
    }
    let state = app.global::<AppState>();
    if !state.get_deep_optimization_job_id().is_empty() {
        recover_prompt_optimization_scoped(app, context, capture.session.clone());
        return;
    }
    let prompt = state.get_prompt().trim().to_string();
    if prompt.is_empty() { deep_current_error(app, &context, &transition_error("请先输入需要优化的提示词")); return; }
    let request = CreatePromptOptimization {
        client_request_id: Uuid::new_v4().to_string(), prompt: prompt.clone(),
        run_mode: state.get_deep_optimization_run_mode().to_string(),
        focus_mode: state.get_deep_optimization_focus_mode().to_string(),
        max_rounds: state.get_deep_optimization_max_rounds().clamp(2, 4), target_score: 90,
    };
    // Capture refusal is retained as a value: an existing ambiguous row must still replay
    // under saved authority even when the current selection cannot admit NEW work.
    let billing = context.capture_billing_action(KnownCapability::Bill).map(|(scope, _, _)| scope);
    let manager = context.billing_context.clone();
    let visible = state.get_deep_optimization_job_id().to_string();
    if !deep_ui(&context, || {
        clear_prompt_optimization_result(&state);
        state.set_deep_optimization_original_prompt(prompt.into());
        state.set_deep_optimization_stage("running".into());
        state.set_deep_optimization_error("".into());
        state.set_deep_optimization_maximum_credits("".into());
        state.set_deep_optimization_status_message("正在保存请求并确认任务...".into());
    }) { return; }
    let key = request.client_request_id.clone();
    spawn_deep_job(app, context, capture, PromptRequestEffect::Start, visible, move |backend, authority, session| {
        if let Some(saved) = load_pending_prompt_optimizations_for_namespace(authority).map_err(transition_error)?
            .into_iter().find(|row| row.server_job_id.is_empty()) {
            return replay_or_get_deep(backend, authority, session, saved).map(Some);
        }
        let billing = billing?;
        execute_new_deep(backend, authority, &manager, &billing, PendingPromptOptimizationOperation::Create { request }, key).map(Some)
    });
}
fn retry_prompt_optimization(app: &AppWindow, context: AppContext, source_job_id: String) {
    let Some(session) = current_prompt_optimization_session_scope(&context) else { return; };
    if context.store.borrow().deep_prompt_jobs_by_owner.get(&session.owner_user_id).is_some_and(|id| id != &source_job_id) {
        recover_prompt_optimization_scoped(app, context, session);
        return;
    }
    let capture = match capture_deep_user(&context, session) {
        Ok(value) => value, Err(error) => { deep_current_error(app, &context, &error); return; }
    };
    let billing = context.capture_billing_action(KnownCapability::Bill).map(|(scope, _, _)| scope);
    let manager = context.billing_context.clone();
    let expected = source_job_id.clone();
    let key = Uuid::new_v4().to_string();
    spawn_deep_job(app, context, capture, PromptRequestEffect::Start, expected, move |backend, authority, session| {
        if let Some(saved) = load_pending_prompt_optimizations_for_namespace(authority).map_err(transition_error)?
            .into_iter().find(|row| row.server_job_id.is_empty()) {
            return replay_or_get_deep(backend, authority, session, saved).map(Some);
        }
        execute_new_deep(backend, authority, &manager, &billing?, PendingPromptOptimizationOperation::Retry { source_job_id }, key).map(Some)
    });
}

fn run_prompt_request_scoped<F>(
    app: &AppWindow, context: AppContext, session_scope: SessionScope,
    effect: PromptRequestEffect, expected_job_id: Option<String>, request: F,
) where F: FnOnce(PromptOptimizationApi, SessionScope) -> std::result::Result<PromptOptimizationDetail, ApiError> + Send + 'static {
    let Some(id) = expected_job_id else { return; };
    let capture = match capture_deep_user(&context, session_scope) {
        Ok(value) => value, Err(error) => { deep_current_error(app, &context, &error); return; }
    };
    spawn_deep_job(app, context, capture, effect, id.clone(), move |backend, authority, session| {
        let saved = deep_record_for_job(authority, &id)?;
        // Existing resources stay creator/session scoped and have no selected-group header.
        let _network = backend.api.begin_user_work(session)?;
        let detail = request(PromptOptimizationApi::new(backend.api.clone()), session.clone())?;
        accept_deep_detail(authority, session, saved, detail).map(Some)
    });
}

#[cfg(test)]
fn prompt_optimization_detail_matches_request(
    current_job_id: &str,
    expected_job_id: Option<&str>,
    response_job_id: &str,
    effect: PromptRequestEffect,
) -> bool {
    if response_job_id.trim().is_empty() {
        return false;
    }
    match expected_job_id {
        Some(expected) => response_job_id == expected && current_job_id == expected,
        None => matches!(effect, PromptRequestEffect::Start),
    }
}

fn current_prompt_optimization_session_scope(context: &AppContext) -> Option<SessionScope> {
    let owner_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone()
        .filter(|value| !value.trim().is_empty())?;
    context
        .backend
        .as_ref()?
        .api
        .session()
        .scope_for_user(&owner_user_id)
}

fn prompt_optimization_scope_matches_context(
    context: &AppContext,
    session_scope: &SessionScope,
) -> bool {
    let current_owner = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone();
    current_owner.as_deref() == Some(session_scope.owner_user_id.as_str())
        && context.backend.as_ref().is_some_and(|backend| {
            backend.api.session().is_scope_current(session_scope)
        })
}

fn prompt_optimization_scope_disposition(
    context: &AppContext,
    session_scope: &SessionScope,
) -> PromptOptimizationScopeDisposition {
    if prompt_optimization_scope_matches_context(context, session_scope) {
        PromptOptimizationScopeDisposition::Current
    } else if terminal_auth_scope_matches_context(context, session_scope) {
        PromptOptimizationScopeDisposition::CapturedTerminal
    } else {
        PromptOptimizationScopeDisposition::Stale
    }
}

fn handle_prompt_optimization_terminal(
    app: &AppWindow,
    context: &AppContext,
    session_scope: &SessionScope,
) {
    if prompt_optimization_scope_disposition(context, session_scope)
        != PromptOptimizationScopeDisposition::CapturedTerminal
    {
        return;
    }
    clear_prompt_optimization_account_state(app, context);
    sign_out_locally(
        app,
        context,
        true,
        Some(session_scope.auth_epoch),
    );
}

fn prompt_optimization_poll_key(session_scope: &SessionScope, id: &str) -> String {
    format!(
        "{}:{}:{}",
        session_scope.owner_user_id, session_scope.auth_epoch, id
    )
}

fn clear_prompt_optimization_polling_if_matches(context: &AppContext, key: &str) {
    let mut polling = context.prompt_optimization_polling.borrow_mut();
    if polling.as_deref() == Some(key) {
        polling.take();
    }
}

#[cfg(test)]
fn prompt_optimization_recovery_candidate(
    store: &Store,
    owner_user_id: &str,
) -> PromptOptimizationRecoveryCandidate {
    let owner_user_id = owner_user_id.trim();
    if owner_user_id.is_empty() {
        return PromptOptimizationRecoveryCandidate::None;
    }
    if let Some(request) = store
        .deep_prompt_pending_requests_by_owner
        .get(owner_user_id)
        .filter(|request| {
            !request.client_request_id.trim().is_empty() && !request.prompt.trim().is_empty()
        })
    {
        return PromptOptimizationRecoveryCandidate::Pending(request.clone());
    }
    if let Some(job_id) = store
        .deep_prompt_jobs_by_owner
        .get(owner_user_id)
        .map(String::as_str)
        .map(str::trim)
        .filter(|job_id| !job_id.is_empty())
    {
        return PromptOptimizationRecoveryCandidate::Owned(job_id.to_string());
    }
    let legacy_job_id = store.legacy_deep_prompt_job_id.trim();
    if legacy_job_id.is_empty() {
        PromptOptimizationRecoveryCandidate::None
    } else {
        PromptOptimizationRecoveryCandidate::LegacyUnverified(legacy_job_id.to_string())
    }
}

#[cfg(test)]
fn store_prompt_optimization_job_for_owner(
    store: &mut Store,
    owner_user_id: &str,
    job_id: &str,
    verified_legacy_job_id: Option<&str>,
) -> bool {
    let owner_user_id = owner_user_id.trim();
    let job_id = job_id.trim();
    if owner_user_id.is_empty() || job_id.is_empty() {
        return false;
    }
    store
        .deep_prompt_jobs_by_owner
        .insert(owner_user_id.to_string(), job_id.to_string());
    if verified_legacy_job_id.is_some_and(|verified| {
        !verified.trim().is_empty()
            && verified.trim() == store.legacy_deep_prompt_job_id.trim()
            && verified.trim() == job_id
    }) {
        store.legacy_deep_prompt_job_id.clear();
    }
    true
}

fn remove_prompt_optimization_job_for_owner_if_matches(
    store: &mut Store,
    owner_user_id: &str,
    expected_job_id: Option<&str>,
) -> bool {
    let owner_user_id = owner_user_id.trim();
    if owner_user_id.is_empty() {
        return false;
    }
    let should_remove = store
        .deep_prompt_jobs_by_owner
        .get(owner_user_id)
        .is_some_and(|job_id| {
            expected_job_id.map_or(true, |expected| job_id == expected.trim())
        });
    should_remove
        && store
            .deep_prompt_jobs_by_owner
            .remove(owner_user_id)
            .is_some()
}

fn clear_prompt_optimization_job(app: &AppWindow, context: &AppContext) {
    context.prompt_optimization_polling.borrow_mut().take();
    let state = app.global::<AppState>();
    state.set_deep_optimization_job_id("".into());
    clear_prompt_optimization_result(&state);
    let owner_user_id = context
        .current_user_id
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .clone()
        .unwrap_or_default();
    if remove_prompt_optimization_job_for_owner_if_matches(
        &mut context.store.borrow_mut(),
        &owner_user_id,
        None,
    ) {
        // Projection persistence is scheduled by the guarded caller.
    }
}

fn clear_prompt_optimization_result(state: &AppState) {
    state.set_deep_optimization_chinese_prompt("".into());
    state.set_deep_optimization_english_prompt("".into());
    state.set_deep_optimization_highlighted_original(styled_markdown(""));
    state.set_deep_optimization_highlighted_chinese(styled_markdown(""));
    state.set_deep_optimization_change_summary("".into());
}

/// Clears account-bound in-memory/UI state without deleting the persisted server task id.
/// The persisted id is intentionally retained so the same account can recover the task after
/// signing in again; recovery always verifies it through an account-scoped server request.
pub(super) fn clear_prompt_optimization_account_state(app: &AppWindow, context: &AppContext) {
    context.prompt_optimization_polling.borrow_mut().take();
    let state = app.global::<AppState>();
    state.set_deep_optimization_open(false);
    state.set_deep_optimization_job_id("".into());
    state.set_deep_optimization_stage("settings".into());
    state.set_deep_optimization_progress(0);
    state.set_deep_optimization_current_round(0);
    state.set_deep_optimization_completed_rounds(0);
    state.set_deep_optimization_estimated_seconds(0);
    state.set_deep_optimization_phase_label("".into());
    state.set_deep_optimization_status_message("".into());
    state.set_deep_optimization_stop_reason("".into());
    state.set_deep_optimization_original_prompt("".into());
    state.set_deep_optimization_applied_chinese("".into());
    state.set_deep_optimization_applied_english("".into());
    state.set_deep_optimization_result_tab("chinese".into());
    state.set_deep_optimization_feedback("".into());
    state.set_deep_optimization_feedback_stable(false);
    state.set_deep_optimization_stable_feedback_summary("".into());
    state.set_deep_optimization_current_score(0);
    state.set_deep_optimization_baseline_score(0);
    state.set_deep_optimization_consumed_credits("0".into());
    state.set_deep_optimization_maximum_credits("".into());
    state.set_deep_optimization_rounds(ModelRc::new(VecModel::from(
        Vec::<DeepOptimizationRoundView>::new(),
    )));
    state.set_deep_optimization_error("".into());
    state.set_deep_optimization_can_pause(false);
    state.set_deep_optimization_can_resume(false);
    state.set_deep_optimization_can_retry(false);
    state.set_deep_optimization_can_cancel(false);
    state.set_deep_optimization_can_continue(false);
    state.set_deep_optimization_can_apply(false);
    state.set_deep_optimization_can_clear_stable_feedback(false);
    clear_prompt_optimization_result(&state);
}

fn begin_prompt_optimization_polling(app: &AppWindow, context: AppContext, session_scope: SessionScope, id: String) {
    let Ok(lease) = context.namespace_for(&session_scope) else { return; };
    let key = prompt_optimization_poll_key(&session_scope, &id);
    *context.prompt_optimization_polling.borrow_mut() = Some(key.clone());
    let weak = app.as_weak();
    slint::Timer::single_shot(Duration::from_secs(2), move || {
        let Some(app) = weak.upgrade() else { return; };
        if context.prompt_optimization_polling.borrow().as_deref() != Some(key.as_str())
            || context.namespace_for(&session_scope).ok().as_ref() != Some(&lease)
            || app.global::<AppState>().get_deep_optimization_job_id().as_str() != id { return; }
        clear_prompt_optimization_polling_if_matches(&context, &key);
        let expected = id.clone();
        run_prompt_request_scoped(&app, context, session_scope, PromptRequestEffect::Refresh, Some(expected),
            move |api, scope| api.get_scoped(&id, &scope));
    });
}

pub(super) fn recover_prompt_optimization(app: &AppWindow, context: AppContext) {
    let Some(session) = current_prompt_optimization_session_scope(&context) else { return; };
    recover_prompt_optimization_scoped(app, context, session);
}
fn recover_prompt_optimization_scoped(app: &AppWindow, context: AppContext, session: SessionScope) {
    let capture = match capture_deep_user(&context, session.clone()) {
        Ok(value) => value, Err(error) => { deep_current_error(app, &context, &error); return; }
    };
    let visible = app.global::<AppState>().get_deep_optimization_job_id().to_string();
    spawn_deep_job(app, context, capture, PromptRequestEffect::Refresh, visible, move |backend, authority, session| {
        let rows = load_pending_prompt_optimizations_for_namespace(authority).map_err(transition_error)?;
        let selected = visible_deep_recovery_row(&rows);
        match selected {
            Some(saved) => replay_or_get_deep(backend, authority, session, saved).map(Some),
            None => Ok(None),
        }
    });
}

fn displayed_best_score(detail: &PromptOptimizationDetail) -> i32 {
    detail
        .best_score
        .or(detail.baseline_score)
        .or(detail.result_score)
        .unwrap_or(0)
}

fn apply_prompt_optimization_detail(app: &AppWindow, detail: &PromptOptimizationDetail) {
    let state = app.global::<AppState>();
    state.set_deep_optimization_job_id(detail.id.clone().into());
    state.set_deep_optimization_run_mode(detail.run_mode.clone().into());
    state.set_deep_optimization_focus_mode(detail.focus_mode.clone().into());
    state.set_deep_optimization_max_rounds(detail.max_rounds);
    state.set_deep_optimization_target_score(detail.target_score);
    state.set_deep_optimization_completed_rounds(detail.completed_rounds);
    state.set_deep_optimization_current_round(
        if matches!(detail.status.as_str(), "queued" | "processing") {
            (detail.completed_rounds + 1).min(detail.max_rounds)
        } else {
            detail.current_round
        },
    );
    state.set_deep_optimization_progress(detail.progress_percent);
    let remaining_rounds = (detail.max_rounds - detail.completed_rounds).max(0);
    state.set_deep_optimization_estimated_seconds(
        if matches!(detail.status.as_str(), "queued" | "processing") {
            remaining_rounds * 45
        } else {
            0
        },
    );
    state.set_deep_optimization_baseline_score(detail.baseline_score.unwrap_or(0));
    state.set_deep_optimization_current_score(displayed_best_score(detail));
    state.set_deep_optimization_phase_label(phase_label(&detail.phase).into());
    state.set_deep_optimization_status_message(phase_message(&detail.phase).into());
    state
        .set_deep_optimization_stop_reason(stop_reason_label(detail.stop_reason.as_deref()).into());
    state.set_deep_optimization_consumed_credits(detail.pricing.consumed_credits.clone().into());
    state.set_deep_optimization_maximum_credits(detail.pricing.maximum_credits.clone().into());
    state.set_deep_optimization_can_pause(detail.can_pause);
    state.set_deep_optimization_can_resume(detail.can_resume);
    state.set_deep_optimization_can_retry(detail.can_retry);
    state.set_deep_optimization_can_cancel(detail.can_cancel);
    state.set_deep_optimization_can_continue(detail.can_continue);
    state.set_deep_optimization_can_apply(detail.can_apply);
    state.set_deep_optimization_can_clear_stable_feedback(detail.can_clear_stable_feedback);
    let original = detail.original_prompt.as_deref().unwrap_or_default();
    state.set_deep_optimization_original_prompt(original.into());
    match detail.result.as_ref() {
        Some(result) => {
            state.set_deep_optimization_chinese_prompt(result.chinese_prompt.clone().into());
            state.set_deep_optimization_english_prompt(result.english_prompt.clone().into());
            let comparison_base = best_result_comparison_base(detail);
            let (highlighted_original, highlighted_chinese) =
                highlighted_prompt_markdown(comparison_base, &result.chinese_prompt);
            state
                .set_deep_optimization_highlighted_original(styled_markdown(&highlighted_original));
            state.set_deep_optimization_highlighted_chinese(styled_markdown(&highlighted_chinese));
        }
        None => {
            state.set_deep_optimization_chinese_prompt(original.into());
            state.set_deep_optimization_english_prompt("".into());
            let plain_original = styled_markdown(&escape_prompt_markdown(original));
            state.set_deep_optimization_highlighted_original(plain_original.clone());
            state.set_deep_optimization_highlighted_chinese(plain_original);
        }
    }
    state
        .set_deep_optimization_feedback(detail.pending_feedback.clone().unwrap_or_default().into());
    state.set_deep_optimization_feedback_stable(false);
    state.set_deep_optimization_stable_feedback_summary(detail.stable_feedback.join("；").into());
    state.set_deep_optimization_error(
        detail
            .failure
            .as_ref()
            .map(|failure| failure.message.clone())
            .unwrap_or_default()
            .into(),
    );

    state.set_deep_optimization_change_summary(optimization_change_summary(detail).into());

    let rounds = detail
        .rounds
        .iter()
        .map(|round| {
            let candidate_note = match (round.candidate_score, round.score_after, round.accepted) {
                (Some(candidate), Some(best), false) if candidate != best => {
                    format!(" · 本轮候选 {candidate} 分（未采用）")
                }
                _ => String::new(),
            };
            DeepOptimizationRoundView {
                round: round.round,
                status: round.status.clone().into(),
                phase_label: round_status_label(&round.status).into(),
                score_before: round.score_before,
                score_after: round.score_after.unwrap_or(0),
                score_label: match round.score_after {
                    Some(score) => format!(
                        "{} → {} · {} 积分{}{}",
                        round.score_before,
                        score,
                        round.credit_cost,
                        candidate_note,
                        round
                            .top_band
                            .as_ref()
                            .filter(|review| review.triggered)
                            .map(|review| {
                                if review.qualifies {
                                    " · 高分复核通过"
                                } else {
                                    " · 高分复核未通过"
                                }
                            })
                            .unwrap_or(""),
                    )
                    .into(),
                    None => "正在处理本轮内容".into(),
                },
                summary: round.major_changes.join("；").into(),
            }
        })
        .collect::<Vec<_>>();
    state.set_deep_optimization_rounds(ModelRc::new(VecModel::from(rounds)));

    let stage = match detail.status.as_str() {
        "queued" | "processing" => "running",
        "manual_review" => "review",
        "completed" => "complete",
        "paused" => "paused",
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ => "running",
    };
    state.set_deep_optimization_stage(stage.into());
}

fn displayed_result_round(detail: &PromptOptimizationDetail) -> Option<&PromptOptimizationRound> {
    detail
        .result_round_no
        .or(detail.best_round_no)
        .and_then(|number| detail.rounds.iter().find(|round| round.round == number))
}

fn best_result_comparison_base(detail: &PromptOptimizationDetail) -> &str {
    let Some(result_round) = displayed_result_round(detail) else {
        return detail.original_prompt.as_deref().unwrap_or_default();
    };
    detail
        .rounds
        .iter()
        .filter(|round| {
            round.accepted
                && round.round < result_round.round
                && round
                    .chinese_prompt
                    .as_deref()
                    .is_some_and(|prompt| !prompt.trim().is_empty())
        })
        .max_by_key(|round| round.round)
        .and_then(|round| round.chinese_prompt.as_deref())
        .or(detail.original_prompt.as_deref())
        .unwrap_or_default()
}

fn optimization_change_summary(detail: &PromptOptimizationDetail) -> String {
    if detail.stop_reason.as_deref() == Some("target_reached") && detail.completed_rounds == 0 {
        return "原提示词已通过高分复核并达到目标，无需额外优化。".to_string();
    }
    if detail.result.is_none() {
        return "本轮候选未超过原提示词，已保留原提示词。".to_string();
    }
    let Some(round) = displayed_result_round(detail) else {
        return "已生成当前最佳版本。".to_string();
    };
    let rejected_notice = if !detail.result_accepted {
        detail
            .result_score
            .zip(detail.best_score)
            .filter(|(candidate, best)| candidate < best)
            .map(|(candidate, best)| format!("本轮候选 {candidate} 分，未替换当前最佳 {best} 分。"))
    } else {
        None
    };
    if let Some(review) = round
        .top_band
        .as_ref()
        .filter(|review| !review.qualifies && !review.blocking_issues.is_empty())
    {
        let review_summary = format!("高分复核待改进：{}", review.blocking_issues.join("；"));
        return rejected_notice
            .map(|notice| format!("{notice}\n{review_summary}"))
            .unwrap_or(review_summary);
    }
    if !round.major_changes.is_empty() {
        let changes = round
            .major_changes
            .iter()
            .map(|item| format!("• {item}"))
            .collect::<Vec<_>>()
            .join("\n");
        return rejected_notice
            .map(|notice| format!("{notice}\n{changes}"))
            .unwrap_or(changes);
    }
    if let Some(notice) = rejected_notice {
        return notice;
    }
    if !round.issues.is_empty() {
        return round.issues.join("；");
    }
    "已生成当前最佳版本。".to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PromptDiffPiece {
    text: String,
    changed: bool,
}

fn highlighted_prompt_markdown(original: &str, optimized: &str) -> (String, String) {
    let original_tokens = prompt_diff_tokens(original);
    let optimized_tokens = prompt_diff_tokens(optimized);
    let mut matches = Vec::new();
    collect_lcs_matches(&original_tokens, &optimized_tokens, 0, 0, &mut matches);
    let original_pieces = prompt_diff_pieces(&original_tokens, &matches, true);
    let optimized_pieces = prompt_diff_pieces(&optimized_tokens, &matches, false);
    (
        prompt_diff_markdown(&original_pieces, "#d97706"),
        prompt_diff_markdown(&optimized_pieces, "#5147e5"),
    )
}

fn prompt_diff_tokens(prompt: &str) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum TokenKind {
        Word,
        Whitespace,
    }

    let mut tokens = Vec::new();
    let mut buffer = String::new();
    let mut buffer_kind = None;
    let flush = |tokens: &mut Vec<String>, buffer: &mut String| {
        if !buffer.is_empty() {
            tokens.push(std::mem::take(buffer));
        }
    };

    for character in prompt.chars() {
        let kind = if character.is_whitespace() {
            Some(TokenKind::Whitespace)
        } else if character.is_alphanumeric() && !is_cjk_character(character) {
            Some(TokenKind::Word)
        } else {
            None
        };

        match kind {
            Some(kind) if buffer_kind == Some(kind) => buffer.push(character),
            Some(kind) => {
                flush(&mut tokens, &mut buffer);
                buffer.push(character);
                buffer_kind = Some(kind);
            }
            None => {
                flush(&mut tokens, &mut buffer);
                buffer_kind = None;
                tokens.push(character.to_string());
            }
        }
    }
    flush(&mut tokens, &mut buffer);
    tokens
}

fn is_cjk_character(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2ebef
            | 0x30000..=0x323af
    )
}

fn collect_lcs_matches(
    original: &[String],
    optimized: &[String],
    original_offset: usize,
    optimized_offset: usize,
    matches: &mut Vec<(usize, usize)>,
) {
    if original.is_empty() || optimized.is_empty() {
        return;
    }
    if original.len() == 1 {
        if let Some(index) = optimized.iter().position(|token| token == &original[0]) {
            matches.push((original_offset, optimized_offset + index));
        }
        return;
    }

    let middle = original.len() / 2;
    let left_lengths = lcs_prefix_lengths(&original[..middle], optimized);
    let right_lengths = lcs_suffix_lengths(&original[middle..], optimized);
    let mut optimized_split = 0;
    let mut best_length = 0;
    for index in 0..=optimized.len() {
        let length = left_lengths[index] + right_lengths[index];
        if length > best_length {
            best_length = length;
            optimized_split = index;
        }
    }

    collect_lcs_matches(
        &original[..middle],
        &optimized[..optimized_split],
        original_offset,
        optimized_offset,
        matches,
    );
    collect_lcs_matches(
        &original[middle..],
        &optimized[optimized_split..],
        original_offset + middle,
        optimized_offset + optimized_split,
        matches,
    );
}

fn lcs_prefix_lengths(original: &[String], optimized: &[String]) -> Vec<usize> {
    let mut previous = vec![0; optimized.len() + 1];
    for original_token in original {
        let mut current = vec![0; optimized.len() + 1];
        for (index, optimized_token) in optimized.iter().enumerate() {
            current[index + 1] = if original_token == optimized_token {
                previous[index] + 1
            } else {
                current[index].max(previous[index + 1])
            };
        }
        previous = current;
    }
    previous
}

fn lcs_suffix_lengths(original: &[String], optimized: &[String]) -> Vec<usize> {
    let mut next = vec![0; optimized.len() + 1];
    for original_token in original.iter().rev() {
        let mut current = vec![0; optimized.len() + 1];
        for index in (0..optimized.len()).rev() {
            current[index] = if original_token == &optimized[index] {
                next[index + 1] + 1
            } else {
                current[index + 1].max(next[index])
            };
        }
        next = current;
    }
    next
}

fn prompt_diff_pieces(
    tokens: &[String],
    matches: &[(usize, usize)],
    use_original_index: bool,
) -> Vec<PromptDiffPiece> {
    let mut pieces = Vec::new();
    let mut cursor = 0;
    for &(original_index, optimized_index) in matches {
        let matched_index = if use_original_index {
            original_index
        } else {
            optimized_index
        };
        if matched_index > cursor {
            push_prompt_diff_piece(&mut pieces, tokens[cursor..matched_index].concat(), true);
        }
        if let Some(token) = tokens.get(matched_index) {
            push_prompt_diff_piece(&mut pieces, token.clone(), false);
        }
        cursor = matched_index.saturating_add(1);
    }
    if cursor < tokens.len() {
        push_prompt_diff_piece(&mut pieces, tokens[cursor..].concat(), true);
    }
    pieces
}

fn push_prompt_diff_piece(pieces: &mut Vec<PromptDiffPiece>, text: String, changed: bool) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = pieces.last_mut().filter(|last| last.changed == changed) {
        last.text.push_str(&text);
    } else {
        pieces.push(PromptDiffPiece { text, changed });
    }
}

fn prompt_diff_markdown(pieces: &[PromptDiffPiece], color: &str) -> String {
    let mut markdown = String::new();
    for piece in pieces {
        if !piece.changed {
            markdown.push_str(&escape_prompt_markdown(&piece.text));
            continue;
        }
        let mut lines = piece.text.split_inclusive('\n').peekable();
        while let Some(line) = lines.next() {
            let (content, newline) = line
                .strip_suffix('\n')
                .map(|content| (content, "\n"))
                .unwrap_or((line, ""));
            if !content.is_empty() {
                markdown.push_str("<font color='");
                markdown.push_str(color);
                markdown.push_str("'><u>");
                markdown.push_str(&escape_prompt_markdown(content));
                markdown.push_str("</u></font>");
            }
            markdown.push_str(newline);
            if lines.peek().is_none() {
                break;
            }
        }
    }
    markdown
}

fn escape_prompt_markdown(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\\' | '*' | '_' | '[' | ']' | '(' | ')' | '#' | '`' | '!' | '|' | '+' | '-' | '.'
            | '=' | '~' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn styled_markdown(markdown: &str) -> slint::private_unstable_api::re_exports::StyledText {
    use slint::private_unstable_api::re_exports::{parse_markdown, StyledText};
    parse_markdown(markdown, &[] as &[StyledText])
}

fn phase_label(phase: &str) -> &'static str {
    match phase {
        "queued" => "任务排队中",
        "baseline_scoring" => "正在评估原提示词",
        "optimizing" => "优化模型正在生成中英文版本",
        "judging" => "正在进行双重评分与语义偏移检查",
        "top_band_review" => "正在进行图片高分复核",
        "review" => "等待确认优化结果",
        "paused" => "优化已暂停",
        "failed" => "本轮优化暂时失败",
        "completed" => "优化结果已应用",
        "cancelled" => "优化已取消",
        _ => "正在同步任务状态",
    }
}

fn phase_message(phase: &str) -> &'static str {
    match phase {
        "queued" => "积分已预留，正在等待可用任务资源",
        "baseline_scoring" => "从主体、构图、光线、风格和约束等维度评分",
        "optimizing" => "生成中文可读版与模型英文版",
        "judging" => "两次独立评分取平均，避免单次评价偏差",
        "top_band_review" => "复核主体保真、中英文一致性和视觉指令可执行性",
        "review" => "优化版本已就绪，确认后才会替换当前提示词",
        "paused" => "已完成的轮次和结果都会保留",
        "failed" => "不会重复扣除未完成轮次的积分",
        _ => "",
    }
}

fn stop_reason_label(reason: Option<&str>) -> &'static str {
    match reason {
        Some("target_reached") => "已达到目标分数",
        Some("max_rounds") => "已达到最大轮数",
        Some("no_improvement") => "连续两轮提升不足",
        Some("semantic_drift") => "检测到语义偏移",
        Some("round_completed") => "本轮优化已完成",
        Some("user_paused") => "已按你的要求暂停",
        Some("user_applied") => "已使用当前版本",
        _ => "优化结果等待确认",
    }
}

fn round_status_label(status: &str) -> &'static str {
    match status {
        "completed" => "本轮已完成",
        "failed" => "本轮可重试",
        "cancelled" => "本轮已取消",
        _ => "正在处理",
    }
}

#[cfg(test)]
fn should_fallback_to_active_prompt_optimization(error: &ApiError) -> bool {
    error.code() == Some("prompt_optimization_not_found")
}

#[cfg(test)]
mod prompt_diff_tests {
    use super::*;
    use crate::runtime::test_support::MemoryRefreshTokenStore;
    use reqwest::Url;

    fn tokens(access: &str, refresh: &str) -> TokenSet {
        TokenSet {
            access_token: access.to_string(),
            access_expires_in_seconds: 1800,
            refresh_token: refresh.to_string(),
            refresh_expires_at: "2099-01-01T00:00:00Z".to_string(),
            token_type: "X-Token".to_string(),
        }
    }

    fn scoped_context(owner_user_id: &str) -> (AppContext, Arc<SessionManager>, SessionScope) {
        let session = Arc::new(SessionManager::new(Arc::new(
            MemoryRefreshTokenStore::default(),
        )));
        let scope = session
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), owner_user_id)
            .unwrap();
        let api = ApiClient::new(
            ApiClientConfig {
                base_url: Url::parse("http://127.0.0.1:1/").unwrap(),
                app_version: "1.0.18".to_string(),
                timeout: Duration::from_millis(50),
            },
            DeviceIdentity {
                id: Uuid::new_v4().to_string(),
                name: "prompt-optimization-test".to_string(),
                platform: "macos".to_string(),
            },
            session.clone(),
        )
        .unwrap();
        let mut context = AppContext::default();
        context.backend = Some(Arc::new(BackendRuntime { api }));
        *context
            .current_user_id
            .lock()
            .unwrap_or_else(|value| value.into_inner()) = Some(owner_user_id.to_string());
        (context, session, scope)
    }

    #[test]
    fn blocked_account_a_action_becomes_stale_after_account_b_is_installed() {
        let (context, session, scope_a) = scoped_context("user-a");
        assert_eq!(
            prompt_optimization_scope_disposition(&context, &scope_a),
            PromptOptimizationScopeDisposition::Current,
        );

        let scope_b = session
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();
        *context
            .current_user_id
            .lock()
            .unwrap_or_else(|value| value.into_inner()) = Some("user-b".to_string());

        assert_eq!(
            prompt_optimization_scope_disposition(&context, &scope_a),
            PromptOptimizationScopeDisposition::Stale,
        );
        assert!(session.access_token_for_scope(&scope_a).is_err());
        assert_eq!(
            session.access_token_for_scope(&scope_b).unwrap(),
            "access-b",
        );
    }

    #[test]
    fn terminal_get_or_action_is_captured_for_a_but_cannot_sign_out_b() {
        let (context, session, scope_a) = scoped_context("user-a");
        session.clear_scope(&scope_a).unwrap();
        assert_eq!(
            prompt_optimization_scope_disposition(&context, &scope_a),
            PromptOptimizationScopeDisposition::CapturedTerminal,
        );

        let scope_b = session
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();
        *context
            .current_user_id
            .lock()
            .unwrap_or_else(|value| value.into_inner()) = Some("user-b".to_string());
        assert_eq!(
            prompt_optimization_scope_disposition(&context, &scope_a),
            PromptOptimizationScopeDisposition::Stale,
        );
        assert_eq!(
            session.access_token_for_scope(&scope_b).unwrap(),
            "access-b",
        );
    }

    #[test]
    fn late_detail_cannot_replace_a_new_or_cleared_job() {
        assert!(prompt_optimization_detail_matches_request(
            "job-a",
            Some("job-a"),
            "job-a",
            PromptRequestEffect::Refresh,
        ));
        assert!(!prompt_optimization_detail_matches_request(
            "job-b",
            Some("job-a"),
            "job-a",
            PromptRequestEffect::Refresh,
        ));
        assert!(!prompt_optimization_detail_matches_request(
            "",
            Some("job-a"),
            "job-a",
            PromptRequestEffect::Refresh,
        ));
    }

    #[test]
    fn stale_poll_completion_cannot_clear_the_new_accounts_poll_marker() {
        let context = AppContext::default();
        let scope_a = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 7,
        };
        let scope_b = SessionScope {
            owner_user_id: "user-b".to_string(),
            auth_epoch: 9,
        };
        let key_a = prompt_optimization_poll_key(&scope_a, "job-a");
        let key_b = prompt_optimization_poll_key(&scope_b, "job-b");
        *context.prompt_optimization_polling.borrow_mut() = Some(key_b.clone());

        clear_prompt_optimization_polling_if_matches(&context, &key_a);

        assert_eq!(
            context.prompt_optimization_polling.borrow().as_deref(),
            Some(key_b.as_str()),
        );
    }

    #[test]
    fn recovery_falls_back_only_when_the_stored_task_is_not_visible() {
        let not_found = ApiError::Http {
            status: 404,
            code: "prompt_optimization_not_found".to_string(),
            message: "not found".to_string(),
            request_id: None,
            details: None,
        };
        let terminal = ApiError::Http {
            status: 401,
            code: "session_invalid".to_string(),
            message: "revoked".to_string(),
            request_id: None,
            details: None,
        };
        let network = ApiError::Network {
            message: "offline".to_string(),
            timeout: false,
        };

        assert!(should_fallback_to_active_prompt_optimization(&not_found));
        assert!(!should_fallback_to_active_prompt_optimization(&terminal));
        assert!(!should_fallback_to_active_prompt_optimization(&network));
    }

    #[test]
    fn recovery_records_are_partitioned_and_cleared_per_owner() {
        let mut store = Store::default();

        assert!(store_prompt_optimization_job_for_owner(
            &mut store,
            "user-a",
            "job-a",
            None,
        ));
        assert!(store_prompt_optimization_job_for_owner(
            &mut store,
            "user-b",
            "job-b",
            None,
        ));
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-a"),
            PromptOptimizationRecoveryCandidate::Owned("job-a".to_string()),
        );
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-b"),
            PromptOptimizationRecoveryCandidate::Owned("job-b".to_string()),
        );

        assert!(!remove_prompt_optimization_job_for_owner_if_matches(
            &mut store,
            "user-a",
            Some("different-job"),
        ));
        assert!(remove_prompt_optimization_job_for_owner_if_matches(
            &mut store,
            "user-a",
            Some("job-a"),
        ));
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-a"),
            PromptOptimizationRecoveryCandidate::None,
        );
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-b"),
            PromptOptimizationRecoveryCandidate::Owned("job-b".to_string()),
        );
    }

    #[test]
    fn pending_billable_create_request_is_partitioned_and_replayed_exactly() {
        let request = CreatePromptOptimization {
            client_request_id: "request-a-12345678".to_string(),
            prompt: "persist this prompt".to_string(),
            run_mode: "auto".to_string(),
            focus_mode: "system".to_string(),
            max_rounds: 3,
            target_score: 90,
        };
        let mut store = Store::default();
        store
            .deep_prompt_pending_requests_by_owner
            .insert("user-a".to_string(), request.clone());

        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-a"),
            PromptOptimizationRecoveryCandidate::Pending(request),
        );
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-b"),
            PromptOptimizationRecoveryCandidate::None,
        );
    }

    #[test]
    fn deep_prompt_create_is_persisted_before_the_billable_api_call() {
        super::core_deep_caller_patch_tests::assert_new_create_is_durable();
    }

    #[test]
    fn legacy_recovery_record_stays_quarantined_without_exact_verification() {
        let mut store = Store {
            legacy_deep_prompt_job_id: "legacy-job".to_string(),
            ..Default::default()
        };

        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-b"),
            PromptOptimizationRecoveryCandidate::LegacyUnverified("legacy-job".to_string()),
        );
        assert!(store_prompt_optimization_job_for_owner(
            &mut store,
            "user-b",
            "active-job-b",
            None,
        ));
        assert_eq!(store.legacy_deep_prompt_job_id, "legacy-job");
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-b"),
            PromptOptimizationRecoveryCandidate::Owned("active-job-b".to_string()),
        );
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-a"),
            PromptOptimizationRecoveryCandidate::LegacyUnverified("legacy-job".to_string()),
        );
    }

    #[test]
    fn legacy_local_store_json_does_not_assign_the_job_to_an_owner() {
        let data: LocalStoreData = serde_json::from_str(
            r#"{
                "deep_prompt_job_id": "legacy-job"
            }"#,
        )
        .unwrap();

        assert_eq!(data.deep_prompt_job_id, "legacy-job");
        assert!(data.deep_prompt_jobs_by_owner.is_empty());
    }

    #[test]
    fn exact_scoped_verification_migrates_the_legacy_record_once() {
        let mut store = Store {
            legacy_deep_prompt_job_id: "legacy-job".to_string(),
            ..Default::default()
        };

        assert!(store_prompt_optimization_job_for_owner(
            &mut store,
            "user-a",
            "legacy-job",
            Some("legacy-job"),
        ));
        assert!(store.legacy_deep_prompt_job_id.is_empty());
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-a"),
            PromptOptimizationRecoveryCandidate::Owned("legacy-job".to_string()),
        );
        assert_eq!(
            prompt_optimization_recovery_candidate(&store, "user-b"),
            PromptOptimizationRecoveryCandidate::None,
        );
    }

    #[test]
    fn prompt_diff_preserves_both_prompts_and_marks_added_chinese_phrases() {
        let original = "白色连衣裙，花园";
        let optimized = "精致的白色连衣裙，月光花园";
        let original_tokens = prompt_diff_tokens(original);
        let optimized_tokens = prompt_diff_tokens(optimized);
        let mut matches = Vec::new();
        collect_lcs_matches(&original_tokens, &optimized_tokens, 0, 0, &mut matches);
        let original_pieces = prompt_diff_pieces(&original_tokens, &matches, true);
        let optimized_pieces = prompt_diff_pieces(&optimized_tokens, &matches, false);

        assert_eq!(
            original_pieces
                .iter()
                .map(|piece| piece.text.as_str())
                .collect::<String>(),
            original,
        );
        assert_eq!(
            optimized_pieces
                .iter()
                .map(|piece| piece.text.as_str())
                .collect::<String>(),
            optimized,
        );
        assert!(optimized_pieces.iter().any(|piece| {
            piece.changed && (piece.text.contains("精致") || piece.text.contains("月光"))
        }));
    }

    #[test]
    fn highlighted_markdown_uses_different_colors_for_old_and_new_text() {
        let (original, optimized) = highlighted_prompt_markdown("白天\n1. 室内", "夜晚\n1. 室外");
        assert!(original.contains("#d97706"));
        assert!(optimized.contains("#5147e5"));
        assert!(original.contains("<u>白天"));
        assert!(optimized.contains("<u>夜晚"));
        let _ = styled_markdown(&original);
        let _ = styled_markdown(&optimized);
    }

    #[test]
    fn identical_prompts_have_no_change_highlights() {
        let prompt = "一位古风美女，柔和自然光照，高质量人像摄影效果。";
        let (original, optimized) = highlighted_prompt_markdown(prompt, prompt);
        assert!(!original.contains("<u>"));
        assert!(!optimized.contains("<u>"));
    }

    #[test]
    fn best_result_diff_uses_the_previous_accepted_version() {
        let detail = PromptOptimizationDetail {
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".into(),
            original_prompt: Some("最初提示词".into()),
            result: Some(PromptOptimizationResult {
                chinese_prompt: "第二版提示词".into(),
                english_prompt: "second prompt".into(),
            }),
            best_round_no: Some(3),
            result_round_no: Some(3),
            rounds: vec![
                PromptOptimizationRound {
                    round: 1,
                    accepted: true,
                    chinese_prompt: Some("第一版提示词".into()),
                    ..Default::default()
                },
                PromptOptimizationRound {
                    round: 2,
                    accepted: false,
                    chinese_prompt: Some("被拒绝的提示词".into()),
                    ..Default::default()
                },
                PromptOptimizationRound {
                    round: 3,
                    accepted: true,
                    chinese_prompt: Some("第二版提示词".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        assert_eq!(best_result_comparison_base(&detail), "第一版提示词");
    }

    #[test]
    fn rejected_only_job_does_not_describe_candidate_changes() {
        let detail = PromptOptimizationDetail {
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".into(),
            original_prompt: Some("保留的原提示词".into()),
            result: None,
            best_round_no: None,
            rounds: vec![PromptOptimizationRound {
                round: 1,
                status: "completed".into(),
                accepted: false,
                major_changes: vec!["不应展示的候选改动".into()],
                ..Default::default()
            }],
            ..Default::default()
        };

        assert_eq!(
            optimization_change_summary(&detail),
            "本轮候选未超过原提示词，已保留原提示词。",
        );
    }

    #[test]
    fn low_score_reviewable_candidate_describes_its_actual_changes() {
        let detail = PromptOptimizationDetail {
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".into(),
            original_prompt: Some("一位古风美女".into()),
            result: Some(PromptOptimizationResult {
                chinese_prompt: "一位古风美女，青色织锦长裙，园林晨雾".into(),
                english_prompt: "an ancient-style beauty in a cyan brocade dress".into(),
            }),
            baseline_score: Some(88),
            best_score: Some(88),
            result_score: Some(59),
            result_round_no: Some(3),
            can_apply: true,
            rounds: vec![PromptOptimizationRound {
                round: 3,
                status: "completed".into(),
                accepted: false,
                score_before: 88,
                major_changes: vec!["补充服装材质和环境层次".into()],
                chinese_prompt: Some("一位古风美女，青色织锦长裙，园林晨雾".into()),
                ..Default::default()
            }],
            ..Default::default()
        };

        assert_eq!(best_result_comparison_base(&detail), "一位古风美女");
        assert_eq!(
            optimization_change_summary(&detail),
            "本轮候选 59 分，未替换当前最佳 88 分。\n• 补充服装材质和环境层次",
        );
        assert!(detail.can_apply);
        assert_eq!(detail.result_score, Some(59));
    }

    #[test]
    fn headline_score_keeps_the_server_best_when_a_reviewable_candidate_is_lower() {
        let detail = PromptOptimizationDetail {
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".into(),
            baseline_score: Some(100),
            best_score: Some(100),
            result_score: Some(40),
            result_accepted: false,
            ..Default::default()
        };

        assert_eq!(displayed_best_score(&detail), 100);
    }

    #[test]
    fn baseline_target_summary_does_not_claim_that_a_round_was_needed() {
        let detail = PromptOptimizationDetail {
            billing_account_group_id: "11111111-1111-4111-8111-111111111111".into(),
            completed_rounds: 0,
            stop_reason: Some("target_reached".into()),
            ..Default::default()
        };

        assert_eq!(
            optimization_change_summary(&detail),
            "原提示词已通过高分复核并达到目标，无需额外优化。",
        );
    }
}

#[cfg(test)]
mod core_deep_caller_patch_tests {
    use super::*;
    use std::io::Write;
    use backend_generation::billing_capture_test_support::{listener, read_request, OWNER, PAYER, OTHER};
    struct Fixture {
        authority: Arc<NamespaceStorageAuthority>, scope: BillingScope, backend: Arc<BackendRuntime>, context: AppContext,
        writer: client_state::tests::Fixture, _index_root: tempfile::TempDir,
    }
    const SOURCE: &str = "44444444-4444-4444-8444-444444444444";
    const NEW_JOB: &str = "55555555-5555-4555-8555-555555555555";

    struct JoinedTransport(Option<std::thread::JoinHandle<()>>);
    impl JoinedTransport {
        fn spawn(work: impl FnOnce() + Send + 'static) -> Self { Self(Some(std::thread::spawn(work))) }
        fn join(mut self) { self.0.take().unwrap().join().expect("controlled deep transport panicked"); }
    }
    impl Drop for JoinedTransport {
        fn drop(&mut self) { if let Some(worker) = self.0.take() { let _ = worker.join(); } }
    }
    fn select(f: &mut Fixture, payer: &str) {
        let session = f.scope.request.session.clone();
        let snapshot: AccountSnapshot = serde_json::from_value(serde_json::json!({
            "user":{"id":OWNER,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":payer,"name":"Fixture","group_status":"active","role":"member",
                "member_id":"66666666-6666-4666-8666-666666666666","relationship_status":"active",
                "readable_context":true,"selectable":true,"group_version":"1","membership_version":"1","capabilities":["bill"],"quota":null}
        })).unwrap();
        let ticket = f.context.billing_context.begin_switch(&session, "fixture", payer, PreviousBillingAuthority::StillValid).unwrap();
        let staged = f.context.billing_context.stage_confirmation(&ticket, snapshot.billing_group.clone(), snapshot).unwrap();
        f.context.billing_context.publish_persisted(ticket, staged);
        f.scope = f.context.billing_context.current_scope(KnownCapability::Bill).unwrap();
    }
    fn setup(url: &str) -> (Fixture, AppWindow) {
        i_slint_backend_testing::init_no_event_loop();
        let writer = client_state::tests::Fixture::new(false, false);
        let index_root = tempfile::tempdir().unwrap();
        let session = Arc::new(SessionManager::new(Arc::new(crate::runtime::test_support::MemoryRefreshTokenStore::default())));
        let session_scope = session.install_tokens_for_user(&TokenSet {
            access_token:"fixture-access".into(),access_expires_in_seconds:1800,refresh_token:"fixture-refresh".into(),
            refresh_expires_at:"2099-01-01T00:00:00Z".into(),token_type:"X-Token".into(),
        }, OWNER).unwrap();
        let lease = writer.lease(OWNER, session_scope.auth_epoch, 1);
        writer.activate(lease.clone()).unwrap();
        let root = writer.data_root_capability_arc();
        let authority = Arc::new(NamespaceStorageAuthority::open(root.clone(), &lease).unwrap());
        let backend = Arc::new(BackendRuntime { api: ApiClient::new(ApiClientConfig {
            base_url:reqwest::Url::parse(url).unwrap(),app_version:"999.0.0".into(),timeout:Duration::from_secs(2),
        }, DeviceIdentity { id:OTHER.into(),name:"deep-fixture".into(),platform:"macos".into() }, session).unwrap() });
        let context = AppContext {
            data_root_capability:Some(root),file_index:Some(FileIndex::initialize(index_root.path().join("deep-index.sqlite3")).unwrap()),
            backend:Some(backend.clone()),current_user_id:Arc::new(Mutex::new(Some(OWNER.into()))),
            account_snapshot_scope:Arc::new(Mutex::new(Some(session_scope.clone()))),
            billing_context:Arc::new(BillingContextManager::with_upgrade_latch(backend.api.upgrade_latch().clone())),
            ..Default::default()
        };
        context.user_activity.activate(lease.clone()).unwrap();
        *context.active_namespace.lock().unwrap() = Some(lease.clone());
        backend.api.bind_user_work(UserWorkAdmission::new(context.active_namespace.clone(), context.user_activity.clone())).unwrap();
        let transition = context.namespace_operations.try_begin_transition().unwrap();
        let phase = transition.begin_prepublication_recovery(&lease).unwrap();
        phase.verify_no_unsupported_imports(&authority).unwrap();
        let recovered = phase.finish().unwrap();
        transition.prepare_publication(&lease, recovered).unwrap().publish();
        context.billing_context.bind_authenticated_session(session_scope.clone()).unwrap();
        context.store.borrow_mut().private_persistence = Some(PrivatePersistence::for_test(
            (*writer).clone(), lease.clone(), context.user_activity.clone(), backend.api.upgrade_latch().clone()));
        let scope = BillingScope { request:GroupRequestScope { session:session_scope,account_group_id:PAYER.into() },context_epoch:0 };
        let mut f = Fixture { authority, scope, backend, context, writer, _index_root:index_root };
        select(&mut f, PAYER);
        f.authority = Arc::new(f.context.storage_authority_for(&lease).unwrap());
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_session_state("online".into());
        app.global::<AppState>().set_logged_in(true);
        app.global::<AppState>().set_prompt("durable original prompt".into());
        app.global::<AppState>().set_deep_optimization_run_mode("auto".into());
        app.global::<AppState>().set_deep_optimization_focus_mode("system".into());
        app.global::<AppState>().set_deep_optimization_max_rounds(3);
        wire_prompt_optimization_callbacks(&app, f.context.clone());
        (f, app)
    }
    fn pump(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        while !predicate() && Instant::now() < deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(predicate(), "deep callback completion did not arrive");
    }
    fn accept(listener: &std::net::TcpListener) -> std::net::TcpStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(7);
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => panic!("controlled deep request missing: {error}"),
            }
        }
    }
    fn header(request: &str, name: &str) -> Option<String> {
        request.split("\r\n\r\n").next().unwrap().lines().find_map(|line|
            line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim().to_owned()))
    }
    fn body(request: &str) -> serde_json::Value {
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }
    fn reply(stream: &mut std::net::TcpStream, status: &str, data: serde_json::Value, error: serde_json::Value) {
        let body = serde_json::json!({"request_id":"deep-controlled","data":data,"error":error,"meta":null}).to_string();
        write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    }
    fn detail(id: &str, payer: &str) -> serde_json::Value {
        serde_json::json!({"id":id,"billing_account_group_id":payer,"status":"failed","phase":"failed",
            "max_rounds":3,"current_round":0,"completed_rounds":0,"target_score":90,"progress_percent":0,"can_retry":true})
    }
    fn record(f: &Fixture, key: &str, server: &str, operation: PendingPromptOptimizationOperation) -> PendingPromptOptimizationRecord {
        PendingPromptOptimizationRecord { schema_version:2,client_request_id:key.into(),owner_user_id:OWNER.into(),
            billing_account_group_id:f.scope.request.account_group_id.clone(),auth_epoch:f.scope.request.session.auth_epoch,
            server_job_id:server.into(),operation,presentation_dismissed:false }
    }
    fn source_record(f: &Fixture) -> PendingPromptOptimizationRecord {
        record(f, "source-create-key", SOURCE, PendingPromptOptimizationOperation::Create { request: CreatePromptOptimization {
            client_request_id:"source-create-key".into(),prompt:"original source".into(),run_mode:"auto".into(),focus_mode:"system".into(),max_rounds:3,target_score:90 } })
    }
    fn show_source(f: &Fixture, app: &AppWindow) {
        app.global::<AppState>().set_deep_optimization_job_id(SOURCE.into());
        app.global::<AppState>().set_deep_optimization_stage("failed".into());
        f.context.store.borrow_mut().deep_prompt_jobs_by_owner.insert(OWNER.into(), SOURCE.into());
    }
    pub(super) fn assert_new_create_is_durable() {
        let (listener, url) = listener();
        let (f, app) = setup(&url);
        let authority = f.authority.clone();
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert!(request.starts_with("POST /v1/prompt-optimizations "));
            let records = load_pending_prompt_optimizations_for_namespace(&authority).unwrap();
            assert_eq!(records.len(), 1, "durable record must precede the POST");
            let saved = &records[0];
            assert_eq!(header(&request, "x-account-group-id").as_deref(), Some(PAYER));
            assert_eq!(header(&request, "idempotency-key").as_deref(), Some(saved.client_request_id.as_str()));
            let PendingPromptOptimizationOperation::Create { request: saved_body } = &saved.operation else { panic!("wrong operation"); };
            assert_eq!(body(&request), serde_json::to_value(saved_body).unwrap());
            reply(&mut stream, "200 OK", detail(NEW_JOB, PAYER), serde_json::Value::Null);
        });
        app.global::<AppState>().invoke_start_deep_optimization();
        pump(|| app.global::<AppState>().get_deep_optimization_job_id().as_str() == NEW_JOB);
        transport.join();
        assert_eq!(load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap()[0].server_job_id, NEW_JOB);
        assert_eq!(f.writer.load_client_state_for_namespace(f.authority.lease()).unwrap().unwrap().deep_prompt_jobs_by_owner.get(OWNER).map(String::as_str), Some(NEW_JOB), "visible success requires the real SQLite acknowledgement");
    }
    #[test]
    fn core_deep_terminal_retry_records_new_payer_and_new_job_without_overwriting_source() {
        let (listener, url) = listener();
        let (mut f, app) = setup(&url);
        let original = source_record(&f);
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, original.clone()).unwrap();
        show_source(&f, &app);
        select(&mut f, OTHER);
        let authority = f.authority.clone();
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert!(request.starts_with(&format!("POST /v1/prompt-optimizations/{SOURCE}/retry ")));
            let records = load_pending_prompt_optimizations_for_namespace(&authority).unwrap();
            assert_eq!(records.len(), 2);
            let saved = records.iter().find(|row| row.server_job_id.is_empty()).unwrap();
            assert_eq!(header(&request, "x-account-group-id").as_deref(), Some(OTHER));
            assert_eq!(body(&request), serde_json::json!({"client_request_id":saved.client_request_id}));
            assert_eq!(header(&request, "idempotency-key").as_deref(), Some(saved.client_request_id.as_str()));
            reply(&mut stream, "200 OK", detail(NEW_JOB, OTHER), serde_json::Value::Null);
        });
        app.global::<AppState>().invoke_retry_deep_optimization();
        pump(|| app.global::<AppState>().get_deep_optimization_job_id().as_str() == NEW_JOB);
        transport.join();
        let rows = load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap();
        assert_eq!(serde_json::to_value(rows.iter().find(|row| row.server_job_id == SOURCE).unwrap()).unwrap(), serde_json::to_value(original).unwrap());
        assert_eq!(rows.iter().find(|row| row.server_job_id == NEW_JOB).unwrap().billing_account_group_id, OTHER);
        assert_eq!(f.writer.load_client_state_for_namespace(f.authority.lease()).unwrap().unwrap().deep_prompt_jobs_by_owner.get(OWNER).map(String::as_str), Some(NEW_JOB));
    }
    #[test]
    fn core_deep_saved_retry_replays_original_payer_and_preserves_authority_denial() {
        let (listener, url) = listener();
        let (mut f, app) = setup(&url);
        let saved = record(&f, "exact-retained-retry", "", PendingPromptOptimizationOperation::Retry { source_job_id:SOURCE.into() });
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, saved.clone()).unwrap();
        select(&mut f, OTHER);
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert!(request.starts_with(&format!("POST /v1/prompt-optimizations/{SOURCE}/retry ")));
            assert_eq!(header(&request, "x-account-group-id").as_deref(), Some(PAYER));
            assert_eq!(header(&request, "idempotency-key").as_deref(), Some("exact-retained-retry"));
            assert_eq!(body(&request), serde_json::json!({"client_request_id":"exact-retained-retry"}));
            reply(&mut stream, "403 Forbidden", serde_json::Value::Null, serde_json::json!({"code":"account_group_not_selectable","message":"original admission denied","details":null}));
        });
        app.global::<AppState>().invoke_open_deep_optimization();
        pump(|| !app.global::<AppState>().get_deep_optimization_error().is_empty());
        transport.join();
        assert_eq!(serde_json::to_value(load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap()).unwrap(), serde_json::json!([saved]));
        assert!(app.global::<AppState>().get_deep_optimization_job_id().is_empty());
    }
    #[test]
    fn core_deep_resource_payer_mismatch_never_publishes_or_rewrites_retained_job() {
        let (listener, url) = listener();
        let (f, app) = setup(&url);
        let original = source_record(&f);
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, original.clone()).unwrap();
        show_source(&f, &app);
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert!(request.starts_with(&format!("POST /v1/prompt-optimizations/{SOURCE}/pause ")));
            assert_eq!(header(&request, "x-account-group-id"), None);
            reply(&mut stream, "200 OK", detail(SOURCE, OTHER), serde_json::Value::Null);
        });
        app.global::<AppState>().invoke_pause_deep_optimization();
        pump(|| !app.global::<AppState>().get_deep_optimization_error().is_empty());
        transport.join();
        assert_eq!(serde_json::to_value(load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap()).unwrap(), serde_json::json!([original]));
        assert_eq!(app.global::<AppState>().get_deep_optimization_stage().as_str(), "failed");
    }
    #[test]
    fn core_deep_exact_upgrade_blocks_real_restore_effect_and_keeps_source() {
        let (_listener, url) = listener();
        let (f, app) = setup(&url);
        let saved = source_record(&f);
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, saved.clone()).unwrap();
        show_source(&f, &app);
        app.global::<AppState>().set_prompt("current prompt must survive".into());
        app.global::<AppState>().set_deep_optimization_original_prompt("old private source".into());
        f.backend.api.upgrade_latch().trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        app.global::<AppState>().invoke_restore_deep_original_prompt();
        assert_eq!(app.global::<AppState>().get_prompt().as_str(), "current prompt must survive");
        assert_eq!(app.global::<AppState>().get_deep_optimization_job_id().as_str(), SOURCE);
        assert_eq!(serde_json::to_value(load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap()).unwrap(), serde_json::json!([saved]));
    }

    #[test]
    fn core_deep_accepted_job_survives_local_writer_refusal_without_visible_success() {
        let (listener, url) = listener();
        let (f, app) = setup(&url);
        let (arrived_tx, arrived_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            assert!(read_request(&mut stream).starts_with("POST /v1/prompt-optimizations "));
            arrived_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            reply(&mut stream, "200 OK", detail(NEW_JOB, PAYER), serde_json::Value::Null);
        });
        app.global::<AppState>().invoke_start_deep_optimization();
        arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Exercise the real writer's lease refusal, not a simulated helper return.
        f.writer.deactivate(f.authority.lease()).unwrap();
        release_tx.send(()).unwrap();
        pump(|| !app.global::<AppState>().get_deep_optimization_error().is_empty());
        transport.join();
        assert!(app.global::<AppState>().get_deep_optimization_job_id().is_empty());
        assert_eq!(load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap()[0].server_job_id, NEW_JOB);
    }
    #[test]
    fn core_deep_exact_426_response_preserves_ambiguous_record_and_ui() {
        let (listener, url) = listener();
        let (f, app) = setup(&url);
        let saved = record(&f, "upgrade-retained-retry", "", PendingPromptOptimizationOperation::Retry { source_job_id:SOURCE.into() });
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, saved.clone()).unwrap();
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert_eq!(header(&request, "idempotency-key").as_deref(), Some("upgrade-retained-retry"));
            reply(&mut stream, "426 Upgrade Required", serde_json::Value::Null,
                serde_json::json!({"code":"client_upgrade_required","message":"upgrade","details":{"minimum_version":"99.0.0"}}));
        });
        app.global::<AppState>().invoke_open_deep_optimization();
        pump(|| f.backend.api.upgrade_latch().is_tripped());
        transport.join();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(app.global::<AppState>().get_prompt().as_str(), "durable original prompt");
        assert!(app.global::<AppState>().get_deep_optimization_job_id().is_empty());
        assert_eq!(serde_json::to_value(load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap()).unwrap(), serde_json::json!([saved]));
    }
    #[test]
    fn core_deep_old_user_completion_cannot_replace_new_user_prompt() {
        let (listener, url) = listener();
        let (f, app) = setup(&url);
        let saved = source_record(&f);
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, saved.clone()).unwrap();
        show_source(&f, &app);
        let (arrived_tx, arrived_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert_eq!(header(&request, "x-account-group-id"), None);
            arrived_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let mut data = detail(SOURCE, PAYER);
            data["status"] = "completed".into();
            data["final_result"] = serde_json::json!({"chinese_prompt":"A private result","english_prompt":"A private English"});
            reply(&mut stream, "200 OK", data, serde_json::Value::Null);
        });
        app.global::<AppState>().invoke_pause_deep_optimization();
        arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        f.backend.api.session().install_tokens_for_user(&TokenSet {
            access_token:"new-user-access".into(),access_expires_in_seconds:1800,refresh_token:"new-user-refresh".into(),
            refresh_expires_at:"2099-01-01T00:00:00Z".into(),token_type:"X-Token".into(),
        }, OTHER).unwrap();
        *f.context.current_user_id.lock().unwrap() = Some(OTHER.into());
        app.global::<AppState>().set_prompt("B private prompt".into());
        app.global::<AppState>().set_deep_optimization_job_id("".into());
        release_tx.send(()).unwrap();
        transport.join();
        // Drain the actual bounded callback unit; no new-user publication is authorized by A's lease.
        f.context.user_activity.begin_quiesce(f.authority.lease()).unwrap().retire();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(app.global::<AppState>().get_prompt().as_str(), "B private prompt");
        assert!(app.global::<AppState>().get_deep_optimization_job_id().is_empty());
        assert_eq!(serde_json::to_value(load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap()).unwrap(), serde_json::json!([saved]));
    }

    #[test]
    fn core_deep_dismissed_retry_does_not_resurrect_its_preserved_source() {
        let (listener, url) = listener();
        let (f, app) = setup(&url);
        let source = source_record(&f);
        let child = record(&f, "retry-child-key", NEW_JOB, PendingPromptOptimizationOperation::Retry { source_job_id:SOURCE.into() });
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, source.clone()).unwrap();
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, child).unwrap();
        app.global::<AppState>().set_deep_optimization_job_id(NEW_JOB.into());
        app.global::<AppState>().set_deep_optimization_stage("complete".into());
        f.context.store.borrow_mut().deep_prompt_jobs_by_owner.insert(OWNER.into(), NEW_JOB.into());
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert!(request.starts_with(&format!("GET /v1/prompt-optimizations/{NEW_JOB} ")));
            assert_eq!(header(&request, "x-account-group-id"), None);
            let mut data = detail(NEW_JOB, PAYER); data["status"] = "completed".into();
            reply(&mut stream, "200 OK", data, serde_json::Value::Null);
        });
        app.global::<AppState>().invoke_close_deep_optimization();
        pump(|| app.global::<AppState>().get_deep_optimization_job_id().is_empty());
        transport.join();
        let rows = load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap();
        assert_eq!(serde_json::to_value(rows.iter().find(|row| row.server_job_id == SOURCE).unwrap()).unwrap(), serde_json::to_value(source).unwrap());
        assert!(rows.iter().find(|row| row.server_job_id == NEW_JOB).unwrap().presentation_dismissed);
        assert!(!f.writer.load_client_state_for_namespace(f.authority.lease()).unwrap().unwrap().deep_prompt_jobs_by_owner.contains_key(OWNER));
        app.global::<AppState>().invoke_open_deep_optimization();
        pump(|| app.global::<AppState>().get_deep_optimization_stage().as_str() == "settings");
        assert!(app.global::<AppState>().get_deep_optimization_job_id().is_empty());
    }
    #[test]
    fn core_deep_dismiss_payer_mismatch_keeps_visible_job_and_immutable_source() {
        let (listener, url) = listener();
        let (f, app) = setup(&url);
        let source = source_record(&f);
        upsert_pending_prompt_optimization_for_namespace(&f.authority, &f.scope, source.clone()).unwrap();
        show_source(&f, &app);
        let transport = JoinedTransport::spawn(move || {
            let mut stream = accept(&listener);
            let request = read_request(&mut stream);
            assert!(request.starts_with(&format!("GET /v1/prompt-optimizations/{SOURCE} ")));
            assert_eq!(header(&request, "x-account-group-id"), None);
            reply(&mut stream, "200 OK", detail(SOURCE, OTHER), serde_json::Value::Null);
        });
        app.global::<AppState>().invoke_close_deep_optimization();
        pump(|| !app.global::<AppState>().get_deep_optimization_error().is_empty());
        transport.join();
        assert_eq!(app.global::<AppState>().get_deep_optimization_job_id().as_str(), SOURCE);
        assert_eq!(serde_json::to_value(load_pending_prompt_optimizations_for_namespace(&f.authority).unwrap()).unwrap(), serde_json::json!([source]));
    }
}
