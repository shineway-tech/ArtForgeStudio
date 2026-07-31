use super::*;

#[derive(Clone, Copy)]
enum PromptRequestEffect {
    Start,
    Refresh,
    ApplyResult,
}

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
            run_prompt_request(
                &app,
                context.clone(),
                PromptRequestEffect::Refresh,
                move |api| {
                    api.review(
                        &id,
                        &request_id,
                        "continue_step",
                        feedback.as_deref(),
                        feedback_scope.as_deref(),
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
            let request_id = Uuid::new_v4().simple().to_string();
            run_prompt_request(
                &app,
                context.clone(),
                PromptRequestEffect::Refresh,
                move |api| api.review(&id, &request_id, "clear_stable_feedback", None, None),
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
            let request_id = Uuid::new_v4().simple().to_string();
            run_prompt_request(
                &app,
                context.clone(),
                PromptRequestEffect::ApplyResult,
                move |api| api.review(&id, &request_id, "use_current", None, None),
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
            let request_id = Uuid::new_v4().simple().to_string();
            run_prompt_request(
                &app,
                context.clone(),
                PromptRequestEffect::Refresh,
                move |api| api.review(&id, &request_id, "keep_original", None, None),
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
        let action_name = action.to_string();
        run_prompt_request(
            &app,
            handler_context.clone(),
            PromptRequestEffect::Refresh,
            move |api| match action_name.as_str() {
                "pause" => api.pause(&id),
                "resume" => api.resume(&id),
                "cancel" => api.cancel(&id),
                _ => api.retry(&id),
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
    let state = app.global::<AppState>();
    state.set_deep_optimization_open(true);
    state.set_deep_optimization_error("".into());
    let job_id = {
        let current = state.get_deep_optimization_job_id().to_string();
        if current.is_empty() {
            context.store.borrow().deep_prompt_job_id.clone()
        } else {
            current
        }
    };
    if !job_id.is_empty() {
        state.set_deep_optimization_job_id(job_id.clone().into());
        run_prompt_request(app, context, PromptRequestEffect::Refresh, move |api| {
            api.get(&job_id)
        });
        return;
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
    let state = app.global::<AppState>();
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
    clear_prompt_optimization_result(&state);
    state.set_deep_optimization_original_prompt(prompt.into());
    state.set_deep_optimization_stage("running".into());
    state.set_deep_optimization_progress(1);
    state.set_deep_optimization_error("".into());
    state.set_deep_optimization_phase_label("正在创建深度优化任务".into());
    state.set_deep_optimization_status_message("正在预留积分并进入任务队列...".into());
    state.set_deep_optimization_maximum_credits((max_rounds * 5).to_string().into());
    run_prompt_request(app, context, PromptRequestEffect::Start, move |api| {
        api.create(&request)
    });
}

fn run_prompt_request<F>(
    app: &AppWindow,
    context: AppContext,
    effect: PromptRequestEffect,
    request: F,
) where
    F: FnOnce(PromptOptimizationApi) -> std::result::Result<PromptOptimizationDetail, ApiError>
        + Send
        + 'static,
{
    let Some(backend) = context.backend.clone() else {
        app.global::<AppState>()
            .set_deep_optimization_error("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = request(PromptOptimizationApi::new(backend.api.clone()))
            .map_err(|error| error.user_message());
        let _ = sender.send(result);
    });
    poll_prompt_request(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
        effect,
    );
}

fn poll_prompt_request(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<PromptOptimizationDetail, String>>>>,
    >,
    effect: PromptRequestEffect,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
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
                    Some(Err("深度优化请求已中断，请重试".to_string()))
                }
            }
        };
        let Some(result) = result else {
            poll_prompt_request(app_weak, context, receiver, effect);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match result {
            Ok(detail) => {
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
                apply_prompt_optimization_detail(&app, &detail);
                persist_prompt_optimization_job(&app, &context, &id);
                if let Some(result) = applied {
                    apply_prompt_versions(&app, &context, result);
                    refresh_backend_snapshot(&app, context.clone());
                } else if matches!(effect, PromptRequestEffect::Start) || status == "cancelled" {
                    refresh_backend_snapshot(&app, context.clone());
                }
                if matches!(status.as_str(), "queued" | "processing") {
                    begin_prompt_optimization_polling(&app, context, id);
                }
            }
            Err(message) => {
                let state = app.global::<AppState>();
                state.set_deep_optimization_error(message.clone().into());
                if matches!(effect, PromptRequestEffect::Start) {
                    state.set_deep_optimization_stage("settings".into());
                } else if state.get_session_state().as_str() == "online" {
                    let id = state.get_deep_optimization_job_id().to_string();
                    if !id.is_empty() {
                        let retry_app = app.as_weak();
                        let retry_context = context.clone();
                        slint::Timer::single_shot(Duration::from_secs(2), move || {
                            if let Some(app) = retry_app.upgrade() {
                                run_prompt_request(
                                    &app,
                                    retry_context,
                                    PromptRequestEffect::Refresh,
                                    move |api| api.get(&id),
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

fn persist_prompt_optimization_job(app: &AppWindow, context: &AppContext, id: &str) {
    app.global::<AppState>()
        .set_deep_optimization_job_id(id.into());
    context.store.borrow_mut().deep_prompt_job_id = id.to_string();
    save_local_store(app, &context.store.borrow());
}

fn clear_prompt_optimization_job(app: &AppWindow, context: &AppContext) {
    let state = app.global::<AppState>();
    state.set_deep_optimization_job_id("".into());
    clear_prompt_optimization_result(&state);
    context.store.borrow_mut().deep_prompt_job_id.clear();
    save_local_store(app, &context.store.borrow());
}

fn clear_prompt_optimization_result(state: &AppState) {
    state.set_deep_optimization_chinese_prompt("".into());
    state.set_deep_optimization_english_prompt("".into());
    state.set_deep_optimization_highlighted_original(styled_markdown(""));
    state.set_deep_optimization_highlighted_chinese(styled_markdown(""));
    state.set_deep_optimization_change_summary("".into());
}

fn begin_prompt_optimization_polling(app: &AppWindow, context: AppContext, id: String) {
    if app.global::<AppState>().get_session_state().as_str() != "online" {
        *context.prompt_optimization_polling.borrow_mut() = None;
        return;
    }
    if context.prompt_optimization_polling.borrow().as_deref() == Some(id.as_str()) {
        return;
    }
    *context.prompt_optimization_polling.borrow_mut() = Some(id.clone());
    poll_prompt_optimization_once(app.as_weak(), context, id);
}

fn poll_prompt_optimization_once(app_weak: Weak<AppWindow>, context: AppContext, id: String) {
    let Some(backend) = context.backend.clone() else {
        *context.prompt_optimization_polling.borrow_mut() = None;
        return;
    };
    let (sender, receiver) = mpsc::channel();
    let request_id = id.clone();
    std::thread::spawn(move || {
        let result = PromptOptimizationApi::new(backend.api.clone()).get(&request_id);
        let _ = sender.send(result);
    });
    poll_prompt_optimization_once_result(
        app_weak,
        context,
        id,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_prompt_optimization_once_result(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    id: String,
    receiver: Rc<
        RefCell<Option<mpsc::Receiver<std::result::Result<PromptOptimizationDetail, ApiError>>>>,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
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
                    None
                }
            }
        };
        let Some(result) = result else {
            if receiver.borrow().is_some() {
                poll_prompt_optimization_once_result(app_weak, context, id, receiver);
            } else {
                *context.prompt_optimization_polling.borrow_mut() = None;
            }
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match result {
            Ok(detail) => {
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
                    let next_id = id.clone();
                    slint::Timer::single_shot(Duration::from_secs(2), move || {
                        poll_prompt_optimization_once(next_app, next_context, next_id);
                    });
                } else {
                    *context.prompt_optimization_polling.borrow_mut() = None;
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
                *context.prompt_optimization_polling.borrow_mut() = None;
                app.global::<AppState>()
                    .set_deep_optimization_status_message(
                        format!("状态同步暂时失败：{}", error.user_message()).into(),
                    );
                let retry_app = app.as_weak();
                let retry_context = context.clone();
                slint::Timer::single_shot(Duration::from_secs(5), move || {
                    if let Some(app) = retry_app.upgrade() {
                        begin_prompt_optimization_polling(&app, retry_context, id);
                    }
                });
            }
        }
    });
}

pub(super) fn recover_prompt_optimization(app: &AppWindow, context: AppContext) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let stored_id = context.store.borrow().deep_prompt_job_id.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let api = PromptOptimizationApi::new(backend.api.clone());
        let detail = if stored_id.is_empty() {
            api.active().and_then(|items| match items.first() {
                Some(item) => api.get(&item.id).map(Some),
                None => Ok(None),
            })
        } else {
            match api.get(&stored_id) {
                Ok(detail) => Ok(Some(detail)),
                Err(_) => api.active().and_then(|items| match items.first() {
                    Some(item) => api.get(&item.id).map(Some),
                    None => Ok(None),
                }),
            }
        };
        let _ = sender.send(detail);
    });
    poll_prompt_recovery(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_prompt_recovery(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<
        RefCell<
            Option<mpsc::Receiver<std::result::Result<Option<PromptOptimizationDetail>, ApiError>>>,
        >,
    >,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
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
                    None
                }
            }
        };
        let Some(result) = result else {
            if receiver.borrow().is_some() {
                poll_prompt_recovery(app_weak, context, receiver);
            }
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if let Ok(Some(detail)) = result {
            let id = detail.id.clone();
            let active = matches!(detail.status.as_str(), "queued" | "processing");
            let completed_result = if detail.status == "completed" {
                detail.final_result.clone()
            } else {
                None
            };
            apply_prompt_optimization_detail(&app, &detail);
            persist_prompt_optimization_job(&app, &context, &id);
            if let Some(result) = completed_result {
                apply_prompt_versions(&app, &context, result);
            }
            if active {
                begin_prompt_optimization_polling(&app, context, id);
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
    use super::{
        best_result_comparison_base, collect_lcs_matches, displayed_best_score,
        highlighted_prompt_markdown, optimization_change_summary, prompt_diff_pieces,
        prompt_diff_tokens, styled_markdown,
    };
    use crate::runtime::api::{
        PromptOptimizationDetail, PromptOptimizationResult, PromptOptimizationRound,
    };

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
