use super::*;

pub(super) fn video_rate(model: &CatalogModelView, quality: &str) -> String {
    match quality {
        "480P" => model.video_price_480.to_string(),
        "720P" => model.video_price_720.to_string(),
        "1080P" => model.video_price_1080.to_string(),
        _ => String::new(),
    }
}

fn checked_video_price(rate: &str, seconds: i32) -> Option<String> {
    if !(4..=15).contains(&seconds) {
        return None;
    }
    let rate = rate.parse::<u64>().ok().filter(|rate| *rate > 0)?;
    let total = rate.checked_mul(seconds as u64)?;
    (total <= i64::MAX as u64).then(|| total.to_string())
}

pub(super) fn estimate_video_price(state: &AppState) {
    state.set_video_quote_id("".into());
    state.set_video_quote_loading(false);
    let total = state
        .get_catalog_models()
        .iter()
        .find(|model| model.code == state.get_video_model())
        .and_then(|model| {
            checked_video_price(
                &video_rate(&model, state.get_video_resolution().as_str()),
                state.get_video_duration_seconds(),
            )
        });
    state.set_video_quote_ready(total.is_some() && state.get_video_service_available());
    state.set_video_credit_cost(total.unwrap_or_default().into());
    state.set_video_status(
        if state.get_video_quote_ready() {
            "积分已更新，生成时将由服务器校验"
        } else {
            "当前配置价格不可用"
        }
        .into(),
    );
}

fn poll_video_result<T: 'static>(
    receiver: mpsc::Receiver<Result<T, ApiError>>,
    complete: impl FnOnce(Result<T, ApiError>) + 'static,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(result) => complete(result),
            Err(mpsc::TryRecvError::Empty) => poll_video_result(receiver, complete),
            Err(mpsc::TryRecvError::Disconnected) => complete(Err(ApiError::LocalState {
                message: "视频准备工作已中断，请重试".into(),
            })),
        }
    });
}

pub(super) fn wire_video_price_refresh(app: &AppWindow, context: AppContext) {
    let weak = app.as_weak();
    let epoch = Rc::new(std::cell::Cell::new(0u64));
    app.global::<AppState>().on_refresh_video_prices(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if state.get_video_generating() {
            return;
        }
        let Some(persistence) = context.store.borrow().private_persistence.clone() else {
            return;
        };
        let Some(backend) = context.backend.clone() else {
            return;
        };
        let Ok((scope, _, activity)) = context.capture_billing_action(KnownCapability::Bill) else {
            return;
        };
        if !persistence.is_current() {
            return;
        }
        estimate_video_price(&state);
        epoch.set(epoch.get().wrapping_add(1));
        let captured_epoch = epoch.get();
        let epoch = epoch.clone();
        let worker_scope = scope.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _activity = activity;
            let result = AccountApi::new(backend.api.clone()).video_model_catalog(&worker_scope);
            let _ = tx.send(result);
        });
        let weak = app.as_weak();
        let context = context.clone();
        poll_video_result(rx, move |result| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if epoch.get() != captured_epoch
                || state.get_page() != "video-generation"
                || !backend_generation::paid_viewer_binding_matches(&context, &persistence)
                || !persistence.is_current()
                || !context.billing_context.is_current(&scope)
                || state.get_video_generating()
            {
                return;
            }
            match result {
                Ok(models) => {
                    apply_model_catalog_projection(&app, &context, &models);
                    estimate_video_price(&state);
                }
                Err(error) => {
                    state.set_video_status(format!("价格刷新失败：{}", error.user_message()).into())
                }
            }
        });
    });
}

