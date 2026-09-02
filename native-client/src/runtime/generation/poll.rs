use super::*;

pub(super) fn retain_failed_delivery_after_replacement_failure(
    store: &mut Store,
    failed_asset_id: &str,
) -> bool {
    let Some(asset) = store.generations.iter_mut().find(|asset| {
        asset.id == failed_asset_id && asset.source_path == "failed" && asset.delivery_recoverable
    }) else {
        return false;
    };
    asset.delivery_downloading = false;
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GenerationPollTermination {
    ScopeRejected,
    WindowDropped,
    ActiveGenerationMissing,
    ReceiverDropped,
    OutcomeScopeRejected,
    OutcomeActiveGenerationMissing,
    Finished,
    CreditInsufficient,
    WorkerFailure,
    ReceiverDisconnected,
}

#[cfg(test)]
impl GenerationPollTermination {
    const ALL: [Self; 10] = [
        Self::ScopeRejected,
        Self::WindowDropped,
        Self::ActiveGenerationMissing,
        Self::ReceiverDropped,
        Self::OutcomeScopeRejected,
        Self::OutcomeActiveGenerationMissing,
        Self::Finished,
        Self::CreditInsufficient,
        Self::WorkerFailure,
        Self::ReceiverDisconnected,
    ];
}

fn terminate_generation_poll(
    context: &AppContext,
    app: Option<&AppWindow>,
    receiver: &Rc<RefCell<Option<mpsc::Receiver<GenerationOutcome>>>>,
    reservations: &[DeliveryDownloadReservation],
    _termination: GenerationPollTermination,
) {
    receiver.borrow_mut().take();
    release_delivery_download_reservations(context, reservations);
    if let Some(app) = app {
        refresh_delivery_download_flags(app, context);
    }
}

pub(super) fn poll_generation_stream(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    delivery_download_reservations: Vec<DeliveryDownloadReservation>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<GenerationOutcome>>>>,
    raw_prompt: String,
    category: String,
    mode: String,
    ratio: String,
    quality: String,
    image_model: String,
    result_origin: String,
    conversation_id: String,
    create_conversation: bool,
    generation_reference_paths: Vec<String>,
    original_references: Vec<ReferenceData>,
    original_quote: QuoteContext,
    restore_inputs_on_failure: bool,
    task_id: String,
    started_at: Instant,
) {
    let store = context.store.clone();
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            let app = app_weak.upgrade();
            terminate_generation_poll(
                &context,
                app.as_ref(),
                &receiver,
                &delivery_download_reservations,
                GenerationPollTermination::ScopeRejected,
            );
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            terminate_generation_poll(
                &context,
                None,
                &receiver,
                &delivery_download_reservations,
                GenerationPollTermination::WindowDropped,
            );
            return;
        };
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            terminate_generation_poll(
                &context,
                Some(&app),
                &receiver,
                &delivery_download_reservations,
                GenerationPollTermination::ScopeRejected,
            );
            return;
        }
        if !active_generation_matches_scope(&context, &category, &task_id, &session_scope) {
            terminate_generation_poll(
                &context,
                Some(&app),
                &receiver,
                &delivery_download_reservations,
                GenerationPollTermination::ActiveGenerationMissing,
            );
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

        let (outcome, worker_failure_termination) = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                drop(slot);
                terminate_generation_poll(
                    &context,
                    Some(&app),
                    &receiver,
                    &delivery_download_reservations,
                    GenerationPollTermination::ReceiverDropped,
                );
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => (Some(outcome), None),
                Err(TryRecvError::Empty) => (None, None),
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    (
                        Some(GenerationOutcome::Failure {
                            reason: "生成任务已中断，请重新生成。".to_string(),
                            time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                        }),
                        Some(GenerationPollTermination::ReceiverDisconnected),
                    )
                }
            }
        };

        let Some(outcome) = outcome else {
            poll_generation_stream(
                app_weak,
                context,
                session_scope,
                delivery_download_reservations,
                receiver,
                raw_prompt,
                category,
                mode,
                ratio,
                quality,
                image_model,
                result_origin,
                conversation_id,
                create_conversation,
                generation_reference_paths,
                original_references,
                original_quote,
                restore_inputs_on_failure,
                task_id,
                started_at,
            );
            return;
        };

        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            terminate_generation_poll(
                &context,
                Some(&app),
                &receiver,
                &delivery_download_reservations,
                GenerationPollTermination::OutcomeScopeRejected,
            );
            return;
        }
        if !active_generation_matches_scope(&context, &category, &task_id, &session_scope) {
            terminate_generation_poll(
                &context,
                Some(&app),
                &receiver,
                &delivery_download_reservations,
                GenerationPollTermination::OutcomeActiveGenerationMissing,
            );
            return;
        }
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        let destination = context
            .generations
            .active
            .borrow()
            .get(&category)
            .map(|task| task.destination.clone())
            .unwrap_or_default();

        match outcome {
            GenerationOutcome::Accepted {
                task_id: server_task_id,
            } => {
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
                local_path,
                display_prompt,
                time,
                upscale_done,
                delivery,
            } => {
                let replacing_failed_asset_id = delivery
                    .as_ref()
                    .and_then(|delivery| delivery.failed_asset_id.as_deref())
                    .map(ToOwned::to_owned);
                let replacing_failed_delivery = replacing_failed_asset_id.is_some();
                let delivery_download = delivery
                    .as_ref()
                    .filter(|delivery| delivery.failed_asset_id.is_some())
                    .map(|delivery| {
                        (
                            delivery.client_request_id.clone(),
                            delivery.file_id.clone(),
                        )
                    });
                let active = context.generations.active.borrow().get(&category).cloned();
                let saved_result = match (&destination, active.as_ref()) {
                    (GenerationDestination::Canvas { source_node_id }, Some(active)) => {
                        std::fs::read(&local_path)
                            .map_err(anyhow::Error::from)
                            .and_then(|bytes| {
                                add_canvas_stream_success_item(
                                    &app,
                                    &context,
                                    source_node_id,
                                    &raw_prompt,
                                    &bytes,
                                    active.success_count,
                                    active.total_count,
                                    &mode,
                                    &quality,
                                    &image_model,
                                    &result_origin,
                                    &conversation_id,
                                    &display_prompt,
                                    &time,
                                    &generation_reference_paths,
                                    upscale_done,
                                )
                            })
                            .map(|source_path| (Image::default(), source_path, String::new()))
                    }
                    _ if delivery
                        .as_ref()
                        .and_then(|delivery| delivery.failed_asset_id.as_deref())
                        .is_some() =>
                    {
                        let failed_asset_id = delivery
                            .as_ref()
                            .and_then(|delivery| delivery.failed_asset_id.as_deref())
                            .unwrap();
                        replace_failed_delivery_asset_checked(
                            &app,
                            &store,
                            failed_asset_id,
                            Path::new(&local_path),
                            &time,
                        )
                        .map(|(source_path, generated_id)| {
                            let conversation_image = load_preview_image(
                                Path::new(&source_path),
                                PreviewPurpose::Reference,
                            )
                            .unwrap_or_default();
                            (conversation_image, source_path, generated_id)
                        })
                    }
                    _ => add_stream_success_item(
                        &app,
                        &store,
                        &raw_prompt,
                        &category,
                        &mode,
                        &quality,
                        &image_model,
                        &result_origin,
                        &conversation_id,
                        &display_prompt,
                        &time,
                        Path::new(&local_path),
                        &generation_reference_paths,
                        upscale_done,
                    ),
                };
                match saved_result {
                    Ok((conversation_image, source_path, generated_id)) => {
                        if let Some(delivery) = delivery {
                            let delivery_for_persistence = delivery.clone();
                            let _ = pending_delivery_saved_then_acknowledge_with(
                                || {
                                    pending_delivery_saved(
                                        &session_scope.owner_user_id,
                                        session_scope.auth_epoch,
                                        &delivery_for_persistence.client_request_id,
                                        &delivery_for_persistence,
                                        &source_path,
                                    )
                                },
                                || {
                                    acknowledge_delivery_after_local_save(
                                        app.as_weak(),
                                        context.clone(),
                                        session_scope.clone(),
                                        delivery,
                                    );
                                },
                            );
                        }
                        if destination != GenerationDestination::Gallery {
                            cleanup_failed_delivery_staging(Path::new(&local_path));
                        }
                        if destination == GenerationDestination::Gallery
                            && !replacing_failed_delivery
                        {
                            state.set_asset_category_filter("all".into());
                        }
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
                            (!generated_id.is_empty()).then_some(generated_id),
                            None,
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
                        // The recovery record still has the server item and can download it again.
                        // Remove only this app-managed staging file so a local save failure does not
                        // accumulate full-size downloads indefinitely.
                        cleanup_failed_delivery_staging(Path::new(&local_path));
                        let reason = zh_error(&error.to_string());
                        if let Some(failed_asset_id) = replacing_failed_asset_id.as_deref() {
                            let mut store_mut = store.borrow_mut();
                            let _ = retain_failed_delivery_after_replacement_failure(
                                &mut store_mut,
                                failed_asset_id,
                            );
                            push_all(&app, &store_mut);
                            drop(store_mut);
                            set_generation_status_for_category(&context, &app, &category, &reason);
                        } else if destination == GenerationDestination::Gallery {
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
                                &result_origin,
                                &conversation_id,
                                &reason,
                                &time,
                                &generation_reference_paths,
                                None,
                            );
                        } else {
                            set_generation_status_for_category(&context, &app, &category, &reason);
                        }
                        mark_active_generation_image_completed(
                            &context,
                            &app,
                            &category,
                            &task_id,
                            false,
                            None,
                            Some(&reason),
                        );
                    }
                }
                if let Some((client_request_id, file_id)) = delivery_download {
                    if let Some(reservation) = delivery_download_reservations.iter().find(
                        |reservation| {
                            reservation.key.client_request_id == client_request_id
                                && reservation.key.file_id == file_id
                        },
                    ) {
                        complete_delivery_download(&app, &context, reservation);
                    }
                }
            }
            GenerationOutcome::ImageFailure {
                reason,
                time,
                delivery,
            } => {
                if destination == GenerationDestination::Gallery {
                    add_stream_failure_item(
                        &app,
                        &store,
                        &raw_prompt,
                        &category,
                        &mode,
                        &ratio,
                        &quality,
                        &image_model,
                        &result_origin,
                        &conversation_id,
                        &reason,
                        &time,
                        &generation_reference_paths,
                        delivery
                            .as_ref()
                            .and_then(|delivery| delivery.failed_asset_id.as_deref()),
                    );
                }
                if let Some(active) = mark_active_generation_image_completed(
                    &context,
                    &app,
                    &category,
                    &task_id,
                    false,
                    None,
                    Some(&reason),
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
                if let Some(delivery) = delivery.as_ref() {
                    if let Some(reservation) = delivery_download_reservations.iter().find(
                        |reservation| {
                            reservation.key.client_request_id == delivery.client_request_id
                                && reservation.key.file_id == delivery.file_id
                        },
                    ) {
                        complete_delivery_download(&app, &context, reservation);
                    }
                }
            }
            GenerationOutcome::Finished => {
                keep_polling = false;
                let task = remove_active_generation(&context, &category, &task_id);
                terminate_generation_poll(
                    &context,
                    Some(&app),
                    &receiver,
                    &delivery_download_reservations,
                    GenerationPollTermination::Finished,
                );
                let Some(task) = task else {
                    return;
                };
                if task.success_count == 0 {
                    discard_canvas_generation_placeholder(&state, &task.destination);
                }
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
                    task.last_failure_reason.as_deref(),
                );
                sync_generation_state_for_current_category(&context, &app);
                // open-viewer-after-finish
                if destination == GenerationDestination::Gallery {
                    if let Some(viewer_id) = task.latest_success_id.clone() {
                        open_viewer(&app, &store.borrow(), &viewer_id, "generation");
                    }
                }
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            GenerationOutcome::CreditInsufficient { message } => {
                keep_polling = false;
                let task = remove_active_generation(&context, &category, &task_id);
                terminate_generation_poll(
                    &context,
                    Some(&app),
                    &receiver,
                    &delivery_download_reservations,
                    GenerationPollTermination::CreditInsufficient,
                );
                let Some(task) = task else {
                    return;
                };
                discard_canvas_generation_placeholder(&state, &task.destination);
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
                let task = remove_active_generation(&context, &category, &task_id);
                terminate_generation_poll(
                    &context,
                    Some(&app),
                    &receiver,
                    &delivery_download_reservations,
                    worker_failure_termination
                        .unwrap_or(GenerationPollTermination::WorkerFailure),
                );
                let Some(task) = task else {
                    return;
                };
                if task.success_count == 0 {
                    discard_canvas_generation_placeholder(&state, &task.destination);
                }
                let remaining = (task.total_count - task.completed_count).max(1);
                if destination == GenerationDestination::Gallery {
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
                            &result_origin,
                            &conversation_id,
                            &reason,
                            &time,
                            &generation_reference_paths,
                            None,
                        );
                    }
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
                    Some(&reason),
                );
                sync_generation_state_for_current_category(&context, &app);
                if destination == GenerationDestination::Gallery {
                    if let Some(viewer_id) = task.latest_success_id.clone() {
                        open_viewer(&app, &store.borrow(), &viewer_id, "generation");
                    }
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
                session_scope,
                delivery_download_reservations,
                receiver,
                raw_prompt,
                category,
                mode,
                ratio,
                quality,
                image_model,
                result_origin,
                conversation_id,
                create_conversation,
                generation_reference_paths,
                original_references,
                original_quote,
                restore_inputs_on_failure,
                task_id,
                started_at,
            );
        }
    });
}

