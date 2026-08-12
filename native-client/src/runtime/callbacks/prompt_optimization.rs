use super::*;

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

#[derive(Clone, Debug, PartialEq, Eq)]
enum PromptOptimizationRecoveryCandidate {
    Pending(CreatePromptOptimization),
    Owned(String),
    LegacyUnverified(String),
    None,
}

struct PromptOptimizationRecovery {
    detail: Option<PromptOptimizationDetail>,
    stale_owned_job_id: Option<String>,
    verified_legacy_job_id: Option<String>,
    legacy_preserved: bool,
}

const LEGACY_PROMPT_RECOVERY_PRESERVED_MESSAGE: &str =
    "检测到旧版未归属的深度优化恢复记录，已安全保留，未绑定到当前账号";

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
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_deep_optimization_open(false);
            if matches!(
                state.get_deep_optimization_stage().as_str(),
                "complete" | "cancelled"
            ) {
                clear_prompt_optimization_job(&app, &context);
            }
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
            state.set_deep_optimization_stage("running".into());
            state.set_deep_optimization_error("".into());
            state.set_deep_optimization_status_message("正在提交下一轮优化...".into());
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
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            clear_prompt_optimization_job(&app, &context);
            let state = app.global::<AppState>();
            state.set_deep_optimization_stage("settings".into());
            state.set_deep_optimization_original_prompt(state.get_prompt());
            state.set_deep_optimization_error("".into());
            state.set_deep_optimization_feedback("".into());
            state.set_deep_optimization_feedback_stable(false);
            state.set_deep_optimization_stable_feedback_summary("".into());
            state.set_deep_optimization_result_tab("chinese".into());
            state.set_deep_optimization_progress(0);
            state.set_deep_optimization_maximum_credits(
                (state.get_deep_optimization_max_rounds() * 5)
                    .to_string()
                    .into(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_invalidate_deep_prompt_binding(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let category = current_workspace_category(&app);
            if context
                .store
                .borrow_mut()
                .deep_prompt_bindings
                .remove(&category)
                .is_some()
            {
                save_local_store(&app, &context.store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_restore_deep_original_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let original = state.get_deep_optimization_original_prompt().to_string();
            if original.trim().is_empty() {
                return;
            }
            state.set_prompt(original.into());
            state.set_deep_optimization_applied_chinese("".into());
            state.set_deep_optimization_applied_english("".into());
            context
                .store
                .borrow_mut()
                .deep_prompt_bindings
                .remove(&current_workspace_category(&app));
            store_current_prompt_draft(&app, &context.store, &current_workspace_category(&app));
            save_local_store(&app, &context.store.borrow());
            clear_prompt_optimization_job(&app, &context);
            state.set_generation_status("已恢复深度优化前的提示词".into());
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
                _ => api.retry_scoped(&id, &scope),
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
    if !require_online_operation(app, "深度优化") {
        return;
    }
    let Some(session_scope) = current_prompt_optimization_session_scope(&context) else {
        clear_prompt_optimization_account_state(app, &context);
        return;
    };
    let state = app.global::<AppState>();
    state.set_deep_optimization_open(true);
    state.set_deep_optimization_error("".into());
    let current_job_id = state.get_deep_optimization_job_id().to_string();
    if !current_job_id.is_empty() {
        recover_prompt_optimization_scoped(app, context, session_scope);
        return;
    }
    let candidate = prompt_optimization_recovery_candidate(
        &context.store.borrow(),
        &session_scope.owner_user_id,
    );
    match candidate {
        PromptOptimizationRecoveryCandidate::Pending(_) => {
            state.set_deep_optimization_status_message(
                "正在按原请求恢复尚未确认的深度优化任务...".into(),
            );
            recover_prompt_optimization_scoped(app, context, session_scope);
            return;
        }
        PromptOptimizationRecoveryCandidate::Owned(job_id) => {
            state.set_deep_optimization_job_id(job_id.into());
            recover_prompt_optimization_scoped(app, context, session_scope);
            return;
        }
        PromptOptimizationRecoveryCandidate::LegacyUnverified(_) => {
            state.set_deep_optimization_status_message(
                "正在通过当前账号安全核验旧版深度优化恢复记录...".into(),
            );
            recover_prompt_optimization_scoped(app, context, session_scope);
            return;
        }
        PromptOptimizationRecoveryCandidate::None => {}
    }
    state.set_deep_optimization_stage("settings".into());
    state.set_deep_optimization_original_prompt(state.get_prompt());
    state.set_deep_optimization_maximum_credits(
        (state.get_deep_optimization_max_rounds() * 5)
            .to_string()
            .into(),
    );
}

fn start_prompt_optimization(app: &AppWindow, context: AppContext) {
    if !require_online_operation(app, "深度优化") {
        return;
    }
    let Some(session_scope) = current_prompt_optimization_session_scope(&context) else {
        clear_prompt_optimization_account_state(app, &context);
        app.global::<AppState>()
            .set_deep_optimization_error("登录状态已失效，请重新登录".into());
        return;
    };
    let state = app.global::<AppState>();
    if prompt_optimization_recovery_candidate(
        &context.store.borrow(),
        &session_scope.owner_user_id,
    ) != PromptOptimizationRecoveryCandidate::None
    {
        state.set_deep_optimization_error(
            "检测到尚未确认的深度优化任务，正在恢复；恢复完成前不能重复创建".into(),
        );
        recover_prompt_optimization_scoped(app, context, session_scope);
        return;
    }
    let prompt = state.get_prompt().trim().to_string();
    if prompt.is_empty() {
        state.set_deep_optimization_error("请先输入需要优化的提示词".into());
        return;
    }
    let max_rounds = state.get_deep_optimization_max_rounds().clamp(2, 4);
    let request = CreatePromptOptimization {
        client_request_id: Uuid::new_v4().simple().to_string(),
        prompt: prompt.clone(),
        run_mode: state.get_deep_optimization_run_mode().to_string(),
        focus_mode: state.get_deep_optimization_focus_mode().to_string(),
        max_rounds,
        target_score: 90,
    };
    if let Err(error) =
        persist_pending_prompt_optimization_request(app, &context, &session_scope, &request)
    {
        state.set_deep_optimization_stage("settings".into());
        state.set_deep_optimization_error(
            format!(
                "无法保存深度优化恢复信息，任务尚未提交，请检查磁盘后重试：{error}"
            )
            .into(),
        );
        return;
    }
    clear_prompt_optimization_result(&state);
    state.set_deep_optimization_original_prompt(prompt.into());
    state.set_deep_optimization_stage("running".into());
    state.set_deep_optimization_progress(1);
    state.set_deep_optimization_error("".into());
    state.set_deep_optimization_phase_label("正在创建深度优化任务".into());
    state.set_deep_optimization_status_message("正在预留积分并进入任务队列...".into());
    state.set_deep_optimization_maximum_credits((max_rounds * 5).to_string().into());
    run_prompt_request_scoped(
        app,
        context,
        session_scope,
        PromptRequestEffect::Start,
        None,
        move |api, scope| api.create_scoped(&request, &scope),
    );
}

fn run_prompt_request_scoped<F>(
    app: &AppWindow,
    context: AppContext,
    session_scope: SessionScope,
    effect: PromptRequestEffect,
    expected_job_id: Option<String>,
    request: F,
) where
    F: FnOnce(PromptOptimizationApi, SessionScope)
            -> std::result::Result<PromptOptimizationDetail, ApiError>
        + Send
        + 'static,
{
    let Some(backend) = context.backend.clone() else {
        if prompt_optimization_scope_disposition(&context, &session_scope)
            == PromptOptimizationScopeDisposition::Current
        {
            app.global::<AppState>()
                .set_deep_optimization_error("服务端尚未初始化，请重启客户端后重试".into());
        }
        return;
    };
    let (sender, receiver) = mpsc::channel();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let result = request(PromptOptimizationApi::new(backend.api.clone()), worker_scope);
        let _ = sender.send(result);
    });
    poll_prompt_request(
        app.as_weak(),
        context,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
        effect,
        expected_job_id,
    );
}

fn poll_prompt_request(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<
        RefCell<
            Option<mpsc::Receiver<std::result::Result<PromptOptimizationDetail, ApiError>>>,
        >,
    >,
    effect: PromptRequestEffect,
    expected_job_id: Option<String>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let Some(app) = app_weak.upgrade() else {
            receiver.borrow_mut().take();
            return;
        };
        match prompt_optimization_scope_disposition(&context, &session_scope) {
            PromptOptimizationScopeDisposition::Current => {}
            PromptOptimizationScopeDisposition::CapturedTerminal => {
                receiver.borrow_mut().take();
                handle_prompt_optimization_terminal(&app, &context, &session_scope);
                return;
            }
            PromptOptimizationScopeDisposition::Stale => {
                receiver.borrow_mut().take();
                return;
            }
        }
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(receiver) = slot.as_ref() else {
                return;
            };
            match receiver.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(ApiError::Network {
                        message: "深度优化请求已中断，请重试".to_string(),
                        timeout: false,
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_prompt_request(
                app_weak,
                context,
                session_scope,
                receiver,
                effect,
                expected_job_id,
            );
            return;
        };
        match result {
            Ok(detail) => {
                if !prompt_optimization_detail_matches_request(
                    app.global::<AppState>()
                        .get_deep_optimization_job_id()
                        .as_str(),
                    expected_job_id.as_deref(),
                    &detail.id,
                    effect,
                ) {
                    return;
                }
                let id = detail.id.clone();
                let status = detail.status.clone();
                let applied = if matches!(effect, PromptRequestEffect::ApplyResult)
                    || status == "completed"
                {
                    detail
                        .final_result
                        .clone()
                        .or_else(|| detail.result.clone())
                } else {
                    None
                };
                if let Err(error) = persist_prompt_optimization_job(
                    &app,
                    &context,
                    &session_scope,
                    &id,
                    None,
                ) {
                    let message = format!(
                        "服务端任务已创建，但本地恢复信息保存失败；原请求记录仍保留，请勿重复提交：{error}"
                    );
                    let state = app.global::<AppState>();
                    state.set_deep_optimization_error(message.clone().into());
                    state.set_deep_optimization_status_message(message.into());
                    return;
                }
                apply_prompt_optimization_detail(&app, &detail);
                if let Some(result) = applied {
                    apply_prompt_versions(&app, &context, result);
                    refresh_backend_snapshot(&app, context.clone());
                } else if matches!(effect, PromptRequestEffect::Start) || status == "cancelled" {
                    refresh_backend_snapshot(&app, context.clone());
                }
                if matches!(status.as_str(), "queued" | "processing") {
                    begin_prompt_optimization_polling(&app, context, session_scope, id);
                }
            }
            Err(error) => {
                let message = error.user_message();
                let state = app.global::<AppState>();
                state.set_deep_optimization_error(message.clone().into());
                if matches!(effect, PromptRequestEffect::Start) {
                    state.set_deep_optimization_stage("settings".into());
                } else if state.get_session_state().as_str() == "online" {
                    let id = state.get_deep_optimization_job_id().to_string();
                    if !id.is_empty() {
                        let retry_app = app.as_weak();
                        let retry_context = context.clone();
                        let retry_scope = session_scope.clone();
                        slint::Timer::single_shot(Duration::from_secs(2), move || {
                            if let Some(app) = retry_app.upgrade() {
                                if prompt_optimization_scope_disposition(
                                    &retry_context,
                                    &retry_scope,
                                ) != PromptOptimizationScopeDisposition::Current
                                {
                                    return;
                                }
                                run_prompt_request_scoped(
                                    &app,
                                    retry_context,
                                    retry_scope,
                                    PromptRequestEffect::Refresh,
                                    Some(id.clone()),
                                    move |api, scope| api.get_scoped(&id, &scope),
                                );
                            }
                        });
                    }
                }
                state.set_deep_optimization_status_message(message.into());
            }
        }
    });
}

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

fn persist_pending_prompt_optimization_request(
    app: &AppWindow,
    context: &AppContext,
    session_scope: &SessionScope,
    request: &CreatePromptOptimization,
) -> Result<()> {
    if session_scope.owner_user_id.trim().is_empty()
        || request.client_request_id.trim().is_empty()
        || request.prompt.trim().is_empty()
    {
        return Err(anyhow!("deep prompt optimization recovery request is invalid"));
    }
    let owner_user_id = session_scope.owner_user_id.clone();
    let previous = {
        let mut store = context.store.borrow_mut();
        store
            .deep_prompt_pending_requests_by_owner
            .insert(owner_user_id.clone(), request.clone())
    };
    if let Err(error) = save_local_store_checked(app, &context.store.borrow()) {
        let mut store = context.store.borrow_mut();
        match previous {
            Some(previous) => {
                store
                    .deep_prompt_pending_requests_by_owner
                    .insert(owner_user_id, previous);
            }
            None => {
                store
                    .deep_prompt_pending_requests_by_owner
                    .remove(&owner_user_id);
            }
        }
        return Err(error);
    }
    Ok(())
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

fn persist_prompt_optimization_job(
    app: &AppWindow,
    context: &AppContext,
    session_scope: &SessionScope,
    id: &str,
    verified_legacy_job_id: Option<&str>,
) -> Result<()> {
    let owner_user_id = session_scope.owner_user_id.trim();
    let id = id.trim();
    if owner_user_id.is_empty() || id.is_empty() {
        return Err(anyhow!("deep prompt optimization job ownership is invalid"));
    }
    let (previous_job_id, previous_pending, previous_legacy) = {
        let mut store = context.store.borrow_mut();
        let previous_job_id = store.deep_prompt_jobs_by_owner.get(owner_user_id).cloned();
        let previous_pending = store
            .deep_prompt_pending_requests_by_owner
            .remove(owner_user_id);
        let previous_legacy = store.legacy_deep_prompt_job_id.clone();
        if !store_prompt_optimization_job_for_owner(
            &mut store,
            owner_user_id,
            id,
            verified_legacy_job_id,
        ) {
            if let Some(previous_pending) = previous_pending {
                store
                    .deep_prompt_pending_requests_by_owner
                    .insert(owner_user_id.to_string(), previous_pending);
            }
            return Err(anyhow!("deep prompt optimization job could not be persisted"));
        }
        (previous_job_id, previous_pending, previous_legacy)
    };
    if let Err(error) = save_local_store_checked(app, &context.store.borrow()) {
        let mut store = context.store.borrow_mut();
        match previous_job_id {
            Some(previous_job_id) => {
                store
                    .deep_prompt_jobs_by_owner
                    .insert(owner_user_id.to_string(), previous_job_id);
            }
            None => {
                store.deep_prompt_jobs_by_owner.remove(owner_user_id);
            }
        }
        if let Some(previous_pending) = previous_pending {
            store
                .deep_prompt_pending_requests_by_owner
                .insert(owner_user_id.to_string(), previous_pending);
        }
        store.legacy_deep_prompt_job_id = previous_legacy;
        return Err(error);
    }
    app.global::<AppState>()
        .set_deep_optimization_job_id(id.into());
    Ok(())
}

fn clear_prompt_optimization_job(app: &AppWindow, context: &AppContext) {
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
        save_local_store(app, &context.store.borrow());
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
    state.set_deep_optimization_maximum_credits("15".into());
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

fn begin_prompt_optimization_polling(
    app: &AppWindow,
    context: AppContext,
    session_scope: SessionScope,
    id: String,
) {
    match prompt_optimization_scope_disposition(&context, &session_scope) {
        PromptOptimizationScopeDisposition::Current => {}
        PromptOptimizationScopeDisposition::CapturedTerminal => {
            handle_prompt_optimization_terminal(app, &context, &session_scope);
            return;
        }
        PromptOptimizationScopeDisposition::Stale => return,
    }
    let polling_key = prompt_optimization_poll_key(&session_scope, &id);
    if context.prompt_optimization_polling.borrow().as_deref() == Some(polling_key.as_str()) {
        return;
    }
    *context.prompt_optimization_polling.borrow_mut() = Some(polling_key);
    poll_prompt_optimization_once(app.as_weak(), context, session_scope, id);
}

fn poll_prompt_optimization_once(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    id: String,
) {
    match prompt_optimization_scope_disposition(&context, &session_scope) {
        PromptOptimizationScopeDisposition::Current => {}
        PromptOptimizationScopeDisposition::CapturedTerminal => {
            if let Some(app) = app_weak.upgrade() {
                handle_prompt_optimization_terminal(&app, &context, &session_scope);
            }
            return;
        }
        PromptOptimizationScopeDisposition::Stale => return,
    }
    let Some(backend) = context.backend.clone() else {
        clear_prompt_optimization_polling_if_matches(
            &context,
            &prompt_optimization_poll_key(&session_scope, &id),
        );
        return;
    };
    let (sender, receiver) = mpsc::channel();
    let request_id = id.clone();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let result = PromptOptimizationApi::new(backend.api.clone())
            .get_scoped(&request_id, &worker_scope);
        let _ = sender.send(result);
    });
    poll_prompt_optimization_once_result(
        app_weak,
        context,
        session_scope,
        id,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_prompt_optimization_once_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    id: String,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<PromptOptimizationDetail, ApiError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let polling_key = prompt_optimization_poll_key(&session_scope, &id);
        let Some(app) = app_weak.upgrade() else {
            receiver.borrow_mut().take();
            clear_prompt_optimization_polling_if_matches(&context, &polling_key);
            return;
        };
        match prompt_optimization_scope_disposition(&context, &session_scope) {
            PromptOptimizationScopeDisposition::Current => {}
            PromptOptimizationScopeDisposition::CapturedTerminal => {
                receiver.borrow_mut().take();
                clear_prompt_optimization_polling_if_matches(&context, &polling_key);
                handle_prompt_optimization_terminal(&app, &context, &session_scope);
                return;
            }
            PromptOptimizationScopeDisposition::Stale => {
                receiver.borrow_mut().take();
                clear_prompt_optimization_polling_if_matches(&context, &polling_key);
                return;
            }
        }
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(receiver) = slot.as_ref() else {
                return;
            };
            match receiver.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err(ApiError::Network {
                        message: "深度优化状态请求已中断，请稍后重试".to_string(),
                        timeout: false,
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_prompt_optimization_once_result(
                app_weak,
                context,
                session_scope,
                id,
                receiver,
            );
            return;
        };
        match result {
            Ok(detail) => {
                if detail.id != id
                    || app
                        .global::<AppState>()
                        .get_deep_optimization_job_id()
                        .as_str()
                        != id
                {
                    clear_prompt_optimization_polling_if_matches(&context, &polling_key);
                    return;
                }
                let completed_result = if detail.status == "completed" {
                    detail.final_result.clone()
                } else {
                    None
                };
                apply_prompt_optimization_detail(&app, &detail);
                if let Some(result) = completed_result {
                    apply_prompt_versions(&app, &context, result);
                }
                if matches!(detail.status.as_str(), "queued" | "processing") {
                    let next_app = app.as_weak();
                    let next_context = context.clone();
                    let next_scope = session_scope.clone();
                    let next_id = id.clone();
                    slint::Timer::single_shot(Duration::from_secs(2), move || {
                        poll_prompt_optimization_once(
                            next_app,
                            next_context,
                            next_scope,
                            next_id,
                        );
                    });
                } else {
                    clear_prompt_optimization_polling_if_matches(&context, &polling_key);
                    let state = app.global::<AppState>();
                    if detail.status == "manual_review" {
                        state.set_generation_status(
                            "深度优化已生成新版本，请点击“查看深度优化”确认".into(),
                        );
                    } else if matches!(detail.status.as_str(), "completed" | "cancelled") {
                        refresh_backend_snapshot(&app, context.clone());
                    }
                }
            }
            Err(error) => {
                clear_prompt_optimization_polling_if_matches(&context, &polling_key);
                app.global::<AppState>()
                    .set_deep_optimization_status_message(
                        format!("状态同步暂时失败：{}", error.user_message()).into(),
                    );
                let retry_app = app.as_weak();
                let retry_context = context.clone();
                let retry_scope = session_scope.clone();
                slint::Timer::single_shot(Duration::from_secs(5), move || {
                    if let Some(app) = retry_app.upgrade() {
                        begin_prompt_optimization_polling(
                            &app,
                            retry_context,
                            retry_scope,
                            id,
                        );
                    }
                });
            }
        }
    });
}

pub(super) fn recover_prompt_optimization(app: &AppWindow, context: AppContext) {
    let Some(session_scope) = current_prompt_optimization_session_scope(&context) else {
        clear_prompt_optimization_account_state(app, &context);
        return;
    };
    recover_prompt_optimization_scoped(app, context, session_scope);
}

fn recover_prompt_optimization_scoped(
    app: &AppWindow,
    context: AppContext,
    session_scope: SessionScope,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let candidate = prompt_optimization_recovery_candidate(
        &context.store.borrow(),
        &session_scope.owner_user_id,
    );
    let (sender, receiver) = mpsc::channel();
    let worker_scope = session_scope.clone();
    let worker_candidate = candidate.clone();
    std::thread::spawn(move || {
        let api = PromptOptimizationApi::new(backend.api.clone());
        let _ = sender.send(resolve_prompt_optimization_recovery(
            &api,
            &worker_scope,
            worker_candidate,
        ));
    });
    poll_prompt_recovery(
        app.as_weak(),
        context,
        session_scope,
        candidate,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn should_fallback_to_active_prompt_optimization(error: &ApiError) -> bool {
    error.code() == Some("prompt_optimization_not_found")
}

fn get_prompt_optimization_detail_scoped(
    api: &PromptOptimizationApi,
    id: &str,
    session_scope: &SessionScope,
) -> std::result::Result<PromptOptimizationDetail, ApiError> {
    let detail = api.get_scoped(id, session_scope)?;
    if detail.id != id {
        return Err(ApiError::Protocol {
            message: "深度优化任务编号与请求不一致".to_string(),
            request_id: None,
        });
    }
    Ok(detail)
}

fn active_prompt_optimization_detail(
    api: &PromptOptimizationApi,
    session_scope: &SessionScope,
) -> std::result::Result<Option<PromptOptimizationDetail>, ApiError> {
    let items = api.active_scoped(session_scope)?;
    let Some(item) = items.first() else {
        return Ok(None);
    };
    let id = item.id.trim();
    if id.is_empty() {
        return Err(ApiError::Protocol {
            message: "服务端返回了无效的深度优化任务编号".to_string(),
            request_id: None,
        });
    }
    get_prompt_optimization_detail_scoped(api, id, session_scope).map(Some)
}

fn resolve_prompt_optimization_recovery(
    api: &PromptOptimizationApi,
    session_scope: &SessionScope,
    candidate: PromptOptimizationRecoveryCandidate,
) -> std::result::Result<PromptOptimizationRecovery, ApiError> {
    match candidate {
        PromptOptimizationRecoveryCandidate::Pending(request) => {
            let detail = api.create_scoped(&request, session_scope)?;
            Ok(PromptOptimizationRecovery {
                detail: Some(detail),
                stale_owned_job_id: None,
                verified_legacy_job_id: None,
                legacy_preserved: false,
            })
        }
        PromptOptimizationRecoveryCandidate::Owned(stored_id) => {
            match get_prompt_optimization_detail_scoped(api, &stored_id, session_scope) {
                Ok(detail) => Ok(PromptOptimizationRecovery {
                    detail: Some(detail),
                    stale_owned_job_id: None,
                    verified_legacy_job_id: None,
                    legacy_preserved: false,
                }),
                Err(error) if should_fallback_to_active_prompt_optimization(&error) => {
                    Ok(PromptOptimizationRecovery {
                        detail: active_prompt_optimization_detail(api, session_scope)?,
                        stale_owned_job_id: Some(stored_id),
                        verified_legacy_job_id: None,
                        legacy_preserved: false,
                    })
                }
                Err(error) => Err(error),
            }
        }
        PromptOptimizationRecoveryCandidate::LegacyUnverified(legacy_id) => {
            match get_prompt_optimization_detail_scoped(api, &legacy_id, session_scope) {
                Ok(detail) => Ok(PromptOptimizationRecovery {
                    detail: Some(detail),
                    stale_owned_job_id: None,
                    verified_legacy_job_id: Some(legacy_id),
                    legacy_preserved: false,
                }),
                Err(error) if should_fallback_to_active_prompt_optimization(&error) => {
                    Ok(PromptOptimizationRecovery {
                        detail: active_prompt_optimization_detail(api, session_scope)?,
                        stale_owned_job_id: None,
                        verified_legacy_job_id: None,
                        legacy_preserved: true,
                    })
                }
                Err(error) => Err(error),
            }
        }
        PromptOptimizationRecoveryCandidate::None => Ok(PromptOptimizationRecovery {
            detail: active_prompt_optimization_detail(api, session_scope)?,
            stale_owned_job_id: None,
            verified_legacy_job_id: None,
            legacy_preserved: false,
        }),
    }
}

fn poll_prompt_recovery(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    candidate: PromptOptimizationRecoveryCandidate,
    receiver: Rc<
        RefCell<
            Option<mpsc::Receiver<std::result::Result<PromptOptimizationRecovery, ApiError>>>,
        >,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let Some(app) = app_weak.upgrade() else {
            receiver.borrow_mut().take();
            return;
        };
        match prompt_optimization_scope_disposition(&context, &session_scope) {
            PromptOptimizationScopeDisposition::Current => {}
            PromptOptimizationScopeDisposition::CapturedTerminal => {
                receiver.borrow_mut().take();
                handle_prompt_optimization_terminal(&app, &context, &session_scope);
                return;
            }
            PromptOptimizationScopeDisposition::Stale => {
                receiver.borrow_mut().take();
                return;
            }
        }
        let result = {
            let mut slot = receiver.borrow_mut();
            let receiver = slot.as_ref();
            match receiver.map(mpsc::Receiver::try_recv) {
                Some(Ok(result)) => {
                    slot.take();
                    Some(result)
                }
                Some(Err(TryRecvError::Empty)) => None,
                Some(Err(TryRecvError::Disconnected)) | None => {
                    slot.take();
                    Some(Err(ApiError::Network {
                        message: "深度优化恢复请求已中断，请稍后重试".to_string(),
                        timeout: false,
                    }))
                }
            }
        };
        let Some(result) = result else {
            poll_prompt_recovery(app_weak, context, session_scope, candidate, receiver);
            return;
        };
        if prompt_optimization_recovery_candidate(
            &context.store.borrow(),
            &session_scope.owner_user_id,
        ) != candidate
        {
            return;
        }
        match result {
            Ok(recovery) => {
                let PromptOptimizationRecovery {
                    detail,
                    stale_owned_job_id,
                    verified_legacy_job_id,
                    legacy_preserved,
                } = recovery;
                match detail {
                    Some(detail) if !detail.id.trim().is_empty() => {
                        let id = detail.id.clone();
                        let active = matches!(detail.status.as_str(), "queued" | "processing");
                        let completed_result = if detail.status == "completed" {
                            detail.final_result.clone()
                        } else {
                            None
                        };
                        if let Err(error) = persist_prompt_optimization_job(
                            &app,
                            &context,
                            &session_scope,
                            &id,
                            verified_legacy_job_id.as_deref(),
                        ) {
                            app.global::<AppState>().set_deep_optimization_status_message(
                                format!(
                                    "深度优化任务已找到，但本地恢复信息保存失败；原记录仍保留：{error}"
                                )
                                .into(),
                            );
                            return;
                        }
                        apply_prompt_optimization_detail(&app, &detail);
                        if let Some(result) = completed_result {
                            apply_prompt_versions(&app, &context, result);
                        }
                        if legacy_preserved {
                            app.global::<AppState>().set_generation_status(
                                LEGACY_PROMPT_RECOVERY_PRESERVED_MESSAGE.into(),
                            );
                        }
                        if active {
                            begin_prompt_optimization_polling(
                                &app,
                                context,
                                session_scope,
                                id,
                            );
                        }
                    }
                    Some(_) => {
                        app.global::<AppState>()
                            .set_deep_optimization_status_message(
                                "深度优化任务状态无效，请稍后重试".into(),
                            );
                    }
                    None => {
                        if let Some(stale_owned_job_id) = stale_owned_job_id {
                            let removed = remove_prompt_optimization_job_for_owner_if_matches(
                                &mut context.store.borrow_mut(),
                                &session_scope.owner_user_id,
                                Some(&stale_owned_job_id),
                            );
                            if removed {
                                save_local_store(&app, &context.store.borrow());
                            }
                        }
                        let state = app.global::<AppState>();
                        state.set_deep_optimization_job_id("".into());
                        state.set_deep_optimization_stage("settings".into());
                        state.set_deep_optimization_progress(0);
                        state.set_deep_optimization_can_pause(false);
                        state.set_deep_optimization_can_resume(false);
                        state.set_deep_optimization_can_retry(false);
                        state.set_deep_optimization_can_cancel(false);
                        state.set_deep_optimization_can_continue(false);
                        state.set_deep_optimization_can_apply(false);
                        state.set_deep_optimization_can_clear_stable_feedback(false);
                        clear_prompt_optimization_result(&state);
                        if legacy_preserved {
                            state.set_deep_optimization_status_message(
                                LEGACY_PROMPT_RECOVERY_PRESERVED_MESSAGE.into(),
                            );
                        }
                    }
                }
            }
            Err(error) => {
                let state = app.global::<AppState>();
                let legacy_notice = matches!(
                    candidate,
                    PromptOptimizationRecoveryCandidate::LegacyUnverified(_)
                )
                .then_some(format!("；{LEGACY_PROMPT_RECOVERY_PRESERVED_MESSAGE}"))
                .unwrap_or_default();
                state.set_deep_optimization_status_message(
                    format!(
                        "深度优化任务恢复失败：{}{legacy_notice}",
                        error.user_message(),
                    )
                    .into(),
                );
            }
        }
    });
}

fn apply_prompt_versions(app: &AppWindow, context: &AppContext, result: PromptOptimizationResult) {
    if result.chinese_prompt.trim().is_empty() || result.english_prompt.trim().is_empty() {
        app.global::<AppState>()
            .set_deep_optimization_error("服务端返回的中英文提示词不完整".into());
        return;
    }
    let state = app.global::<AppState>();
    state.set_prompt(result.chinese_prompt.clone().into());
    state.set_deep_optimization_applied_chinese(result.chinese_prompt.clone().into());
    state.set_deep_optimization_applied_english(result.english_prompt.clone().into());
    context.store.borrow_mut().deep_prompt_bindings.insert(
        current_workspace_category(app),
        DeepPromptBinding {
            chinese: result.chinese_prompt,
            english: result.english_prompt,
        },
    );
    store_current_prompt_draft(app, &context.store, &current_workspace_category(app));
    save_local_store(app, &context.store.borrow());
    state.set_generation_status("深度优化结果已应用，生图时将使用英文版本".into());
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
        let source = include_str!("prompt_optimization.rs");
        let start = source
            .split("fn start_prompt_optimization")
            .nth(1)
            .and_then(|value| value.split("fn run_prompt_request_scoped").next())
            .expect("start prompt optimization source");

        assert!(
            start.find("persist_pending_prompt_optimization_request(")
                .expect("durable pending request")
                < start
                    .find("run_prompt_request_scoped(")
                    .expect("billable create call")
        );
        assert!(start.contains("api.create_scoped(&request, &scope)"));
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
