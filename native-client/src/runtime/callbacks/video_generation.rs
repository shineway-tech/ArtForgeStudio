use super::*;

enum VideoGenerationOutcome {
    Success { local_path: String },
    Failure { message: String },
}

fn apply_video_model_selection(state: &AppState, model_code: &str) -> bool {
    if state.get_video_generating() || state.get_video_model().as_str() == model_code {
        return false;
    }
    let selected = state
        .get_catalog_models()
        .iter()
        .find(|model| model.code == model_code && model.purpose == "video_generation");
    let Some(selected) = selected else {
        return false;
    };

    state.set_video_model(selected.code);
    state.set_video_model_name(selected.name);
    state.set_video_model_description(selected.capabilities);
    state.set_video_quote_loading(false);
    state.set_video_quote_ready(false);
    state.set_video_quote_id("".into());
    state.set_video_credit_cost("".into());
    state.invoke_request_video_quote(
        state.get_video_aspect_ratio(),
        state.get_video_resolution(),
        state.get_video_duration_seconds(),
    );
    true
}

pub(super) fn wire_video_generation_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();
    let quote_epoch = Arc::new(AtomicU64::new(0));
    let pending_client_request_id = Arc::new(Mutex::new(String::new()));

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_video_model(move |model_code| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if apply_video_model_selection(&state, model_code.as_str()) {
                save_local_store(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_sync_video_player(move |x, y, width, height| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_video_result_path().to_string());
            if path.as_os_str().is_empty() {
                close_video_player();
                return;
            }
            if let Err(error) = sync_video_player(&app, &path, (x, y, width, height)) {
                state.set_video_status(format!("播放器打开失败：{error}").into());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let quote_epoch = quote_epoch.clone();
        let pending_client_request_id = pending_client_request_id.clone();
        state.on_viewer_generate_video(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            quote_epoch.fetch_add(1, Ordering::SeqCst);
            close_video_player();
            pending_client_request_id
                .lock()
                .unwrap_or_else(|value| value.into_inner())
                .clear();
            state.set_video_source_id(state.get_viewer_id());
            state.set_video_source_path(state.get_viewer_source_path());
            state.set_video_source_file_id("".into());
            state.set_video_source_image(state.get_viewer_image());
            state.set_video_source_title(state.get_viewer_title());
            state.set_video_prompt(state.get_viewer_prompt());
            state.set_video_aspect_ratio("16:9".into());
            state.set_video_resolution("720P".into());
            state.set_video_duration_seconds(4);
            state.set_video_quote_loading(false);
            state.set_video_quote_ready(false);
            state.set_video_credit_cost("".into());
            state.set_video_quote_id("".into());
            state.set_video_generating(false);
            state.set_video_progress(0);
            state.set_video_result_path("".into());
            state.set_video_task_id("".into());
            state.set_video_return_page(state.get_page());
            state.set_video_status(if state.get_video_service_available() {
                "正在获取服务端报价...".into()
            } else {
                "视频服务暂未开放".into()
            });
            state.set_viewer_open(false);
            navigate_to(&app, "video-generation");
            if state.get_video_service_available() {
                state.invoke_request_video_quote("16:9".into(), "720P".into(), 4);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let quote_epoch = quote_epoch.clone();
        state.on_close_video_generation(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            quote_epoch.fetch_add(1, Ordering::SeqCst);
            close_video_player();
            let state = app.global::<AppState>();
            let return_page = state.get_video_return_page().to_string();
            state.set_video_quote_loading(false);
            navigate_to_with_store(&app, &store.borrow(), &return_page);
            state.set_viewer_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let quote_epoch = quote_epoch.clone();
        state.on_request_video_quote(move |aspect_ratio, resolution, duration_secs| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_video_quote_ready(false);
            state.set_video_quote_id("".into());
            state.set_video_credit_cost("".into());
            if !state.get_video_service_available() || state.get_video_model().trim().is_empty() {
                state.set_video_quote_loading(false);
                state.set_video_status("视频服务暂未开放".into());
                return;
            }
            let Some(backend) = context.backend.clone() else {
                state.set_video_quote_loading(false);
                state.set_video_status("视频服务暂未开放".into());
                return;
            };
            let Some(session_scope) = context.current_account_session_scope() else {
                state.set_video_quote_loading(false);
                state.set_video_status("请先登录后再生成视频".into());
                return;
            };
            let source_path = state.get_video_source_path().to_string();
            if source_path.trim().is_empty() || !Path::new(&source_path).is_file() {
                state.set_video_quote_loading(false);
                state.set_video_status("源图片不可用，请返回后重新选择".into());
                return;
            }

            state.set_video_aspect_ratio(aspect_ratio.clone());
            state.set_video_resolution(resolution.clone());
            state.set_video_duration_seconds(duration_secs.clamp(4, 15));
            state.set_video_quote_loading(true);
            state.set_video_status("正在获取服务端报价...".into());
            let request_epoch = quote_epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let model_code = state.get_video_model().to_string();
            let existing_file_id = state.get_video_source_file_id().to_string();
            let weak = app.as_weak();
            let epoch = quote_epoch.clone();
            std::thread::spawn(move || {
                let api = GenerationApi::new(backend.api.clone());
                let result = (|| {
                    let source_file_id = if existing_file_id.trim().is_empty() {
                        api.upload_reference_scoped(Path::new(&source_path), &session_scope)?
                    } else {
                        existing_file_id
                    };
                    let quote = api.quote_video_scoped(
                        &CreateVideoQuote {
                            model_code,
                            source_file_id: source_file_id.clone(),
                            aspect_ratio: aspect_ratio.to_string(),
                            resolution: resolution.to_string(),
                            duration_secs,
                        },
                        &session_scope,
                    )?;
                    Ok::<_, ApiError>((source_file_id, quote))
                })();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    if epoch.load(Ordering::SeqCst) != request_epoch {
                        return;
                    }
                    let state = app.global::<AppState>();
                    state.set_video_quote_loading(false);
                    match result {
                        Ok((source_file_id, quote)) => {
                            state.set_video_source_file_id(source_file_id.into());
                            state.set_video_quote_id(quote.quote_id.into());
                            state.set_video_credit_cost(quote.credit_cost.into());
                            state.set_video_aspect_ratio(quote.aspect_ratio.into());
                            state.set_video_resolution(quote.resolution.into());
                            state.set_video_duration_seconds(quote.duration_secs);
                            state.set_video_quote_ready(true);
                            state.set_video_status("报价已更新".into());
                        }
                        Err(error) => {
                            state.set_video_quote_ready(false);
                            state.set_video_status(
                                format!("暂时无法获取视频报价：{}", error.user_message()).into(),
                            );
                        }
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let pending_client_request_id = pending_client_request_id.clone();
        state.on_submit_video_generation(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_video_generating() {
                return;
            }
            if !state.get_video_quote_ready() || state.get_video_quote_id().trim().is_empty() {
                state.set_video_status("请先获取有效的服务端报价".into());
                return;
            }
            if state.get_video_prompt().trim().is_empty() {
                state.set_video_status("请填写视频提示词".into());
                return;
            }
            let Some(backend) = context.backend.clone() else {
                state.set_video_status("视频服务暂未开放".into());
                return;
            };
            let Some(session_scope) = context.current_account_session_scope() else {
                state.set_video_status("请先登录后再生成视频".into());
                return;
            };
            let mut request_id = pending_client_request_id
                .lock()
                .unwrap_or_else(|value| value.into_inner());
            if request_id.is_empty() {
                *request_id = Uuid::new_v4().to_string();
            }
            let request = CreateVideoGenerationTask {
                client_request_id: request_id.clone(),
                task_type: "image_to_video".to_string(),
                model_code: state.get_video_model().to_string(),
                prompt: state.get_video_prompt().trim().to_string(),
                source_file_id: state.get_video_source_file_id().to_string(),
                aspect_ratio: state.get_video_aspect_ratio().to_string(),
                resolution: state.get_video_resolution().to_string(),
                duration_secs: state.get_video_duration_seconds(),
                quote_id: state.get_video_quote_id().to_string(),
            };
            drop(request_id);
            if let Err(error) = request.validate() {
                state.set_video_status(error.user_message().into());
                return;
            }

            state.set_video_generating(true);
            close_video_player();
            state.set_video_progress(0);
            state.set_video_status("正在提交视频生成任务...".into());
            let weak = app.as_weak();
            let completed_request_id = pending_client_request_id.clone();
            std::thread::spawn(move || {
                let outcome =
                    run_video_generation_task(weak.clone(), backend, session_scope, request);
                completed_request_id
                    .lock()
                    .unwrap_or_else(|value| value.into_inner())
                    .clear();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<AppState>();
                    state.set_video_generating(false);
                    match outcome {
                        VideoGenerationOutcome::Success { local_path } => {
                            state.set_video_progress(100);
                            state.set_video_result_path(local_path.into());
                            state.set_video_status("视频生成完成".into());
                        }
                        VideoGenerationOutcome::Failure { message } => {
                            state.set_video_status(message.into());
                        }
                    }
                });
            });
        });
    }
}

fn run_video_generation_task(
    app: Weak<AppWindow>,
    backend: Arc<BackendRuntime>,
    session_scope: SessionScope,
    request: CreateVideoGenerationTask,
) -> VideoGenerationOutcome {
    let api = GenerationApi::new(backend.api.clone());
    let mut task = match api.create_video_task_scoped(&request, &session_scope) {
        Ok(task) => task,
        Err(error) => {
            return VideoGenerationOutcome::Failure {
                message: format!("视频任务提交失败：{}", error.user_message()),
            };
        }
    };
    let task_id = task.id.clone();
    let task_id_for_ui = task_id.clone();
    let _ = app.clone().upgrade_in_event_loop(move |app| {
        let state = app.global::<AppState>();
        state.set_video_task_id(task_id_for_ui.into());
        state.set_video_status("视频任务已提交，正在生成...".into());
    });

    while !task.terminal() {
        std::thread::sleep(Duration::from_secs(1));
        task = match api.task_scoped(&task_id, &session_scope) {
            Ok(task) => task,
            Err(error) => {
                return VideoGenerationOutcome::Failure {
                    message: format!("视频任务查询失败：{}", error.user_message()),
                };
            }
        };
        let progress = task.progress_percent.clamp(0, 99);
        let _ = app.clone().upgrade_in_event_loop(move |app| {
            app.global::<AppState>().set_video_progress(progress);
        });
    }

    if !matches!(task.status.as_str(), "completed" | "partially_completed") {
        let reason = task
            .failure
            .as_ref()
            .map(TaskFailure::generation_message)
            .unwrap_or_else(|| "视频生成失败，请重试".to_string());
        return VideoGenerationOutcome::Failure { message: reason };
    }
    let Some(file) = task.items.iter().find_map(|item| {
        item.file
            .as_ref()
            .filter(|file| file.mime_type.starts_with("video/"))
    }) else {
        return VideoGenerationOutcome::Failure {
            message: "服务端没有返回可播放的视频文件".to_string(),
        };
    };
    let video_dir = app_data_dir().join("videos");
    if let Err(error) = fs::create_dir_all(&video_dir) {
        return VideoGenerationOutcome::Failure {
            message: format!("无法创建视频保存目录：{error}"),
        };
    }
    let local_path = video_dir.join(format!("{}.mp4", Uuid::new_v4()));
    if let Err(error) = api.download_verified_to_path_scoped(file, &session_scope, &local_path) {
        return VideoGenerationOutcome::Failure {
            message: format!("视频下载校验失败：{}", error.user_message()),
        };
    }
    let size_bytes = match file.size_bytes.parse::<u64>() {
        Ok(value) => value,
        Err(_) => {
            let _ = fs::remove_file(&local_path);
            return VideoGenerationOutcome::Failure {
                message: "服务端返回了无效的视频文件大小".to_string(),
            };
        }
    };
    if let Err(error) = api.acknowledge_delivery_scoped(
        &task_id,
        &file.id,
        &file.sha256,
        size_bytes,
        &session_scope,
    ) {
        return VideoGenerationOutcome::Failure {
            message: format!("视频已保存，但交付确认失败：{}", error.user_message()),
        };
    }
    VideoGenerationOutcome::Success {
        local_path: local_path.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_model(code: &str, name: &str, purpose: &str) -> CatalogModelView {
        CatalogModelView {
            code: code.into(),
            name: name.into(),
            purpose: purpose.into(),
            version: 1,
            capabilities: "支持图生视频".into(),
            pricing: String::new().into(),
            price_1k: 0,
            price_2k: 0,
            price_4k: 0,
            price_standard: String::new().into(),
            supports_image_edit: false,
            supports_style_analysis: false,
        }
    }

    #[test]
    fn selecting_video_model_invalidates_quote_and_requests_fresh_quote() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();

        state.set_catalog_models(ModelRc::new(VecModel::from(vec![
            catalog_model("seedance-lite", "Seedance Lite", "video_generation"),
            catalog_model("seedance-pro", "Seedance Pro", "video_generation"),
            catalog_model("image-only", "Image Only", "image_generation"),
        ])));
        state.set_video_model("seedance-lite".into());
        state.set_video_model_name("Seedance Lite".into());
        state.set_video_aspect_ratio("9:16".into());
        state.set_video_resolution("1080P".into());
        state.set_video_duration_seconds(8);
        state.set_video_quote_ready(true);
        state.set_video_quote_id("quote-old".into());
        state.set_video_credit_cost("42".into());

        let observed = Rc::new(RefCell::new(Vec::new()));
        {
            let observed = observed.clone();
            let weak = app.as_weak();
            state.on_request_video_quote(move |ratio, resolution, duration| {
                let app = weak.upgrade().expect("app remains alive");
                let state = app.global::<AppState>();
                observed.borrow_mut().push((
                    state.get_video_model().to_string(),
                    state.get_video_model_name().to_string(),
                    state.get_video_quote_ready(),
                    state.get_video_quote_id().to_string(),
                    state.get_video_credit_cost().to_string(),
                    ratio.to_string(),
                    resolution.to_string(),
                    duration,
                ));
            });
        }

        assert!(apply_video_model_selection(&state, "seedance-pro"));

        assert_eq!(
            observed.borrow().as_slice(),
            &[(
                "seedance-pro".to_string(),
                "Seedance Pro".to_string(),
                false,
                String::new(),
                String::new(),
                "9:16".to_string(),
                "1080P".to_string(),
                8,
            )]
        );
    }
}
