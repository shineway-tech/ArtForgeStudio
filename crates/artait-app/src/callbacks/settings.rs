//! 设置、主题、运行时日志、侧边栏、窗口关闭回调。

use std::sync::atomic::Ordering;

use artait_model::ThemeId;
use slint::{CloseRequestResponse, ComponentHandle, Model};

use super::CbCtx;

pub(crate) fn init(ctx: &CbCtx) {
    let app = ctx.app.upgrade().expect("AppShell 应在 init 前存活");
    let state = app.global::<crate::ui::AppState>();
    push_basic_settings(&state, &ctx.cfg.borrow());

    // ── 运行时日志 ──────────────────────────────────────────────────────────────
    {
        let app_weak = ctx.app.clone();
        state.on_refresh_runtime_log(move || {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                crate::debug_log("refresh runtime log");
                crate::push_runtime_log(&s);
                s.set_status_text("运行日志已刷新".into());
            }
        });
    }

    {
        let app_weak = ctx.app.clone();
        state.on_poll_runtime_log(move || {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                crate::push_runtime_log(&s);
            }
        });
    }

    {
        let app_weak = ctx.app.clone();
        state.on_clear_runtime_log(move || {
            let path = crate::runtime_log_path();
            crate::debug_log(format!("clear runtime log -> {}", path.display()));
            if let Err(e) = std::fs::write(&path, "") {
                tracing::warn!(error = %e, path = %path.display(), "清空运行日志失败");
            }
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                crate::push_runtime_log(&s);
                s.set_status_text("运行日志已清空".into());
            }
        });
    }

    // ── 运行时日志开关 ──────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_set_runtime_log_enabled(move |enabled| {
            crate::RUNTIME_LOG_ENABLED.store(enabled, Ordering::Relaxed);
            {
                let mut c = ctx.cfg.borrow_mut();
                c.runtime.log_enabled = enabled;
            }
            ctx.save_cfg();
            if enabled {
                tracing::info!("运行日志写入已开启");
            }
            if let Some(app) = ctx.app.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                s.set_runtime_log_enabled(enabled);
                crate::push_runtime_log(&s);
                s.set_status_text(
                    (if enabled {
                        "运行日志写入已开启"
                    } else {
                        "运行日志写入已关闭"
                    })
                    .into(),
                );
            }
        });
    }

    {
        let ctx = ctx.clone();
        state.on_set_runtime_debug_log_enabled(move |enabled| {
            crate::RUNTIME_DEBUG_LOG_ENABLED.store(enabled, Ordering::Relaxed);
            {
                let mut c = ctx.cfg.borrow_mut();
                c.runtime.debug_log_enabled = enabled;
            }
            ctx.save_cfg();
            if enabled {
                crate::debug_log("debug runtime log enabled");
            }
            if let Some(app) = ctx.app.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                s.set_runtime_debug_log_enabled(enabled);
                crate::push_runtime_log(&s);
                s.set_status_text(
                    (if enabled {
                        "调试日志已开启"
                    } else {
                        "调试日志已关闭"
                    })
                    .into(),
                );
            }
        });
    }

    // ── 主题 ────────────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_toggle_theme(move || {
            if let Some(app) = ctx.app.upgrade() {
                let mut id = ctx.theme_id.borrow_mut();
                *id = crate::theme::next_id(*id);
                crate::debug_log(format!("toggle theme -> {}", crate::theme::id_str(*id)));
                let loaded = crate::theme::load(*id);
                crate::theme::apply(&app, &loaded);
                ctx.user_active
                    .store(matches!(*id, ThemeId::User), Ordering::Relaxed);

                ctx.cfg.borrow_mut().ui.theme = *id;
                ctx.save_cfg();

                let s = app.global::<crate::ui::AppState>();
                s.set_current_theme_id(crate::theme::id_str(*id).into());
                s.set_status_text(
                    format!(
                        "主题：{} （{}）",
                        crate::theme::id_label(*id),
                        loaded.file.display_name
                    )
                    .into(),
                );
            }
        });
    }

    {
        let ctx = ctx.clone();
        state.on_set_theme(move |raw_id| {
            if let Some(next_id) = crate::theme::id_from_str(&raw_id) {
                if let Some(app) = ctx.app.upgrade() {
                    crate::debug_log(format!("set theme -> {}", crate::theme::id_str(next_id)));
                    *ctx.theme_id.borrow_mut() = next_id;
                    let loaded = crate::theme::load(next_id);
                    crate::theme::apply(&app, &loaded);
                    crate::theme::apply_font_overrides(&app, &ctx.cfg.borrow());
                    ctx.user_active
                        .store(matches!(next_id, ThemeId::User), Ordering::Relaxed);

                    ctx.cfg.borrow_mut().ui.theme = next_id;
                    ctx.save_cfg();

                    let s = app.global::<crate::ui::AppState>();
                    s.set_current_theme_id(crate::theme::id_str(next_id).into());
                    s.set_status_text(
                        format!(
                            "主题：{} （{}）",
                            crate::theme::id_label(next_id),
                            loaded.file.display_name
                        )
                        .into(),
                    );
                }
            }
        });
    }

    // ── 基础设置 ──────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_preview_theme(move |raw_id| {
            if let Some(next_id) = crate::theme::id_from_str(&raw_id) {
                if let Some(app) = ctx.app.upgrade() {
                    crate::debug_log(format!(
                        "preview theme -> {}",
                        crate::theme::id_str(next_id)
                    ));
                    let loaded = crate::theme::load(next_id);
                    crate::theme::apply(&app, &loaded);
                    crate::theme::apply_font_overrides(&app, &ctx.cfg.borrow());

                    let s = app.global::<crate::ui::AppState>();
                    s.set_current_theme_id(crate::theme::id_str(next_id).into());
                    s.set_status_text(
                        format!(
                            "预览主题：{} （{}）",
                            crate::theme::id_label(next_id),
                            loaded.file.display_name
                        )
                        .into(),
                    );
                }
            }
        });
    }

    {
        let ctx = ctx.clone();
        state.on_preview_font(move |family| {
            if let Some(app) = ctx.app.upgrade() {
                let family = family.to_string();
                let s = app.global::<crate::ui::AppState>();
                let size = s.get_settings_font_size().clamp(11, 22) as u32;

                let theme = app.global::<crate::ui::Theme>();
                let mut typo = theme.get_typo();
                typo.family = family.clone().into();
                typo.size_md = size as f32;
                typo.size_sm = size.saturating_sub(2) as f32;
                typo.size_xs = size.saturating_sub(3) as f32;
                typo.size_lg = (size + 2) as f32;
                typo.size_xl = (size + 4) as f32;
                theme.set_typo(typo);

                s.set_status_text(format!("预览字体：{family} {size}px").into());
            }
        });
    }

    {
        let ctx = ctx.clone();
        state.on_load_system_fonts(move || {
            let fonts = system_fonts();
            if let Some(app) = ctx.app.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                let shared: Vec<slint::SharedString> =
                    fonts.into_iter().map(slint::SharedString::from).collect();
                s.set_settings_font_list(slint::ModelRc::new(slint::VecModel::from(shared)));
            }
        });
    }

    {
        let ctx = ctx.clone();
        state.on_reset_basic_settings(move || {
            if let Some(app) = ctx.app.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                push_basic_settings(&s, &ctx.cfg.borrow());
                s.set_status_text("基础设置已还原为已保存配置".into());
            }
        });
    }

    {
        let ctx = ctx.clone();
        state.on_pick_settings_dir(move |kind| {
            let kind_s = kind.to_string();
            let Some(app) = ctx.app.upgrade() else {
                return;
            };
            let s = app.global::<crate::ui::AppState>();
            let starting = match kind_s.as_str() {
                "input" => s.get_settings_input_dir().to_string(),
                "output" => s.get_settings_output_dir().to_string(),
                "prompt" => s.get_settings_prompt_dir().to_string(),
                _ => String::new(),
            };
            let title = match kind_s.as_str() {
                "input" => "选择输入素材目录",
                "output" => "选择输出目录",
                "prompt" => "选择提示词模板目录",
                _ => "选择目录",
            };
            let start_path = std::path::PathBuf::from(starting);
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if start_path.exists() {
                dialog = dialog.set_directory(&start_path);
            } else if let Some(parent) = start_path.parent() {
                dialog = dialog.set_directory(parent);
            }
            if let Some(path) = dialog.pick_folder() {
                let text = path.display().to_string();
                match kind_s.as_str() {
                    "input" => s.set_settings_input_dir(text.into()),
                    "output" => s.set_settings_output_dir(text.into()),
                    "prompt" => s.set_settings_prompt_dir(text.into()),
                    _ => {}
                }
            }
        });
    }

    {
        let ctx = ctx.clone();
        state.on_save_basic_settings(move || {
            let Some(app) = ctx.app.upgrade() else {
                return;
            };
            let s = app.global::<crate::ui::AppState>();
            let raw_theme = s.get_settings_theme_id().to_string();
            let Some(next_theme) = crate::theme::id_from_str(&raw_theme) else {
                s.set_status_text(format!("未知主题：{raw_theme}").into());
                return;
            };
            let font_family = s.get_settings_font_family().trim().to_string();
            let font_size = s.get_settings_font_size().clamp(11, 22) as u32;

            // 应用配置（单次 borrow_mut，返回变更决策）
            let outcome = {
                use artait_service::settings::{apply_basic_settings, BasicSettingsChange};
                let upload_url = s.get_settings_upload_api_url().trim().to_string();
                let upload_key = s.get_settings_upload_api_key().trim().to_string();
                let mut cfg = ctx.cfg.borrow_mut();
                match apply_basic_settings(
                    &mut cfg,
                    BasicSettingsChange {
                        theme: next_theme,
                        font_family,
                        font_size,
                        input_dir: std::path::PathBuf::from(s.get_settings_input_dir().to_string()),
                        output_dir: std::path::PathBuf::from(
                            s.get_settings_output_dir().to_string(),
                        ),
                        prompt_dir: std::path::PathBuf::from(
                            s.get_settings_prompt_dir().to_string(),
                        ),
                        upload_api_url: if upload_url.is_empty() {
                            None
                        } else {
                            Some(upload_url)
                        },
                        upload_api_key: if upload_key.is_empty() {
                            None
                        } else {
                            Some(upload_key)
                        },
                    },
                ) {
                    Ok(o) => o,
                    Err(e) => {
                        s.set_status_text(e.into());
                        return;
                    }
                }
            }; // ← borrow_mut 立即 drop
            if let Err(e) = artait_config::ensure_dirs(&ctx.cfg.borrow()) {
                tracing::warn!(error = %e, "创建基础设置目录失败");
                s.set_status_text(format!("目录创建失败：{e}").into());
                return;
            }
            tracing::debug!("基础设置保存：执行 save_cfg");
            ctx.save_cfg();
            tracing::debug!("基础设置保存：save_cfg 完成");

            *ctx.theme_id.borrow_mut() = next_theme;
            let loaded = crate::theme::load(next_theme);
            crate::theme::apply(&app, &loaded);
            crate::theme::apply_font_overrides(&app, &ctx.cfg.borrow());
            ctx.user_active
                .store(matches!(next_theme, ThemeId::User), Ordering::Relaxed);
            s.set_current_theme_id(crate::theme::id_str(next_theme).into());
            push_basic_settings(&s, &ctx.cfg.borrow());

            if outcome.output_dir_changed {
                let old = ctx.asset_watcher_slot.borrow_mut().take();
                if let Some(watcher) = old {
                    drop(watcher);
                    let output_dir = ctx.cfg.borrow().paths.output_dir.clone();
                    let watcher = crate::assets::spawn_asset_bridge(
                        &ctx.rt_handle,
                        output_dir.clone(),
                        ctx.app.clone(),
                    );
                    *ctx.asset_watcher_slot.borrow_mut() = Some(watcher);
                    crate::assets::refresh_once(output_dir, ctx.app.clone());
                }
            }
            s.set_status_text("基础设置已保存".into());
        });
    }

    // ── 图床设置 ──────────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_open_upload_url(move || {
            let Some(app) = ctx.app.upgrade() else { return };
            let s = app.global::<crate::ui::AppState>();
            let url = s.get_settings_upload_api_url().trim().to_string();
            let url = if url.is_empty() {
                // 默认 ImgBB
                "https://api.imgbb.com/1/upload".to_string()
            } else {
                url
            };
            if let Err(e) = open::that(&url) {
                s.set_status_text(format!("无法打开浏览器: {e}").into());
            }
        });
    }

    // ── 侧边栏 ──────────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_sidebar_collapse_changed(move |collapsed| {
            if let Some(app) = ctx.app.upgrade() {
                let state = app.global::<crate::ui::AppState>();
                let changed = {
                    let mut c = ctx.cfg.borrow_mut();
                    if c.ui.sidebar_collapsed != collapsed {
                        c.ui.sidebar_collapsed = collapsed;
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    ctx.save_cfg();
                }
                state.set_sidebar_collapsed(collapsed);
                state.set_status_text(
                    (if collapsed {
                        "侧边栏已收起"
                    } else {
                        "侧边栏已展开"
                    })
                    .into(),
                );
            }
        });
    }

    // ── 侧边栏特征可见性切换 ──────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_toggle_sidebar_feature_visibility(move |feature_id| {
            let fid = feature_id.to_string();
            if let Some(app) = ctx.app.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                let changed = {
                    let mut c = ctx.cfg.borrow_mut();
                    if let Some(fid_enum) = artait_model::feature_id_from_route(&fid) {
                        if c.features.sidebar_hidden.contains(&fid_enum) {
                            c.features.sidebar_hidden.retain(|f| *f != fid_enum);
                        } else {
                            c.features.sidebar_hidden.push(fid_enum);
                        }
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    ctx.save_cfg();
                    s.set_features(crate::build_feature_model(
                        &ctx.cfg.borrow(),
                        "art",
                        ctx.cfg.borrow().last_project.is_some(),
                    ));
                }
                s.set_status_text("侧边栏已更新".into());
            }
        });
    }

    // ── 导航（运行时日志、常规页面切换） ──────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_navigate(move |id| {
            if let Some(app) = ctx.app.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                let target = id.to_string();
                s.set_ws_open_select("".into());
                s.set_prompt_history_open(false);
                s.set_ws_advanced_open(false);
                crate::debug_log(format!("navigate page -> {target}"));
                crate::navigate_to_page(
                    &s,
                    &ctx.cfg.borrow(),
                    &ctx.ref_images,
                    &ctx.workspace_drafts,
                    &target,
                );
                if target == "runtime_log" {
                    crate::push_runtime_log(&s);
                }
                if target == "settings" && s.get_settings_font_list().row_count() == 0 {
                    let fonts = system_fonts();
                    let shared: Vec<slint::SharedString> =
                        fonts.into_iter().map(slint::SharedString::from).collect();
                    s.set_settings_font_list(slint::ModelRc::new(slint::VecModel::from(shared)));
                }
                s.set_status_text(format!("已切换：{target}").into());

                // 导航到角色库/场景库时自动加载数据
                if target == "character_library" {
                    let store = ctx.character_store.borrow();
                    crate::callbacks::character_library::refresh_character_list(
                        &s,
                        store.all_characters(),
                    );
                }
                if target == "scene_library" {
                    let store = ctx.scene_store.borrow();
                    crate::callbacks::scene_library::refresh_scene_list(&s, store.all_scenes());
                }
            }
        });
    }

    // ── 窗口关闭时保存工作状态 ────────────────────────────────────────────────
    // 注册在 app.window() 上（调用方传入 app shell）
}