pub(super) fn acknowledge_delivery_after_local_save(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    delivery: DeliveryConfirmation,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let (sender, receiver) = mpsc::channel::<()>();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        for attempt in 0..5 {
            if api
                .acknowledge_delivery_scoped(
                    &delivery.task_id,
                    &delivery.file_id,
                    &delivery.sha256,
                    delivery.size_bytes,
                    &worker_scope,
                )
                .is_ok()
            {
                let _ = pending_delivery_acknowledged(
                    &worker_scope.owner_user_id,
                    worker_scope.auth_epoch,
                    &delivery.client_request_id,
                    &delivery.file_id,
                );
                let _ = sender.send(());
                return;
            }
            if attempt < 4 {
                std::thread::sleep(Duration::from_secs(2_u64.pow(attempt)));
            }
        }
        let _ = sender.send(());
    });
    observe_detached_generation_scope(
        app_weak,
        context,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_poll_terminal_and_abort_path_releases_its_captured_reservations() {
        let scope = SessionScope {
            owner_user_id: "user-a".to_string(),
            auth_epoch: 7,
        };
        let pair = [("request-a".to_string(), "file-1".to_string())];

        for termination in GenerationPollTermination::ALL {
            let context = AppContext::default();
            let reservations = try_reserve_delivery_download_pairs(
                &context.generations,
                &scope,
                &pair,
            )
            .expect("reserve delivery before terminating poll");
            let (_sender, receiver) = mpsc::channel::<GenerationOutcome>();
            let receiver = Rc::new(RefCell::new(Some(receiver)));

            terminate_generation_poll(
                &context,
                None,
                &receiver,
                &reservations,
                termination,
            );

            assert!(receiver.borrow().is_none(), "{termination:?}");
            assert!(
                try_reserve_delivery_download_pairs(&context.generations, &scope, &pair)
                    .is_some(),
                "{termination:?}"
            );
        }
    }
}
