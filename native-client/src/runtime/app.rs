use super::*;
use crate::platform;

pub(super) fn run() -> Result<()> {
    // The local Store, SQLite index and preview cache are shared process resources. Keeping a
    // second process out avoids cross-process reference rebuilds and cache/write races.
    let instance_name = native_client_instance_name();
    let instance = single_instance::SingleInstance::new(&instance_name)
        .context("无法创建客户端单实例锁")?;
    if !instance.is_single() {
        anyhow::bail!("客户端已在运行");
    }
    let _instance_guard = instance;
    let app = AppWindow::new()?;
    configure_rendering_profile(&app);
    register_external_image_drop_wakeup(&app);
    platform::schedule_application_icon_install();
    schedule_external_image_drop_install(app.as_weak(), 20);
    app.window().set_size(slint::PhysicalSize::new(1440, 900));
    init_version_state(&app);
    cleanup_stale_update_dirs();
    apply_theme(&app, "light");
    init_portable_dirs(&app)?;
    initialize_client_state_repository().context("无法初始化本地元数据库")?;
    initialize_storage_index();
    initialize_preview_cache();
    cleanup_stale_reference_imports();
    cleanup_stale_toolbox_files();
    load_user_profile(&app);
    load_showcase_images(&app);

    let context = AppContext {
        backend: Some(Arc::new(BackendRuntime::new(&app_data_dir())?)),
        ..AppContext::default()
    };
    let store = context.store.clone();
    let local_store_loaded = load_local_store(&app, &store);
    seed_inspiration(&app, &store)?;
    let reference_index_healthy = rebuild_storage_references(&store.borrow());
    if local_store_loaded && reference_index_healthy {
        cleanup_orphaned_durable_copies_at_startup();
    }
    push_startup_state(&app, &store.borrow());

    wire_callbacks(&app, context.clone());
    begin_update_check(&app, false);
    initialize_auth(&app, context.clone());
    app.run()?;
    store_current_prompt_draft(
        &app,
        &store,
        &resolve_category(&app.global::<AppState>().get_asset_type().to_string(), ""),
    );
    let _ = save_user_profile_checked(&app);
    if local_store_loaded && save_local_store_checked(&app, &store.borrow()).is_ok() {
        rebuild_storage_references(&store.borrow());
        cleanup_orphaned_durable_copies_at_shutdown();
    } else {
        // Still persist a recovered/new store when possible, but keep durable reference and
        // canvas copies for the whole session if startup could not prove the JSON was healthy.
        let _ = save_local_store_checked(&app, &store.borrow());
    }
    Ok(())
}

fn native_client_instance_name() -> String {
    #[cfg(target_os = "macos")]
    {
        // single-instance uses the supplied string as a lock-file path on macOS. The per-user
        // temporary directory is writable for Finder launches and isolates different users.
        return std::env::temp_dir()
            .join("elunvi-canvas-native-client.lock")
            .display()
            .to_string();
    }
    #[cfg(not(target_os = "macos"))]
    {
        "elunvi-canvas-native-client".to_string()
    }
}

fn schedule_external_image_drop_install(app_weak: Weak<AppWindow>, attempts_left: u8) {
    let delay = if attempts_left == 20 {
        Duration::ZERO
    } else {
        Duration::from_millis(50)
    };
    slint::Timer::single_shot(delay, move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if platform::install_external_image_drop_target(app.window()) {
            return;
        }
        if attempts_left > 1 {
            schedule_external_image_drop_install(app.as_weak(), attempts_left - 1);
        } else {
            eprintln!("failed to install native external image drop target");
        }
    });
}

fn configure_rendering_profile(app: &AppWindow) {
    // Installing a rendering notifier makes GPU backends draw on every display-link tick.
    // Keep normal rendering demand-driven and enable reduced motion when software rendering
    // is explicitly requested for GPU-less or compatibility-mode machines.
    let using_software_renderer = std::env::var("SLINT_BACKEND")
        .map(|backend| {
            let backend = backend.to_ascii_lowercase();
            backend.contains("software") || backend == "sw" || backend.ends_with("-sw")
        })
        .unwrap_or(false);
    app.global::<AppState>()
        .set_reduced_motion(using_software_renderer);
}

fn register_external_image_drop_wakeup(app: &AppWindow) {
    let app_weak = app.as_weak();
    platform::set_external_image_drop_wakeup(move || {
        let _ = app_weak.upgrade_in_event_loop(|app| {
            app.global::<AppState>()
                .invoke_process_external_image_drops();
        });
    });
}

