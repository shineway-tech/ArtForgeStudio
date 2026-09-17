use super::*;

/// Test fixture for the previous disabled release; production wires the catalogue-backed flow.
#[cfg(test)]
pub(super) fn wire_deferred_video_generation_callbacks(app: &AppWindow, context: AppContext) {
    wire_video_generation_callbacks(app, context.clone());
    let state = app.global::<AppState>();
    state.set_video_service_available(false);
    let unavailable = Rc::new({
        let weak = app.as_weak();
        let context = context.clone();
        move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let Some(persistence) = context.store.borrow().private_persistence.clone() else {
                return;
            };
            if !persistence.is_current() {
                return;
            }
            let _ = context.apply_user_completion(persistence.lease(), || {
                if !context
                    .store
                    .borrow()
                    .private_persistence
                    .as_ref()
                    .is_some_and(|current| current.same_binding_metadata(&persistence))
                {
                    return;
                }
                let state = app.global::<AppState>();
                let message = if state.get_language() == "en" {
                    "Video generation is not enabled in this release. Existing data is preserved."
                } else {
                    "视频生成本轮暂未开放，已有数据已保留"
                };
                state.set_video_status(message.into());
                state.set_viewer_message(message.into());
            });
        }
    });
    let action = unavailable.clone();
    state.on_viewer_generate_video(move || action());
    let action = unavailable.clone();
    state.on_request_video_quote(move |_, _, _| action());
    let action = unavailable.clone();
    state.on_submit_video_generation(move || action());
    state.on_select_video_model(move |_| unavailable());
    let weak = app.as_weak();
    state.on_close_video_generation(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let Some(persistence) = context.store.borrow().private_persistence.clone() else {
            return;
        };
        if !persistence.is_current() {
            return;
        }
        let closed = context
            .apply_user_completion(persistence.lease(), || {
                if !context
                    .store
                    .borrow()
                    .private_persistence
                    .as_ref()
                    .is_some_and(|current| current.same_binding_metadata(&persistence))
                {
                    return false;
                }
                let state = app.global::<AppState>();
                let page = state.get_video_return_page();
                state.set_page(if page.is_empty() {
                    "assets".into()
                } else {
                    page
                });
                state.set_video_prompt_expanded_open(false);
                state.set_video_quote_loading(false);
                state.set_viewer_open(
                    state.get_video_return_to_viewer() && !state.get_viewer_id().is_empty(),
                );
                true
            })
            .unwrap_or(false);
        if closed {
            close_video_player();
        }
    });
}

#[path = "video_pricing.rs"]
mod pricing;
use pricing::*;