pub(super) fn wire_video_submission(
    app: &AppWindow,
    context: AppContext,
    epoch: Arc<AtomicU64>,
    pending: Arc<Mutex<String>>,
) {
    let weak = app.as_weak();
    app.global::<AppState>()
        .on_submit_video_generation(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_video_generating() || state.get_optimizing_video_prompt() {
                return;
            }
            let Some(persistence) = context.store.borrow().private_persistence.clone() else {
                return;
            };
            if !persistence.is_current() {
                return;
            }
            if let Some(error) = video_image_generation_error(&state) {
                state.set_video_status(error.into());
                return;
            }
            if !state.get_video_quote_ready() || state.get_video_credit_cost().is_empty() {
                state.set_video_status("请先获取模型价格".into());
                return;
            }
            if state.get_video_prompt().trim().is_empty() {
                state.set_video_status("请填写视频提示词".into());
                return;
            }
            let Some(backend) = context.backend.clone() else {
                return;
            };
            let Some(session_scope) = context.current_account_session_scope() else {
                return;
            };
            let Ok((scope, authority, activity)) =
                context.capture_billing_action(KnownCapability::Bill)
            else {
                return;
            };
            let expected = state.get_video_credit_cost().to_string();
            let source_id = state.get_video_source_id().to_string();
            let paths: Vec<_> = state
                .get_video_images()
                .iter()
                .map(|row| PathBuf::from(row.source_path.as_str()))
                .collect();
            let mut key = pending.lock().unwrap_or_else(|p| p.into_inner());
            if key.is_empty() {
                *key = Uuid::new_v4().to_string();
            }
            let mut request = CreateVideoGenerationTask {
                client_request_id: key.clone(),
                task_type: "image_to_video".into(),
                model_code: state.get_video_model().to_string(),
                prompt: state.get_video_prompt().trim().to_string(),
                source_file_id: String::new(),
                reference_file_ids: Vec::new(),
                aspect_ratio: state.get_video_aspect_ratio().to_string(),
                resolution: state.get_video_resolution().to_string(),
                duration_secs: state.get_video_duration_seconds(),
                quote_id: String::new(),
            };
            drop(key);
            let captured_epoch = epoch.fetch_add(1, Ordering::SeqCst) + 1;
            state.set_video_generating(true);
            state.set_video_quote_loading(true);
            state.set_video_status("正在准备图片并校验价格...".into());
            let worker_scope = scope.clone();
            let worker_authority = authority.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _activity = activity;
                let result = (|| {
                    let api = GenerationApi::new(backend.api.clone());
                    for path in paths {
                        let (proof, _) =
                            backend_generation::capture_paid_image_file(&worker_authority, &path)
                                .map_err(|_| ApiError::LocalState {
                                message: "图片读取失败".into(),
                            })?;
                        request.reference_file_ids.push(
                            api.upload_reference_for_namespace_checked(
                                &path,
                                &worker_authority,
                                &session_scope,
                                false,
                                &proof.sha256,
                                proof.size,
                            )?,
                        );
                    }
                    request.source_file_id =
                        request.reference_file_ids.first().cloned().ok_or_else(|| {
                            ApiError::LocalState {
                                message: "请先添加图片".into(),
                            }
                        })?;
                    let quote = api.quote_video_billing(
                        &CreateVideoQuote {
                            model_code: request.model_code.clone(),
                            source_file_id: request.source_file_id.clone(),
                            reference_file_ids: request.reference_file_ids.clone(),
                            aspect_ratio: request.aspect_ratio.clone(),
                            resolution: request.resolution.clone(),
                            duration_secs: request.duration_secs,
                        },
                        &worker_scope,
                    )?;
                    if quote.aspect_ratio != request.aspect_ratio
                        || quote.resolution != request.resolution
                        || quote.duration_secs != request.duration_secs
                    {
                        return Err(ApiError::LocalState {
                            message: "服务端报价参数不一致，请重试".into(),
                        });
                    }
                    request.quote_id = quote.quote_id;
                    request.validate()?;
                    Ok::<_, ApiError>((request, quote.credit_cost))
                })();
                let _ = tx.send(result);
            });
            let weak = app.as_weak();
            let context = context.clone();
            let epoch = epoch.clone();
            let pending = pending.clone();
            poll_video_result(rx, move |result| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                let state = app.global::<AppState>();
                if !backend_generation::paid_viewer_binding_matches(&context, &persistence)
                    || !persistence.is_current()
                {
                    return;
                }
                if epoch.load(Ordering::SeqCst) != captured_epoch {
                    return;
                }
                state.set_video_generating(false);
                state.set_video_quote_loading(false);
                if !context.billing_context.is_current(&scope)
                    || state.get_page() != "video-generation"
                {
                    state.set_video_status("页面或付款账户已变化，请重新生成".into());
                    return;
                }
                match result {
                    Ok((request, total)) => {
                        if !confirm_video_price(&state, &request, &expected, &total) {
                            pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
                            return;
                        }
                        pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
                        backend_generation::start_backend_video(
                            &app,
                            context,
                            persistence,
                            authority,
                            &scope,
                            request,
                            source_id,
                        );
                    }
                    Err(error) => state
                        .set_video_status(format!("视频准备失败：{}", error.user_message()).into()),
                }
            });
        });
}

