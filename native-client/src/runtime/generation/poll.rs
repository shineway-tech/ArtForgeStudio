use super::*;

pub(super) fn poll_generation_stream(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<RefCell<Option<mpsc::Receiver<GenerationOutcome>>>>,
    raw_prompt: String,
    category: String,
    mode: String,
    ratio: String,
    quality: String,
    image_model: String,
    conversation_id: String,
    create_conversation: bool,
    original_references: Vec<ReferenceData>,
    original_quote: QuoteContext,
    restore_inputs_on_failure: bool,
    task_id: String,
    started_at: Instant,
) {
    let store = context.store.clone();
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if let Some(app) = app_weak.upgrade() {
            if !active_generation_matches(&context, &category, &task_id) {
                return;
            }
            let elapsed = started_at.elapsed().as_secs() as i32;
            let wait_secs = IMAGE_GENERATION_WAIT_SECS as i32;
            update_active_generation_progress(
                &context,
                &app,
                &category,
                &task_id,
                (8 + elapsed * 88 / wait_secs).clamp(1, 96),
                (wait_secs - elapsed).clamp(1, wait_secs),
            );
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
                    Some(GenerationOutcome::Failure {
                        reason: "生成任务已中断，请重新生成。".to_string(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    })
                }
            }
        };

        let Some(outcome) = outcome else {
            poll_generation_stream(
                app_weak,
                context,
                receiver,
                raw_prompt,
                category,
                mode,
                ratio,
                quality,
                image_model,
                conversation_id,
                create_conversation,
                original_references,
                original_quote,
                restore_inputs_on_failure,
                task_id,
                started_at,
            );
            return;
        };

        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;

        match outcome {
            GenerationOutcome::Accepted { task_id: server_task_id } => {
                if let Some(task) = context.generations.active.borrow_mut().get_mut(&category) {
                    if task.task_id == task_id {
                        task.server_task_id = Some(server_task_id);
                    }
                }
                set_generation_status_for_category(
                    &context,
                    &app,
                    &category,
                    "任务已提交，正在排队...",
                );
            }
            GenerationOutcome::Progress { percent } => {
                update_active_generation_progress(
                    &context,
                    &app,
                    &category,
                    &task_id,
                    percent.clamp(1, 99),
                    0,
                );
            }
            GenerationOutcome::ImageSuccess {
                bytes,
                optimized,
                time,
                upscale_done,
                delivery,
            } => match add_stream_success_item(
                &app,
                &store,
                &raw_prompt,
                &category,
                &mode,
                &quality,
                &image_model,
                &conversation_id,
                &optimized,
                &time,
                &bytes,
                upscale_done,
            ) {
                Ok((conversation_image, source_path, generated_id)) => {
                    if let (Some(backend), Some(delivery)) = (context.backend.clone(), delivery) {
                        let _ = pending_delivery_saved(
                            &delivery.client_request_id,
                            &delivery,
                            &source_path,
                        );
                        acknowledge_delivery_after_local_save(backend, delivery);
                    }
                    state.set_asset_category_filter("all".into());
                    if create_conversation {
                        finish_conversation_placeholder(
                            &state,
                            &conversation_id,
                            Some(conversation_image),
                        );
                    }
                    if let Some(active) = mark_active_generation_image_completed(
                        &context,
                        &app,
                        &category,
                        &task_id,
                        true,
                        Some(generated_id),
                    ) {
                        if active.loading_count > 0 {
                            set_generation_status_for_category(
                                &context,
                                &app,
                                &category,
                                "正在生成...",
                            );
                        }
                    }
                }
                Err(error) => {
                    let reason = zh_error(&error.to_string());
                    let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
                    add_stream_failure_item(
                        &app,
                        &store,
                        &raw_prompt,
                        &category,
                        &mode,
                        &ratio,
                        &quality,
                        &image_model,
                        &conversation_id,
                        &reason,
                        &time,
                    );
                    mark_active_generation_image_completed(
                        &context, &app, &category, &task_id, false, None,
                    );
                }
            },
            GenerationOutcome::ImageFailure { reason, time } => {
                add_stream_failure_item(
                    &app,
                    &store,
                    &raw_prompt,
                    &category,
                    &mode,
                    &ratio,
                    &quality,
                    &image_model,
                    &conversation_id,
                    &reason,
                    &time,
                );
                if let Some(active) = mark_active_generation_image_completed(
                    &context, &app, &category, &task_id, false, None,
                ) {
                    if active.loading_count > 0 {
                        set_generation_status_for_category(
                            &context,
                            &app,
                            &category,
                            "正在生成...",
                        );
                    }
                }
            }
            GenerationOutcome::Finished => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let Some(task) = remove_active_generation(&context, &category, &task_id) else {
                    return;
                };
                if create_conversation && task.success_count == 0 {
                    finish_conversation_placeholder(&state, &conversation_id, None);
                }
                if restore_inputs_on_failure && task.failed_count > 0 && task.success_count == 0 {
                    restore_stream_inputs(
                        &app,
                        &store,
                        &category,
                        original_references.clone(),
                        original_quote.clone(),
                    );
                }
                set_stream_final_status(
                    &context,
                    &app,
                    &category,
                    task.success_count,
                    task.failed_count,
                );
                sync_generation_state_for_current_category(&context, &app);
                // open-viewer-after-finish
                if let Some(viewer_id) = task.latest_success_id.clone() {
                    open_viewer(&app, &store.borrow(), &viewer_id, "generation");
                }
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            GenerationOutcome::CreditInsufficient { message } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let Some(task) = remove_active_generation(&context, &category, &task_id) else {
                    return;
                };
                if create_conversation && task.success_count == 0 {
                    remove_conversation_placeholder(&state, &conversation_id);
                }
                if restore_inputs_on_failure {
                    restore_stream_inputs(
                        &app,
                        &store,
                        &category,
                        original_references.clone(),
                        original_quote.clone(),
                    );
                }
                context.generations.statuses.borrow_mut().remove(&category);
                sync_generation_state_for_current_category(&context, &app);
                state.set_credit_insufficient_message(message.into());
                state.set_credit_insufficient_open(true);
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            GenerationOutcome::Failure { reason, time } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let Some(task) = remove_active_generation(&context, &category, &task_id) else {
                    return;
                };
                let remaining = (task.total_count - task.completed_count).max(1);
                for _ in 0..remaining {
                    add_stream_failure_item(
                        &app,
                        &store,
                        &raw_prompt,
                        &category,
                        &mode,
                        &ratio,
                        &quality,
                        &image_model,
                        &conversation_id,
                        &reason,
                        &time,
                    );
                }
                if create_conversation && task.success_count == 0 {
                    finish_conversation_placeholder(&state, &conversation_id, None);
                }
                if restore_inputs_on_failure && task.success_count == 0 {
                    restore_stream_inputs(
                        &app,
                        &store,
                        &category,
                        original_references.clone(),
                        original_quote.clone(),
                    );
                }
                set_stream_final_status(
                    &context,
                    &app,
                    &category,
                    task.success_count,
                    task.failed_count + remaining,
                );
                sync_generation_state_for_current_category(&context, &app);
                if let Some(viewer_id) = task.latest_success_id.clone() {
                    open_viewer(&app, &store.borrow(), &viewer_id, "generation");
                }
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
        }

        if keep_polling {
            poll_generation_stream(
                app_weak,
                context,
                receiver,
                raw_prompt,
                category,
                mode,
                ratio,
                quality,
                image_model,
                conversation_id,
                create_conversation,
                original_references,
                original_quote,
                restore_inputs_on_failure,
                task_id,
                started_at,
            );
        }
    });
}

fn acknowledge_delivery_after_local_save(
    backend: Arc<BackendRuntime>,
    delivery: DeliveryConfirmation,
) {
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        for attempt in 0..5 {
            if api
                .acknowledge_delivery(
                    &delivery.task_id,
                    &delivery.file_id,
                    &delivery.sha256,
                    delivery.size_bytes,
                )
                .is_ok()
            {
                let _ = pending_delivery_acknowledged(
                    &delivery.client_request_id,
                    &delivery.file_id,
                );
                return;
            }
            if attempt < 4 {
                std::thread::sleep(Duration::from_secs(2_u64.pow(attempt)));
            }
        }
    });
}