pub(super) fn wire_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();

    wire_auth_callbacks(app, context.clone());
    wire_wechat_binding_callbacks(app, context.clone());
    wire_email_binding_callbacks(app, context.clone());
    wire_invitation_code_callbacks(app, context.clone());
    wire_payment_callbacks(app, context.clone());
    wire_credit_callbacks(app, context.clone());
    wire_custom_prompt_callbacks(app, context.clone());
    wire_prompt_optimization_callbacks(app, context.clone());
    wire_infinite_canvas_callbacks(app, context.clone());
    wire_toolbox_callbacks(app, context.clone());
    wire_image_enhancement_callbacks(app, context.clone());
    wire_image_cutout_callbacks(app, context.clone());
    wire_contact_callbacks(app, store.clone());
    wire_external_link_callbacks(app);

    {
        let app_weak = app.as_weak();
        let auth_context = context.clone();
        let auth_backend = context.backend.clone();
        let store = store.clone();
        state.on_use_now(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_logged_in() {
                navigate_to_with_store(&app, &store.borrow(), "generation");
            } else {
                state.set_auth_open(true);
                if state.get_auth_method().as_str() == "wechat"
                    && !state.get_auth_wechat_busy()
                    && !state.get_auth_wechat_qr_ready()
                {
                    if let Some(backend) = auth_backend.clone() {
                        begin_wechat_login(&app, auth_context.clone(), backend);
                    }
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_save_profile(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let name = state.get_profile_name().trim().to_string();
                state.set_nickname(name.into());
                state.set_profile_open(false);
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let context = context.clone();
        state.on_navigate(move |page| {
            if let Some(app) = app_weak.upgrade() {
                navigate_to_with_store(&app, &store.borrow(), &page);
                if page.as_str() == "credits"
                    && app.global::<AppState>().get_session_state().as_str() == "online"
                {
                    refresh_backend_snapshot(&app, context.clone());
                }
                if page.as_str() == "notifications"
                    && app.global::<AppState>().get_session_state().as_str() == "online"
                {
                    refresh_server_notifications(&app, context.clone());
                }
                if page.as_str() == "settings" {
                    refresh_storage_usage_async(&app);
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        let store = store.clone();
        state.on_back(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let page = state.get_page().to_string();
                if page == "custom-prompt-editor" {
                    close_custom_prompt_editor(&app, &context);
                    return;
                }
                if page.starts_with("toolbox-") {
                    navigate_to_with_store(&app, &store.borrow(), "toolbox");
                    return;
                }
                if page == "generation" {
                    return;
                }
                navigate_to_with_store(&app, &store.borrow(), "generation");
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_theme(move |theme| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_theme_id(theme.clone());
                apply_theme(&app, &theme);
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_card_style(move |style| {
            if let Some(app) = app_weak.upgrade() {
                let style = if style.as_str() == "square" {
                    "square"
                } else {
                    "rounded"
                };
                app.global::<AppState>().set_card_style(style.into());
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_language(move |lang| {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().set_language(lang);
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_save_gallery_layout(move |scope, layout| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let layout = normalize_gallery_layout(&layout).into();
            let state = app.global::<AppState>();
            match scope.as_str() {
                "generation" => state.set_generation_gallery_layout(layout),
                "assets" => state.set_asset_gallery_layout(layout),
                "inspiration" => state.set_inspiration_gallery_layout(layout),
                _ => return,
            }
            save_user_profile(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let context = context.clone();
        state.on_select_workspace_category(move |category| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let category = resolve_category(&category.to_string(), "");
            let state = app.global::<AppState>();
            let previous_category = resolve_category(&state.get_asset_type().to_string(), "");
            if previous_category != category {
                store_current_prompt_draft(&app, &store, &previous_category);
                state.set_creation_mode("free".into());
                state.set_style_mode("free".into());
                state.set_view_mode("free".into());
                state.set_weather_mode("natural".into());
                state.set_time_mode("natural".into());
                state.set_light_mode("natural".into());
                state.set_advanced_preview_open(false);
                state.set_advanced_prompt_preview("".into());
            }
            state.set_asset_type(category.clone().into());
            state.set_mode("game".into());
            if previous_category != category {
                let prompt = prompt_draft_for_category(&store.borrow().prompt_drafts, &category);
                state.set_prompt(prompt.into());
                let negative_prompt =
                    negative_prompt_draft_for_category(&store.borrow().prompt_drafts, &category);
                state.set_negative_prompt(negative_prompt.into());
                state.set_negative_prompt_expanded(false);
                sync_deep_prompt_binding_for_category(&app, &store.borrow(), &category);
            }
            push_custom_prompts(&app, &store.borrow());
            push_references(&app, &store.borrow());
            save_local_store(&app, &store.borrow());
            save_user_profile(&app);
            reset_generation_gallery_page(&app);
            push_generations(&app, &store.borrow());
            sync_generation_state_for_current_category(&context, &app);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_pick_dir(move |kind| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                let text = path.display().to_string();
                let state = app.global::<AppState>();
                match kind.as_str() {
                    "input" => state.set_input_dir(text.into()),
                    "output" => state.set_output_dir(text.into()),
                    "prompt" => state.set_prompt_dir(text.into()),
                    _ => {}
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_load_fonts(move || {
            if let Some(app) = app_weak.upgrade() {
                let fonts = load_system_fonts()
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>();
                app.global::<AppState>()
                    .set_font_list(ModelRc::new(VecModel::from(fonts)));
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_check_version(move || {
            if let Some(app) = app_weak.upgrade() {
                begin_update_check(&app, true);
            }
        });
    }

    let update_cancellation = new_update_cancellation();
    {
        let app_weak = app.as_weak();
        let update_cancellation = update_cancellation.clone();
        state.on_start_update(move || {
            if let Some(app) = app_weak.upgrade() {
                begin_automatic_update(&app, update_cancellation.clone());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_cancel_update(move || {
            if let Some(app) = app_weak.upgrade() {
                cancel_automatic_update(&app, &update_cancellation);
            }
        });
    }

    wire_model_catalog_callbacks(app, store.clone());
    wire_storage_callbacks(app);
    wire_reference_callbacks(app, store.clone());
    wire_prompt_preview_callbacks(app);
    wire_generation_callbacks(app, context.clone());
    wire_prompt_task_recovery_callbacks(app, context.clone());
    wire_viewer_callbacks(app, context.clone());
    wire_video_generation_callbacks(app, context.clone());
    wire_notification_callbacks(app, context);
}

pub(super) fn wire_prompt_preview_callbacks(app: &AppWindow) {
    let state = app.global::<AppState>();
    let app_weak = app.as_weak();
    state.on_refresh_advanced_prompt_preview(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        refresh_advanced_prompt_preview(&app);
    });
    refresh_advanced_prompt_preview(app);
}
