use super::*;
use crate::platform;

pub(super) fn run() -> Result<()> {
    configure_renderer_backend();
    let app = AppWindow::new()?;
    platform::schedule_application_icon_install();
    app.window().set_size(slint::PhysicalSize::new(1440, 900));
    init_version_state(&app);
    apply_theme(&app, "light");
    init_portable_dirs(&app)?;
    load_user_profile(&app);
    load_showcase_images(&app);

    let context = AppContext {
        backend: Some(Arc::new(BackendRuntime::new(&app_data_dir())?)),
        ..AppContext::default()
    };
    let store = context.store.clone();
    load_local_store(&app, &store);
    seed_inspiration(&app, &store)?;
    push_all(&app, &store.borrow());

    wire_callbacks(&app, context.clone());
    begin_update_check(&app, false);
    initialize_auth(&app, context.clone());
    app.run()?;
    store_current_prompt_draft(
        &app,
        &store,
        &resolve_category(&app.global::<AppState>().get_asset_type().to_string(), ""),
    );
    save_user_profile(&app);
    save_local_store(&app, &store.borrow());
    Ok(())
}

fn configure_renderer_backend() {
    if std::env::var_os("SLINT_BACKEND").is_some() {
        return;
    }
    #[cfg(windows)]
    std::env::set_var("SLINT_BACKEND", "winit-femtovg");
    #[cfg(not(windows))]
    std::env::set_var("SLINT_BACKEND", "winit-software");
}

pub(super) fn wire_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();

    wire_auth_callbacks(app, context.clone());
    wire_wechat_binding_callbacks(app, context.clone());
    wire_email_binding_callbacks(app, context.clone());
    wire_payment_callbacks(app, context.clone());
    wire_credit_callbacks(app, context.clone());
    wire_custom_prompt_callbacks(app, store.clone());
    wire_infinite_canvas_callbacks(app, store.clone());

    {
        let app_weak = app.as_weak();
        let auth_context = context.clone();
        let auth_backend = context.backend.clone();
        state.on_use_now(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_logged_in() {
                navigate_to(&app, "generation");
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
                if page.as_str() == "credits" && app.global::<AppState>().get_session_state().as_str() == "online" {
                    refresh_backend_snapshot(&app, context.clone());
                }
                if page.as_str() == "notifications" && app.global::<AppState>().get_session_state().as_str() == "online" {
                    refresh_server_notifications(&app, context.clone());
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_back(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let page = state.get_page().to_string();
                if page == "generation" {
                    return;
                }
                state.set_page("generation".into());
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
                let style = if style.as_str() == "square" { "square" } else { "rounded" };
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
            if category == "action-sequence" {
                state.set_creation_mode("anim-idle".into());
                state.set_count(1);
                state.set_ratio_more_open(false);
                if !action_sequence_ratio_allowed(&state.get_ratio().to_string()) {
                    state.set_ratio("1:1".into());
                }
                let mut store_mut = store.borrow_mut();
                references_for_category_mut(&mut store_mut.references, &category)
                    .truncate(max_reference_images_for_category(&category));
            }
            state.set_asset_type(category.clone().into());
            state.set_mode("game".into());
            if previous_category != category {
                let prompt = prompt_draft_for_category(&store.borrow().prompt_drafts, &category);
                state.set_prompt(prompt.into());
            }
            push_references(&app, &store.borrow());
            save_local_store(&app, &store.borrow());
            save_user_profile(&app);
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

    {
        let app_weak = app.as_weak();
        state.on_start_update(move || {
            if let Some(app) = app_weak.upgrade() {
                open_update_download(&app);
            }
        });
    }

    wire_model_catalog_callbacks(app, store.clone());
    wire_reference_callbacks(app, store.clone());
    wire_prompt_preview_callbacks(app);
    wire_generation_callbacks(app, context.clone());
    wire_viewer_callbacks(app, context.clone());
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