fn confirm_video_price(
    state: &AppState,
    request: &CreateVideoGenerationTask,
    expected: &str,
    total: &str,
) -> bool {
    if total == expected {
        return true;
    }
    update_video_rate(
        state,
        &request.model_code,
        &request.resolution,
        request.duration_secs,
        total,
    );
    state.set_video_credit_cost(total.into());
    state.set_video_status("价格已更新，请确认积分后再次点击生成".into());
    false
}

fn update_video_rate(state: &AppState, model_code: &str, quality: &str, seconds: i32, total: &str) {
    let Some(total) = total.parse::<u64>().ok().filter(|_| seconds > 0) else {
        return;
    };
    if total % seconds as u64 != 0 {
        return;
    }
    let rate = (total / seconds as u64).to_string();
    let models = state
        .get_catalog_models()
        .iter()
        .map(|mut model| {
            if model.code == model_code {
                match quality {
                    "480P" => model.video_price_480 = rate.clone().into(),
                    "720P" => model.video_price_720 = rate.clone().into(),
                    "1080P" => model.video_price_1080 = rate.clone().into(),
                    _ => {}
                }
            }
            model
        })
        .collect::<Vec<_>>();
    state.set_catalog_models(ModelRc::new(VecModel::from(models)));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> CatalogModelView {
        CatalogModelView {
            code: "seedance-test".into(),
            purpose: "video_generation".into(),
            video_price_720: "200".into(),
            ..Default::default()
        }
    }
    #[test]
    fn video_parameter_changes_estimate_without_network_or_images() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        wire_video_generation_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_catalog_models(ModelRc::new(VecModel::from(vec![model()])));
        state.set_video_model("seedance-test".into());
        state.set_video_service_available(true);
        let authority = fixture.authority.clone();
        let owned = std::thread::spawn(move || {
            persist_reference_image_for_namespace(&authority, &image::DynamicImage::new_rgb8(8, 8))
                .unwrap()
        })
        .join()
        .unwrap();
        state.set_video_source_path(owned.to_string_lossy().into_owned().into());
        state.set_video_images(ModelRc::new(VecModel::from(vec![VideoImageItem {
            id: "owned-reference".into(),
            source_path: owned.to_string_lossy().into_owned().into(),
            ..Default::default()
        }])));
        // Valid owned images are present: the former callback would upload here.
        // Fixture URL has no HTTP listener: these callbacks must finish synchronously.
        for duration in 4..=15 {
            state.invoke_request_video_quote("16:9".into(), "720P".into(), duration);
            assert_eq!(
                state.get_video_credit_cost().as_str(),
                (duration * 200).to_string()
            );
            assert!(state.get_video_quote_ready());
            assert!(!state.get_video_quote_loading());
            assert!(state.get_video_quote_id().is_empty());
        }
        state.set_video_images(ModelRc::new(VecModel::from(Vec::<VideoImageItem>::new())));
        state.invoke_request_video_quote("16:9".into(), "720P".into(), 4);
        assert_eq!(state.get_video_credit_cost(), "800");
        state.invoke_request_video_quote("16:9".into(), "480P".into(), 4);
        assert!(!state.get_video_quote_ready());
        assert!(state.get_video_credit_cost().is_empty());
        fixture.drain();
    }
    #[test]
    fn video_changed_quote_updates_cached_rate_for_next_confirmation() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_catalog_models(ModelRc::new(VecModel::from(vec![model()])));
        state.set_video_model("seedance-test".into());
        state.set_video_service_available(true);
        state.set_video_resolution("720P".into());
        state.set_video_duration_seconds(4);
        estimate_video_price(&state);
        let displayed = state.get_video_credit_cost();
        assert_ne!(displayed, "1000");
        update_video_rate(&state, "seedance-test", "720P", 4, "1000");
        estimate_video_price(&state);
        assert_eq!(state.get_video_credit_cost(), "1000");
        state.set_video_duration_seconds(5);
        estimate_video_price(&state);
        assert_eq!(state.get_video_credit_cost(), "1250");
    }

    #[test]
    fn video_changed_price_requires_second_click_before_task_submission() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_catalog_models(ModelRc::new(VecModel::from(vec![model()])));
        let request = CreateVideoGenerationTask {
            client_request_id: "request".into(),
            task_type: "image_to_video".into(),
            model_code: "seedance-test".into(),
            prompt: "move".into(),
            source_file_id: "source".into(),
            reference_file_ids: vec!["source".into()],
            aspect_ratio: "16:9".into(),
            resolution: "720P".into(),
            duration_secs: 4,
            quote_id: "quote".into(),
        };
        let mut submissions = 0;
        if confirm_video_price(&state, &request, "800", "1000") {
            submissions += 1;
        }
        assert_eq!(submissions, 0);
        assert_eq!(state.get_video_credit_cost(), "1000");
        if confirm_video_price(
            &state,
            &request,
            state.get_video_credit_cost().as_str(),
            "1000",
        ) {
            submissions += 1;
        }
        assert_eq!(submissions, 1);
    }
    #[test]
    fn video_preparation_disconnection_delivers_cleanup_error() {
        i_slint_backend_testing::init_no_event_loop();
        let (tx, rx) = mpsc::channel::<Result<(), ApiError>>();
        drop(tx);
        let completed = Rc::new(std::cell::Cell::new(false));
        let observed = completed.clone();
        poll_video_result(rx, move |result| {
            assert!(result.is_err());
            observed.set(true);
        });
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(60));
        assert!(completed.get());
    }
    #[test]
    fn video_price_refresh_does_not_mutate_unowned_or_busy_ui() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        wire_video_price_refresh(&app, AppContext::default());
        let state = app.global::<AppState>();
        state.set_video_credit_cost("retained-private-price".into());
        state.invoke_refresh_video_prices();
        assert_eq!(state.get_video_credit_cost(), "retained-private-price");
        state.set_video_generating(true);
        state.set_video_quote_loading(true);
        state.invoke_refresh_video_prices();
        assert!(state.get_video_quote_loading());
        assert!(state.get_video_generating());
    }
    #[test]
    fn video_local_prices_are_checked_integer_totals() {
        assert_eq!(checked_video_price("200", 4), Some("800".into()));
        assert_eq!(checked_video_price("", 4), None);
        assert_eq!(checked_video_price("0", 4), None);
        assert_eq!(checked_video_price("-1", 4), None);
        assert_eq!(checked_video_price("200", 16), None);
        assert_eq!(checked_video_price(&u64::MAX.to_string(), 15), None);
        assert_eq!(checked_video_price(&i64::MAX.to_string(), 4), None);
    }
}