/// 窗口关闭回调，需要在 main.rs 中手动挂载：
/// `callbacks::settings::on_close_requested(ctx, &app);`
pub(crate) fn on_close_requested(ctx: &CbCtx, app: &crate::ui::AppShell) {
    let ctx = ctx.clone();
    app.window().on_close_requested(move || {
        if let Some(app) = ctx.app.upgrade() {
            crate::persist_current_app_state(
                &app,
                &ctx.cfg,
                &ctx.ref_images,
                &ctx.workspace_drafts,
            );
        }
        let _ = slint::quit_event_loop();
        CloseRequestResponse::HideWindow
    });
}

fn push_basic_settings(state: &crate::ui::AppState, cfg: &artait_model::AppConfig) {
    state.set_settings_theme_id(crate::theme::id_str(cfg.ui.theme).into());
    state.set_settings_font_family(cfg.ui.font_family.clone().into());
    state.set_settings_font_size(cfg.ui.font_size.min(i32::MAX as u32) as i32);
    state.set_settings_input_dir(cfg.paths.input_dir.display().to_string().into());
    state.set_settings_output_dir(cfg.paths.output_dir.display().to_string().into());
    state.set_settings_prompt_dir(cfg.paths.prompt_dir.display().to_string().into());
    state.set_settings_upload_api_url(cfg.image_upload.api_url.as_deref().unwrap_or("").into());
    state.set_settings_upload_api_key(cfg.image_upload.api_key.as_deref().unwrap_or("").into());
}

