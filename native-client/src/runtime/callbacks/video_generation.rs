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
    wire_video_prompt_callbacks(app, context.clone());
    let state = app.global::<AppState>();
    let store = context.store.clone();
    let quote_epoch = Arc::new(AtomicU64::new(0));
    let pending_client_request_id = Arc::new(Mutex::new(String::new()));
    let image_epoch = wire_video_image_callbacks(app, store.clone(), quote_epoch.clone(), pending_client_request_id.clone());

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
            if state.get_video_prompt_expanded_open() || state.get_recovered_prompt_result_open() || state.get_video_image_dialog() != "" {
                set_video_player_visible(false);
                return;
            }
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
        let image_epoch = image_epoch.clone();
        let context = context.clone();
        state.on_viewer_generate_video(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            quote_epoch.fetch_add(1, Ordering::SeqCst);
            reset_video_images(&state, &image_epoch);
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
            let owner = context.current_user_id.lock()
                .unwrap_or_else(|value| value.into_inner()).clone().unwrap_or_default();
            state.set_video_prompt(video_prompt_for_source(
                &context.store.borrow().prompt_drafts,
                &owner,
                state.get_viewer_id().as_str(),
                state.get_viewer_prompt().as_str(),
            ).into());
            state.set_video_prompt_expanded_open(false);
            state.set_video_prompt_status("".into());
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
            recover_pending_prompt_tasks(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let quote_epoch = quote_epoch.clone();
        let image_epoch = image_epoch.clone();
        state.on_close_video_generation(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            quote_epoch.fetch_add(1, Ordering::SeqCst);
            close_video_player();
            let state = app.global::<AppState>();
            cancel_video_image_work(&state, &image_epoch);
            let return_page = state.get_video_return_page().to_string();
            state.set_video_prompt_expanded_open(false);
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
            let request_epoch = quote_epoch.fetch_add(1, Ordering::SeqCst) + 1;
            state.set_video_quote_ready(false);
            state.set_video_quote_id("".into());
            state.set_video_credit_cost("".into());
            if let Some(error) = video_image_generation_error(&state) {
                state.set_video_quote_loading(false);
                state.set_video_status(error.into());
                return;
            }
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
            if state.get_video_generating() || state.get_optimizing_video_prompt() {
                return;
            }
            if let Some(error) = video_image_generation_error(&state) {
                state.set_video_status(error.into());
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

    #[test]
    fn video_workspace_starts_with_a_thumbnail_and_offers_both_image_sources() {
        use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
        use slint::platform::PointerEventButton;

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        }))).unwrap();
        let app = AppWindow::new().unwrap();
        wire_video_generation_callbacks(&app, AppContext::default());
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_session_state("offline".into());
        state.set_page("assets".into());
        state.set_viewer_id("source".into());
        state.set_viewer_source_path("source.png".into());
        state.set_viewer_title("Source image".into());
        state.set_viewer_prompt("Keep this prompt".into());
        state.invoke_viewer_generate_video();
        assert_eq!(state.get_page(), "video-generation");
        assert_eq!(state.get_video_images().row_count(), 1);
        app.show().unwrap();

        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0), (1920.0, 1080.0)] {
            app.window().set_size(slint::LogicalSize::new(width, height));
            let cards: Vec<_> = ElementHandle::find_by_element_type_name(&app, "VideoImageCard").collect();
            assert_eq!(cards.len(), 1, "the original image must appear as one thumbnail");
            assert!(cards[0].size().width <= 240.0 && cards[0].size().height <= 280.0,
                "a source image must not grow into a full-height preview");
            let add = ElementHandle::find_by_element_id(&app, "VideoImageGrid::add-image")
                .next().expect("add tile beside the image");
            assert!(add.absolute_position().x >= cards[0].absolute_position().x + cards[0].size().width);
        }
        ElementHandle::find_by_element_id(&app, "VideoImageGrid::add-image").next().unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert!(ElementHandle::find_by_accessible_label(&app, "从我的资产添加").next().is_some());
        assert!(ElementHandle::find_by_accessible_label(&app, "本地上传").next().is_some());
        assert_eq!(state.get_video_prompt(), "Keep this prompt");
    }

    #[test]
    fn viewer_video_workspace_uses_full_width_and_restores_navigation_on_return() {
        use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
        use slint::platform::PointerEventButton;

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        })))
        .unwrap();
        let app = AppWindow::new().unwrap();
        super::super::app::wire_callbacks(&app, AppContext::default());
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_session_state("offline".into());
        app.show().unwrap();

        for (origin, collapsed, width, height, top_bar_back) in [
            ("generation", false, 1180.0, 760.0, false),
            ("assets", true, 1440.0, 900.0, true),
            ("generation", true, 1920.0, 1080.0, true),
            ("assets", false, 1440.0, 900.0, false),
        ] {
            app.window().set_size(slint::LogicalSize::new(width, height));
            state.set_page(origin.into());
            state.set_sidebar_collapsed(collapsed);
            state.set_viewer_source(origin.into());
            state.set_viewer_id("video-source-image".into());
            state.set_viewer_title("Source image".into());
            state.set_viewer_prompt("Keep the original scene and move the camera slowly.".into());
            state.set_viewer_open(true);
            let sidebar = ElementHandle::find_by_element_type_name(&app, "Sidebar")
                .next().expect("navigation is available before entering the video page");
            // Read the settled layout, not the sidebar's 160ms collapse animation.
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(200));
            let sidebar_width = sidebar.size().width;

            ElementHandle::find_by_element_type_name(&app, "ViewerFooterActionButton")
                .nth(1).expect("generate video action in image details")
                .mock_single_click(PointerEventButton::Left);

            assert_eq!(state.get_page(), "video-generation");
            assert!(!state.get_viewer_open());
            assert_eq!(state.get_video_source_id(), "video-source-image");
            assert_eq!(state.get_video_prompt(), "Keep the original scene and move the camera slowly.");
            assert!(ElementHandle::find_by_element_type_name(&app, "Sidebar").next().is_none(),
                "the video workspace must not show the main navigation sidebar");
            let video_page = ElementHandle::find_by_element_type_name(&app, "VideoGenerationPage")
                .next().expect("dedicated video generation page");
            assert!(video_page.absolute_position().x.abs() < 1.0);
            assert!((video_page.size().width - width).abs() < 1.0,
                "hiding navigation must also reclaim its layout width");
            let preview = ElementHandle::find_by_element_id(&app, "VideoGenerationPage::preview-card")
                .next().unwrap();
            let settings = ElementHandle::find_by_element_id(&app, "VideoGenerationPage::settings-card")
                .next().unwrap();
            assert!(preview.absolute_position().x + preview.size().width < settings.absolute_position().x);
            assert!(settings.absolute_position().x + settings.size().width <= width);
            assert!(settings.absolute_position().y + settings.size().height <= height);

            let back_container = if top_bar_back {
                ElementHandle::find_by_element_type_name(&app, "TopBar").next().unwrap()
            } else {
                video_page
            };
            back_container.query_descendants().match_inherits("PillButton")
                .find_first().expect("back action")
                .mock_single_click(PointerEventButton::Left);

            assert_eq!(state.get_page().as_str(), origin);
            assert!(state.get_viewer_open(), "return to the current image details");
            assert_eq!(state.get_viewer_id(), "video-source-image");
            assert_eq!(state.get_sidebar_collapsed(), collapsed);
            let restored_sidebar = ElementHandle::find_by_element_type_name(&app, "Sidebar")
                .next().expect("navigation must return after leaving the video page");
            assert_eq!(restored_sidebar.size().width, sidebar_width);
        }
    }

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
    fn video_prompt_expand_edits_the_full_text_without_changing_image_prompt() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;

        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                renderer_name: Some("software".into()),
                ..Default::default()
            },
        ))).unwrap();
        let app = AppWindow::new().unwrap();
        wire_video_prompt_callbacks(&app, AppContext::default());
        let state = app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_prompt("image prompt stays unchanged".into());
        let long_prompt = "主体缓慢向前行走，镜头平稳推进。\n".repeat(150);
        state.set_video_prompt(long_prompt.clone().into());
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().unwrap();

        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0), (1920.0, 1080.0)] {
            app.window().set_size(slint::LogicalSize::new(width, height));
            let optimize = ElementHandle::find_by_element_id(
                &app, "VideoGenerationPage::video-prompt-optimize",
            ).next().expect("video prompt optimize button");
            let expand = ElementHandle::find_by_element_id(
                &app, "VideoGenerationPage::video-prompt-expand",
            ).next().expect("video prompt expand button");
            assert!(optimize.absolute_position().x + optimize.size().width < expand.absolute_position().x);
            assert!((optimize.absolute_position().y - expand.absolute_position().y).abs() < 1.0);
            assert!(expand.absolute_position().x + expand.size().width < width);
        }
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        save_video_prompt_test_snapshot(&app, "video-prompt-header.png");

        let expand = ElementHandle::find_by_element_id(
            &app, "VideoGenerationPage::video-prompt-expand",
        ).next().expect("video prompt expand button");
        expand.mock_single_click(PointerEventButton::Left);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(5));
        let editor = ElementHandle::find_by_element_id(
            &app, "VideoPromptEditorDialog::expanded-input",
        ).next().expect("expanded video prompt editor");
        assert_eq!(editor.accessible_value().unwrap().as_str(), long_prompt);
        save_video_prompt_test_snapshot(&app, "video-prompt-expanded.png");
        let edited = format!("{long_prompt}\n保持画面主体与光影不变。");
        editor.set_accessible_value(edited.clone());
        assert_eq!(state.get_video_prompt().as_str(), edited);
        assert_eq!(state.get_prompt(), "image prompt stays unchanged");

        let done = ElementHandle::find_by_element_id(
            &app, "VideoPromptEditorDialog::done-button",
        ).next().expect("done button");
        done.mock_single_click(PointerEventButton::Left);
        assert!(ElementHandle::find_by_element_id(
            &app, "VideoPromptEditorDialog::expanded-input",
        ).next().is_none());
        assert_eq!(state.get_video_prompt().as_str(), edited);
        expand.mock_single_click(PointerEventButton::Left);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(5));
        app.window().dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
        app.window().dispatch_event(slint::platform::WindowEvent::KeyReleased {
            text: slint::platform::Key::Escape.into(),
        });
        assert!(!state.get_video_prompt_expanded_open());
        assert_eq!(state.get_video_prompt().as_str(), edited);

        state.set_session_state("offline".into());
        let optimize = ElementHandle::find_by_element_id(
            &app, "VideoGenerationPage::video-prompt-optimize",
        ).next().unwrap();
        optimize.mock_single_click(PointerEventButton::Left);
        assert!(!state.get_video_prompt_status().is_empty());
        assert_eq!(state.get_video_prompt().as_str(), edited);

        state.set_recovered_prompt_target_kind("video_prompt".into());
        state.set_recovered_prompt_target_id("another-image".into());
        state.set_recovered_prompt_result("retained result".into());
        state.set_recovered_prompt_result_open(true);
        let later = ElementHandle::find_by_element_id(
            &app, "RecoveredPromptResultDialog::video-result-later",
        ).next().expect("video recovery can be deferred without discarding it");
        later.mock_single_click(PointerEventButton::Left);
        assert!(!state.get_recovered_prompt_result_open());
        assert_eq!(state.get_recovered_prompt_result(), "retained result");
    }

    fn save_video_prompt_test_snapshot(app: &AppWindow, name: &str) {
        let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") else { return };
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).unwrap();
        let pixels = app.window().take_snapshot().unwrap();
        image::save_buffer(
            directory.join(name), pixels.as_bytes(), pixels.width(), pixels.height(),
            image::ColorType::Rgba8,
        ).unwrap();
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