pub(super) fn sync_video_resolutions(state: &AppState) {
    let model = state
        .get_catalog_models()
        .iter()
        .find(|row| row.code == state.get_video_model());
    let available = |quality: &str| {
        model
            .as_ref()
            .is_some_and(|row| !video_rate(row, quality).is_empty())
    };
    state.set_video_480_available(available("480P"));
    state.set_video_720_available(available("720P"));
    state.set_video_1080_available(available("1080P"));
    if !available(state.get_video_resolution().as_str()) {
        if let Some(quality) = ["720P", "480P", "1080P"]
            .into_iter()
            .find(|quality| available(quality))
        {
            state.set_video_resolution(quality.into());
        }
    }
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
    sync_video_resolutions(state);
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

fn apply_saved_video_selection(state: &AppState, output: &SavedVideoOutput, prompt: String) {
    if state.get_page() != "video-generation" {
        state.set_video_return_page("assets".into());
        state.set_video_return_to_viewer(false);
    }
    state.set_video_page_tab("create".into());
    state.set_video_result_path(output.source_path.clone().into());
    state.set_video_source_title(output.title.clone().into());
    state.set_video_prompt(prompt.into());
    if !output.model.is_empty() {
        state.set_video_model(output.model.clone().into());
    }
    if !output.resolution.is_empty() {
        state.set_video_resolution(output.resolution.clone().into());
    }
    if output.duration_secs > 0 {
        state.set_video_duration_seconds(output.duration_secs);
    }
    state.set_page("video-generation".into());
}

pub(super) fn wire_video_generation_callbacks(app: &AppWindow, context: AppContext) {
    wire_video_prompt_callbacks(app, context.clone());
    let state = app.global::<AppState>();
    state.on_update_video_player_visibility(set_video_player_visible);
    {
        let weak = app.as_weak();
        let context = context.clone();
        state.on_play_saved_video(move |id| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let Some(persistence) = context.store.borrow().private_persistence.clone() else {
                return;
            };
            if !persistence.is_current() {
                return;
            }
            let Some(output) = context
                .store
                .borrow()
                .video_outputs
                .get(id.as_str())
                .cloned()
            else {
                return;
            };
            let prompt=if !output.prompt.trim().is_empty() { output.prompt.clone() } else {
                video_source_asset(&context.store.borrow(),&output).map(|asset|asset.prompt.clone())
                    .filter(|prompt|!prompt.trim().is_empty()).unwrap_or_else(||output.title.clone())
            };
            let applied = context.apply_user_completion(persistence.lease(), || {
                let state = app.global::<AppState>();
                apply_saved_video_selection(&state, &output, prompt);
            });
            if applied.is_ok() {
                app.global::<AppState>().invoke_refresh_video_prices();
            }
        });
    }
    {
        let context = context.clone();
        state.on_reveal_saved_video(move |id| {
            let Some(persistence) = context.store.borrow().private_persistence.clone() else {
                return;
            };
            let Some(output) = context
                .store
                .borrow()
                .video_outputs
                .get(id.as_str())
                .cloned()
            else {
                return;
            };
            let _ = reveal_saved_video_folder(&context, persistence, output);
        });
    }
    let store = context.store.clone();
    let quote_epoch = Arc::new(AtomicU64::new(0));
    let pending_client_request_id = Arc::new(Mutex::new(String::new()));
    wire_video_price_refresh(app, context.clone());
    let image_epoch = wire_video_image_callbacks(
        app,
        store.clone(),
        quote_epoch.clone(),
        pending_client_request_id.clone(),
    );

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_video_model(move |model_code| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(persistence) = store.borrow().private_persistence.clone() else {
                return;
            };
            if !persistence.is_current() {
                return;
            }
            let state = app.global::<AppState>();
            if apply_video_model_selection(&state, model_code.as_str()) {
                save_local_store(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_sync_video_player(move |x, y, width, height| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_video_prompt_expanded_open()
                || state.get_recovered_prompt_result_open()
                || state.get_video_image_dialog() != ""
            {
                set_video_player_visible(false);
                return;
            }
            let path = PathBuf::from(state.get_video_result_path().to_string());
            if path.as_os_str().is_empty() {
                close_video_player();
                return;
            }
            let Some(persistence) = context.store.borrow().private_persistence.clone() else {
                return;
            };
            let Some(output) = context
                .store
                .borrow()
                .video_outputs
                .values()
                .find(|output| Path::new(&output.source_path) == path)
                .cloned()
            else {
                return;
            };
            let lease = persistence.lease().clone();
            if let Err(error) = sync_video_player_captured(
                &app,
                &context,
                persistence,
                &output,
                (x, y, width, height),
            ) {
                let _ = context.apply_user_completion(&lease, || {
                    state.set_video_status(format!("播放器打开失败：{error}").into())
                });
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
            let Some(persistence) = context.store.borrow().private_persistence.clone() else {
                return;
            };
            let Ok(_activity) = persistence.begin_activity() else {
                return;
            };
            if !context
                .store
                .borrow()
                .private_persistence
                .as_ref()
                .is_some_and(|current| current.same_binding(&persistence))
            {
                return;
            }
            let candidate = (
                state.get_viewer_title().to_string(),
                PathBuf::from(state.get_viewer_source_path().to_string()),
            );
            let source_id = state.get_viewer_id().to_string();
            let owner = persistence.lease().namespace.user_public_id().to_owned();
            let initialized = context
                .apply_user_completion(persistence.lease(), || {
                    if quote_epoch
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                            value.checked_add(1)
                        })
                        .is_err()
                    {
                        return false;
                    }
                    reset_video_images(&state, &image_epoch);
                    pending_client_request_id
                        .lock()
                        .unwrap_or_else(|value| value.into_inner())
                        .clear();
                    state.set_video_source_id(state.get_viewer_id());
                    state.set_video_source_path("".into());
                    state.set_video_source_file_id("".into());
                    state.set_video_source_image(slint::Image::default());
                    state.set_video_source_title(state.get_viewer_title());
                    state.set_video_prompt(
                        video_prompt_for_source(
                            &context.store.borrow().prompt_drafts,
                            &owner,
                            state.get_viewer_id().as_str(),
                            state.get_viewer_prompt().as_str(),
                        )
                        .into(),
                    );
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
                    state.set_video_return_to_viewer(true);
                    state.set_video_status(if state.get_video_service_available() {
                        "正在获取服务端报价...".into()
                    } else {
                        "视频服务暂未开放".into()
                    });
                    state.set_viewer_open(false);
                    state.set_page("video-generation".into());
                    true
                })
                .unwrap_or(false);
            if !initialized {
                return;
            }
            app.global::<AppState>().invoke_refresh_video_prices();
            close_video_player();
            start_captured_video_viewer_image_import(
                &app,
                context.store.clone(),
                persistence,
                candidate,
                source_id,
                image_epoch.clone(),
                quote_epoch.clone(),
                pending_client_request_id.clone(),
            );
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
            let Some(persistence) = store.borrow().private_persistence.clone() else {
                return;
            };
            if !persistence.is_current() {
                return;
            }
            quote_epoch.fetch_add(1, Ordering::SeqCst);
            close_video_player();
            let state = app.global::<AppState>();
            if state.get_video_quote_loading() {
                state.set_video_generating(false);
            }
            cancel_video_image_work(&state, &image_epoch);
            let return_page = state.get_video_return_page().to_string();
            let return_to_viewer = state.get_video_return_to_viewer();
            state.set_video_prompt_expanded_open(false);
            state.set_video_quote_loading(false);
            navigate_to_with_store(&app, &store.borrow(), &return_page);
            state.set_viewer_open(return_to_viewer && !state.get_viewer_id().is_empty());
        });
    }

    {
        let weak = app.as_weak();
        let context = context.clone();
        let epoch = quote_epoch.clone();
        let pending = pending_client_request_id.clone();
        state.on_request_video_quote(move |ratio, resolution, duration| {
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
            if !persistence.is_current() {
                return;
            }
            epoch.fetch_add(1, Ordering::SeqCst);
            pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
            state.set_video_aspect_ratio(ratio);
            state.set_video_resolution(resolution);
            state.set_video_duration_seconds(duration);
            estimate_video_price(&state);
        });
    }
    wire_video_submission(app, context, quote_epoch, pending_client_request_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured_video_fixture() -> (
        video_image_callbacks::tests::scoped_inputs::Fixture,
        tempfile::TempDir,
        PathBuf,
    ) {
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let source_root = tempfile::tempdir().unwrap();
        let path = source_root.path().join("source.png");
        image::RgbaImage::from_pixel(80, 120, image::Rgba([40, 90, 120, 255]))
            .save(&path)
            .unwrap();
        (fixture, source_root, path)
    }

    #[test]
    fn video_resolution_options_follow_catalogue_prices() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        let mut model = catalog_model("seedance_2_5_pro", "Seedance 2.5 Pro", "video_generation");
        model.video_price_720 = "200".into();
        model.video_price_1080 = "450".into();
        state.set_catalog_models(ModelRc::new(VecModel::from(vec![model])));
        state.set_video_model("seedance_2_5_pro".into());
        state.set_video_resolution("480P".into());
        sync_video_resolutions(&state);
        assert!(!state.get_video_480_available());
        assert!(state.get_video_720_available());
        assert!(state.get_video_1080_available());
        assert_eq!(state.get_video_resolution(), "720P");
    }

    #[test]
    fn direct_video_navigation_returns_to_workspace_without_opening_empty_viewer() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        wire_video_generation_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_page("generation".into());
        state.set_video_page_tab("history".into());
        state.set_viewer_id("stale-viewer".into());
        state.set_viewer_open(true);

        assert!(crate::runtime::app::prepare_direct_video_navigation(&state));
        state.set_page("video-generation".into());
        assert_eq!(state.get_video_page_tab(), "create");
        assert_eq!(state.get_video_return_page(), "generation");
        assert!(!state.get_video_return_to_viewer());
        assert!(!state.get_viewer_open());

        state.invoke_close_video_generation();

        assert_eq!(state.get_page(), "generation");
        assert!(!state.get_viewer_open());
        fixture.drain();
    }

    #[test]
    fn saved_video_selection_opens_the_player_tab_and_preserves_workspace_return() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_video_page_tab("history".into());
        state.set_video_return_page("generation".into());
        let output = SavedVideoOutput {
            model: "seedance-2.0-fast".into(),
            resolution: "1080P".into(),
            duration_secs: 8,
            source_asset_id: "source".into(),
            prompt: "saved prompt".into(),
            client_request_id: "request".into(),
            server_task_id: "task".into(),
            file_id: "file".into(),
            billing_account_group_id: "group".into(),
            sha256: "0".repeat(64),
            size_bytes: 1,
            source_path: "C:/videos/history.mp4".into(),
            title: "历史视频".into(),
            created_at: "2026-09-17T12:30:00+08:00".into(),
        };

        apply_saved_video_selection(&state, &output, output.prompt.clone());

        assert_eq!(state.get_video_page_tab(), "create");
        assert_eq!(state.get_page(), "video-generation");
        assert_eq!(state.get_video_return_page(), "generation");
        assert_eq!(state.get_video_result_path(), output.source_path);
        assert_eq!(state.get_video_source_title(), output.title);
        assert_eq!(state.get_video_prompt(), output.prompt);
        assert_eq!(state.get_video_model(), output.model);
        assert_eq!(state.get_video_resolution(), output.resolution);
        assert_eq!(state.get_video_duration_seconds(), 8);
    }

    #[test]
    fn core_video_viewer_open_captures_owned_source_and_exact_upgrade_keeps_projection() {
        i_slint_backend_testing::init_no_event_loop();
        let (fixture, _source_root, path) = captured_video_fixture();
        let app = AppWindow::new().unwrap();
        wire_video_generation_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_page("assets".into());
        state.set_viewer_id("source-asset".into());
        state.set_viewer_source_path(path.to_string_lossy().into_owned().into());
        state.set_viewer_title("Original source".into());
        state.set_viewer_open(true);
        state.invoke_viewer_generate_video();
        video_image_callbacks::tests::scoped_inputs::pump(|| !state.get_video_images_loading());
        assert_eq!(state.get_video_images().row_count(), 1);
        assert_eq!(state.get_page(), "video-generation");
        assert!(state.get_video_return_to_viewer());
        assert!(!state.get_viewer_open());
        let owned = PathBuf::from(state.get_video_source_path().to_string());
        assert_ne!(owned, path);
        assert!(fixture.persistence.owns_path(&owned));
        state.invoke_close_video_generation();
        assert_eq!(state.get_page(), "assets");
        assert!(state.get_viewer_open());
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade {
            minimum_version: None,
        });
        state.set_page("upgrade-boundary".into());
        state.set_video_source_title("unchanged".into());
        state.invoke_viewer_generate_video();
        assert_eq!(state.get_page(), "upgrade-boundary");
        assert_eq!(state.get_video_source_title(), "unchanged");
        fixture.drain();
    }

    #[test]
    fn video_workspace_starts_with_a_thumbnail_and_offers_both_image_sources() {
        use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
        use slint::platform::PointerEventButton;

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        })))
        .unwrap();
        let app = AppWindow::new().unwrap();
        let (fixture, _source_root, path) = captured_video_fixture();
        wire_video_generation_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_session_state("offline".into());
        state.set_page("assets".into());
        state.set_viewer_id("source".into());
        state.set_viewer_source_path(path.to_string_lossy().into_owned().into());
        state.set_viewer_title("Source image".into());
        state.set_viewer_prompt("Keep this prompt".into());
        state.invoke_viewer_generate_video();
        video_image_callbacks::tests::scoped_inputs::pump(|| !state.get_video_images_loading());
        assert_eq!(state.get_page(), "video-generation");
        assert_eq!(state.get_video_images().row_count(), 1);
        app.show().unwrap();

        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0), (1920.0, 1080.0)] {
            app.window()
                .set_size(slint::LogicalSize::new(width, height));
            let cards: Vec<_> =
                ElementHandle::find_by_element_type_name(&app, "VideoImageCard").collect();
            assert_eq!(
                cards.len(),
                1,
                "the original image must appear as one thumbnail"
            );
            assert!(
                cards[0].size().width <= 240.0 && cards[0].size().height <= 280.0,
                "a source image must not grow into a full-height preview"
            );
            let add = ElementHandle::find_by_element_id(&app, "VideoImageGrid::add-image")
                .next()
                .expect("add tile beside the image");
            assert!(
                add.absolute_position().x >= cards[0].absolute_position().x + cards[0].size().width
            );
        }
        ElementHandle::find_by_element_id(&app, "VideoImageGrid::add-image")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert!(
            ElementHandle::find_by_accessible_label(&app, "从我的资产添加")
                .next()
                .is_some()
        );
        assert!(ElementHandle::find_by_accessible_label(&app, "本地上传")
            .next()
            .is_some());
        assert_eq!(state.get_video_prompt(), "Keep this prompt");
    }

    #[test]
    fn video_workspace_tabs_open_history_and_history_cards_open_the_player_view() {
        use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
        use slint::platform::PointerEventButton;

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        })))
        .unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_page("video-generation".into());
        state.set_saved_videos(ModelRc::new(VecModel::from(vec![VideoImageItem {
            id: "video-history-id".into(),
            title: "历史视频".into(),
            subtitle: "2026-09-17 12:30 · 1080P · 8s".into(),
            ..Default::default()
        }])));
        let visibility = Rc::new(RefCell::new(Vec::new()));
        {
            let visibility = visibility.clone();
            state.on_update_video_player_visibility(move |visible| visibility.borrow_mut().push(visible));
        }
        let opened = Rc::new(RefCell::new(String::new()));
        {
            let opened = opened.clone();
            let weak = app.as_weak();
            state.on_play_saved_video(move |id| {
                *opened.borrow_mut() = id.to_string();
                if let Some(app) = weak.upgrade() {
                    app.global::<AppState>().set_video_page_tab("create".into());
                }
            });
        }
        app.show().unwrap();

        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0)] {
            app.window().set_size(slint::LogicalSize::new(width, height));
            let tabs = ElementHandle::find_by_element_id(&app, "VideoGenerationPage::video-page-tabs")
                .next()
                .expect("video page tabs");
            assert!(tabs.absolute_position().x >= 0.0);
            assert!(tabs.absolute_position().x + tabs.size().width <= width);
        }
        save_video_prompt_test_snapshot(&app, "video-create-tab.png");

        ElementHandle::find_by_accessible_label(&app, "历史记录")
            .next()
            .expect("history tab")
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(state.get_video_page_tab(), "history");
        assert_eq!(visibility.borrow().as_slice(), &[false]);
        save_video_prompt_test_snapshot(&app, "video-history-tab.png");

        ElementHandle::find_by_accessible_label(&app, "播放 历史视频")
            .next()
            .expect("saved video card")
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(opened.borrow().as_str(), "video-history-id");
        assert_eq!(state.get_video_page_tab(), "create");

        ElementHandle::find_by_accessible_label(&app, "历史记录")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        ElementHandle::find_by_accessible_label(&app, "创作")
            .next()
            .expect("create tab")
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(state.get_video_page_tab(), "create");
        assert_eq!(visibility.borrow().as_slice(), &[false, false, true]);
    }

    #[test]
    fn viewer_video_action_preserves_workspace_and_inputs_while_service_is_deferred() {
        use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
        use slint::platform::PointerEventButton;

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        })))
        .unwrap();
        let app = AppWindow::new().unwrap();
        let (fixture, _source_root, path) = captured_video_fixture();
        // Exercise the production wrapper, not the retained unoffered implementation.
        wire_deferred_video_generation_callbacks(&app, fixture.context.clone());
        let source_bytes = fs::read(&path).unwrap();
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_session_state("offline".into());
        app.show().unwrap();

        for (origin, collapsed, width, height) in [
            ("generation", false, 1180.0, 760.0),
            ("assets", true, 1440.0, 900.0),
            ("generation", true, 1920.0, 1080.0),
            ("assets", false, 1440.0, 900.0),
        ] {
            app.window()
                .set_size(slint::LogicalSize::new(width, height));
            state.set_page(origin.into());
            state.set_sidebar_collapsed(collapsed);
            state.set_viewer_source(origin.into());
            state.set_viewer_id("video-source-image".into());
            state.set_viewer_source_path(path.to_string_lossy().into_owned().into());
            state.set_viewer_title("Source image".into());
            state.set_viewer_prompt("Keep the original scene and move the camera slowly.".into());
            state.set_viewer_open(true);
            state.set_viewer_message("before video request".into());
            state.set_video_status("before video request".into());
            state.set_video_prompt("Retained video draft".into());
            state.set_video_source_id("retained-source".into());
            state.set_video_result_path("retained-video-result".into());
            state.set_video_images(ModelRc::new(VecModel::from(vec![VideoImageItem {
                id: "retained-input".into(),
                source_asset_id: "retained-source".into(),
                source_path: path.to_string_lossy().into_owned().into(),
                ..Default::default()
            }])));
            let sidebar = ElementHandle::find_by_element_type_name(&app, "Sidebar")
                .next()
                .expect("original navigation remains available");
            // Read settled navigation width, not its 160ms collapse animation.
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(200));
            let sidebar_width = sidebar.size().width;

            ElementHandle::find_by_element_type_name(&app, "ViewerFooterActionButton")
                .nth(1)
                .expect("generate video action in image details")
                .mock_single_click(PointerEventButton::Left);
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(200));

            assert_eq!(state.get_page().as_str(), origin);
            assert!(
                state.get_viewer_open(),
                "deferred entry keeps original image details"
            );
            assert_eq!(state.get_viewer_id(), "video-source-image");
            assert_eq!(state.get_viewer_source().as_str(), origin);
            assert_eq!(
                state.get_viewer_source_path().as_str(),
                path.to_str().unwrap()
            );
            assert_eq!(
                state.get_viewer_prompt(),
                "Keep the original scene and move the camera slowly."
            );
            assert_eq!(state.get_sidebar_collapsed(), collapsed);
            let current_sidebar = ElementHandle::find_by_element_type_name(&app, "Sidebar")
                .next()
                .expect("deferred entry must not hide navigation");
            assert_eq!(current_sidebar.size().width, sidebar_width);
            assert!(
                ElementHandle::find_by_element_type_name(&app, "VideoGenerationPage")
                    .next()
                    .is_none()
            );
            assert!(!state.get_video_service_available());
            assert!(!state.get_video_generating());
            assert!(!state.get_video_images_loading());
            assert!(!state.get_video_quote_loading());
            assert_eq!(
                state.get_video_status(),
                "视频生成本轮暂未开放，已有数据已保留"
            );
            assert_eq!(
                state.get_viewer_message(),
                "视频生成本轮暂未开放，已有数据已保留"
            );
            assert_eq!(state.get_video_prompt(), "Retained video draft");
            assert_eq!(state.get_video_source_id(), "retained-source");
            assert_eq!(state.get_video_result_path(), "retained-video-result");
            assert_eq!(state.get_video_images().row_count(), 1);
            let input = state.get_video_images().row_data(0).unwrap();
            assert_eq!(input.id, "retained-input");
            assert_eq!(input.source_asset_id, "retained-source");
            assert_eq!(input.source_path.as_str(), path.to_str().unwrap());
            assert_eq!(fs::read(&path).unwrap(), source_bytes);
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
            video_price_480: String::new().into(),
            video_price_720: String::new().into(),
            video_price_1080: String::new().into(),
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
        )))
        .unwrap();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        struct VideoPromptLayoutDrain<'a>(&'a video_image_callbacks::tests::scoped_inputs::Fixture);
        impl Drop for VideoPromptLayoutDrain<'_> {
            fn drop(&mut self) {
                let prompt = shutdown_prompt_workers();
                let delivery =
                    drain_delivery_commit_workers_for_lease_for_test(self.0.persistence.lease());
                let previews =
                    drain_activation_preview_workers_for_lease_for_test(self.0.persistence.lease());
                let retired = self
                    .0
                    .context
                    .user_activity
                    .begin_quiesce(self.0.persistence.lease())
                    .map(|guard| guard.retire());
                if !std::thread::panicking() {
                    prompt.unwrap();
                    delivery.unwrap();
                    previews.unwrap();
                    retired.unwrap();
                }
            }
        }
        let _drain = VideoPromptLayoutDrain(&fixture);
        let app = AppWindow::new().unwrap();
        wire_video_prompt_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_prompt("image prompt stays unchanged".into());
        let long_prompt = "主体缓慢向前行走，镜头平稳推进。\n".repeat(150);
        state.set_video_prompt(long_prompt.clone().into());
        app.window()
            .set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().unwrap();

        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0), (1920.0, 1080.0)] {
            app.window()
                .set_size(slint::LogicalSize::new(width, height));
            let optimize = ElementHandle::find_by_element_id(
                &app,
                "VideoGenerationPage::video-prompt-optimize",
            )
            .next()
            .expect("video prompt optimize button");
            let expand =
                ElementHandle::find_by_element_id(&app, "VideoGenerationPage::video-prompt-expand")
                    .next()
                    .expect("video prompt expand button");
            assert!(
                optimize.absolute_position().x + optimize.size().width
                    < expand.absolute_position().x
            );
            assert!((optimize.absolute_position().y - expand.absolute_position().y).abs() < 1.0);
            assert!(expand.absolute_position().x + expand.size().width < width);
        }
        app.window()
            .set_size(slint::LogicalSize::new(1440.0, 900.0));
        save_video_prompt_test_snapshot(&app, "video-prompt-header.png");

        let expand =
            ElementHandle::find_by_element_id(&app, "VideoGenerationPage::video-prompt-expand")
                .next()
                .expect("video prompt expand button");
        expand.mock_single_click(PointerEventButton::Left);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(5));
        let editor =
            ElementHandle::find_by_element_id(&app, "VideoPromptEditorDialog::expanded-input")
                .next()
                .expect("expanded video prompt editor");
        assert_eq!(editor.accessible_value().unwrap().as_str(), long_prompt);
        save_video_prompt_test_snapshot(&app, "video-prompt-expanded.png");
        let edited = format!("{long_prompt}\n保持画面主体与光影不变。");
        editor.set_accessible_value(edited.clone());
        assert_eq!(state.get_video_prompt().as_str(), edited);
        assert_eq!(state.get_prompt(), "image prompt stays unchanged");

        let done = ElementHandle::find_by_element_id(&app, "VideoPromptEditorDialog::done-button")
            .next()
            .expect("done button");
        done.mock_single_click(PointerEventButton::Left);
        assert!(
            ElementHandle::find_by_element_id(&app, "VideoPromptEditorDialog::expanded-input",)
                .next()
                .is_none()
        );
        assert_eq!(state.get_video_prompt().as_str(), edited);
        expand.mock_single_click(PointerEventButton::Left);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(5));
        app.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        app.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!state.get_video_prompt_expanded_open());
        assert_eq!(state.get_video_prompt().as_str(), edited);

        state.set_session_state("offline".into());
        let optimize =
            ElementHandle::find_by_element_id(&app, "VideoGenerationPage::video-prompt-optimize")
                .next()
                .unwrap();
        optimize.mock_single_click(PointerEventButton::Left);
        assert!(!state.get_video_prompt_status().is_empty());
        assert_eq!(state.get_video_prompt().as_str(), edited);

        state.set_recovered_prompt_target_kind("video_prompt".into());
        state.set_recovered_prompt_target_id("another-image".into());
        state.set_recovered_prompt_result("retained result".into());
        state.set_recovered_prompt_result_open(true);
        let later = ElementHandle::find_by_element_id(
            &app,
            "RecoveredPromptResultDialog::video-result-later",
        )
        .next()
        .expect("video recovery can be deferred without discarding it");
        later.mock_single_click(PointerEventButton::Left);
        assert!(!state.get_recovered_prompt_result_open());
        assert_eq!(state.get_recovered_prompt_result(), "retained result");
    }

    fn save_video_prompt_test_snapshot(app: &AppWindow, name: &str) {
        let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).unwrap();
        let pixels = app.window().take_snapshot().unwrap();
        image::save_buffer(
            directory.join(name),
            pixels.as_bytes(),
            pixels.width(),
            pixels.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
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

    #[test]
    fn core_video_production_deferred_entry_preserves_existing_inputs_and_retained_records() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        wire_deferred_video_generation_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_page("assets".into());
        state.set_viewer_open(true);
        state.set_video_prompt("saved draft".into());
        state.set_video_result_path("retained-video-result".into());
        state.set_video_images(ModelRc::new(VecModel::from(vec![VideoImageItem {
            id: "old-input".into(),
            source_asset_id: "source".into(),
            source_path: "retained-input".into(),
            ..Default::default()
        }])));
        let scope = BillingScope {
            request: GroupRequestScope {
                session: fixture.context.current_account_session_scope().unwrap(),
                account_group_id: "22222222-2222-4222-8222-222222222222".into(),
            },
            context_epoch: 1,
        };
        let mut row = backend_generation::billing_capture_test_support::generation_record(
            &scope,
            "image_to_video",
        );
        row.uploaded_file_ids.clear();
        row.video_request = Some(CreateVideoGenerationTask {
            client_request_id: row.client_request_id.clone(),
            task_type: "image_to_video".into(),
            model_code: "retained-model".into(),
            prompt: "saved draft".into(),
            source_file_id: "55555555-5555-4555-8555-555555555555".into(),
            reference_file_ids: vec!["55555555-5555-4555-8555-555555555555".into()],
            aspect_ratio: "16:9".into(),
            resolution: "720P".into(),
            duration_secs: 4,
            quote_id: "original-quote".into(),
        });
        upsert_pending_generation_for_namespace(&fixture.authority, &scope, row).unwrap();
        let before = serde_json::to_value(
            load_pending_generations_for_namespace(&fixture.authority).unwrap(),
        )
        .unwrap();
        let store_before =
            serde_json::to_value(local_store_data(&app, &fixture.context.store.borrow())).unwrap();
        state.invoke_viewer_generate_video();
        state.invoke_request_video_quote("9:16".into(), "1080P".into(), 8);
        state.invoke_submit_video_generation();
        assert!(!state.get_video_service_available());
        assert_eq!(state.get_page(), "assets");
        assert!(state.get_viewer_open());
        assert_eq!(state.get_video_prompt(), "saved draft");
        assert_eq!(state.get_video_result_path(), "retained-video-result");
        assert_eq!(state.get_video_images().row_count(), 1);
        assert_eq!(
            state.get_video_images().row_data(0).unwrap().id,
            "old-input"
        );
        assert_eq!(
            state.get_video_status(),
            "视频生成本轮暂未开放，已有数据已保留"
        );
        assert_eq!(
            serde_json::to_value(local_store_data(&app, &fixture.context.store.borrow())).unwrap(),
            store_before
        );
        assert_eq!(
            serde_json::to_value(
                load_pending_generations_for_namespace(&fixture.authority).unwrap()
            )
            .unwrap(),
            before
        );
        fixture.drain();
    }
    #[test]
    fn core_video_quote_and_submit_refuse_missing_store_without_private_projection_changes() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        wire_deferred_video_generation_callbacks(&app, fixture.context.clone());
        fixture.context.store.borrow_mut().private_persistence = None;
        let state = app.global::<AppState>();
        state.set_video_status("unchanged".into());
        state.set_video_quote_ready(true);
        state.set_video_quote_id("retained-original-quote".into());
        state.set_video_credit_cost("12".into());
        state.invoke_request_video_quote("16:9".into(), "720P".into(), 4);
        assert_eq!(state.get_video_status(), "unchanged");
        assert!(state.get_video_quote_ready());
        assert_eq!(state.get_video_quote_id(), "retained-original-quote");
        state.invoke_submit_video_generation();
        assert_eq!(state.get_video_status(), "unchanged");
        assert!(!state.get_video_generating());
        fixture.drain();
    }
    #[test]
    fn core_video_ordinary_model_navigation_quote_and_submit_refuse_exact_upgrade() {
        i_slint_backend_testing::init_no_event_loop();
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        let app = AppWindow::new().unwrap();
        wire_deferred_video_generation_callbacks(&app, fixture.context.clone());
        let state = app.global::<AppState>();
        state.set_catalog_models(ModelRc::new(VecModel::from(vec![
            catalog_model("model-old", "Old", "video_generation"),
            catalog_model("model-new", "New", "video_generation"),
        ])));
        state.set_video_model("model-old".into());
        state.set_page("video-generation".into());
        state.set_video_return_page("assets".into());
        state.set_video_status("upgrade-boundary".into());
        state.set_video_quote_ready(true);
        state.set_video_quote_id("retained-original-quote".into());
        fixture.persistence.upgrade_latch().trip(RequiredUpgrade {
            minimum_version: None,
        });
        state.invoke_select_video_model("model-new".into());
        assert_eq!(state.get_video_model(), "model-old");
        state.invoke_request_video_quote("9:16".into(), "1080P".into(), 8);
        state.invoke_submit_video_generation();
        assert_eq!(state.get_video_status(), "upgrade-boundary");
        assert!(state.get_video_quote_ready());
        assert_eq!(state.get_video_quote_id(), "retained-original-quote");
        state.invoke_close_video_generation();
        assert_eq!(state.get_page(), "video-generation");
        fixture.drain();
    }
}