/// 枚举系统已安装字体。
fn system_fonts() -> Vec<String> {
    let mut fonts: Vec<String> = Vec::new();

    #[cfg(windows)]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        if let Ok(key) = hklm.open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts") {
            for name in key.enum_values().filter_map(|v| v.ok()).map(|(n, _)| n) {
                // 去掉 "(TrueType)"、"(OpenType)" 等后缀
                let family = name
                    .replace(" (TrueType)", "")
                    .replace(" (OpenType)", "")
                    .replace(" (TrueType) V2", "")
                    .trim()
                    .to_string();
                if !family.is_empty() && !fonts.contains(&family) {
                    fonts.push(family);
                }
            }
        }
        fonts.sort();
    }

    #[cfg(not(windows))]
    {
        // macOS/Linux fallback：返回常用字体
        fonts.extend_from_slice(&[
            "Sarasa UI SC".into(),
            "Noto Sans CJK SC".into(),
            "Source Han Sans SC".into(),
            "PingFang SC".into(),
            "Hiragino Sans GB".into(),
            "Microsoft YaHei".into(),
            "Arial".into(),
            "Helvetica".into(),
            "monospace".into(),
        ]);
    }

    // 确保默认字体在列表里
    if !fonts.iter().any(|f| f == "Sarasa UI SC") {
        fonts.insert(0, "Sarasa UI SC".into());
    }
    fonts
}
