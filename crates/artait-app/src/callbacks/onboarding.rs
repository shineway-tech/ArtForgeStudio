//! 首启引导回调。

use artait_model::ThemeId;
use slint::ComponentHandle;

use super::CbCtx;
use crate::ui::AppState;

pub(crate) fn init(ctx: &CbCtx) {
    let app = ctx.app.upgrade().expect("AppShell 应在 init 前存活");
    let state = app.global::<AppState>();

    // ── 选择预设 ──────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_pick_preset(move |id| {
            ctx.onb.borrow_mut().pick_preset(&id);
            if let Some(app) = ctx.app.upgrade() {
                crate::onboarding::push_to_ui(&app, &ctx.onb.borrow());
            }
        });
    }

    // ── 切换功能开关 ──────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_toggle_feature(move |idx| {
            ctx.onb.borrow_mut().toggle_feature(idx as usize);
            if let Some(app) = ctx.app.upgrade() {
                crate::onboarding::push_to_ui(&app, &ctx.onb.borrow());
            }
        });
    }

    // ── 选择主题 ──────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_pick_theme(move |id| {
            ctx.onb.borrow_mut().pick_theme(&id);
            if let Some(app) = ctx.app.upgrade() {
                crate::onboarding::push_to_ui(&app, &ctx.onb.borrow());
                let id = ctx.onb.borrow().theme_id;
                let loaded = crate::theme::load(id);
                crate::theme::apply(&app, &loaded);
            }
        });
    }

    // ── 使用旧路径 ──────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_use_legacy_paths(move || {
            ctx.onb.borrow_mut().use_legacy_paths();
            if let Some(app) = ctx.app.upgrade() {
                crate::onboarding::push_to_ui(&app, &ctx.onb.borrow());
            }
        });
    }

    // ── 选择目录 ──────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_pick_dir(move |kind| {
            let kind_s = kind.to_string();
            let starting = ctx.onb.borrow().current_dir(&kind_s).clone();
            let title = match kind_s.as_str() {
                "input" => "选择输入素材目录",
                "output" => "选择输出目录",
                "prompt" => "选择提示词模板目录",
                _ => "选择目录",
            };

            let picked = rfd::FileDialog::new()
                .set_title(title)
                .set_directory(if starting.exists() {
                    starting.as_path()
                } else {
                    starting.parent().unwrap_or(std::path::Path::new("."))
                })
                .pick_folder();

            if let Some(path) = picked {
                ctx.onb.borrow_mut().set_dir(&kind_s, path);
            }
            if let Some(app) = ctx.app.upgrade() {
                crate::onboarding::push_to_ui(&app, &ctx.onb.borrow());
            }
        });
    }

    // ── 下一步 ──────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_next(move || {
            ctx.onb.borrow_mut().next();
            if let Some(app) = ctx.app.upgrade() {
                crate::onboarding::push_to_ui(&app, &ctx.onb.borrow());
            }
        });
    }

    // ── 上一步 ──────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_prev(move || {
            ctx.onb.borrow_mut().prev();
            if let Some(app) = ctx.app.upgrade() {
                crate::onboarding::push_to_ui(&app, &ctx.onb.borrow());
            }
        });
    }

    // ── 完成引导 ──────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_finish(move || {
            let draft = ctx.onb.borrow().clone();
            let new_cfg = draft.into_config();
            *ctx.cfg.borrow_mut() = new_cfg.clone();
            if let Err(e) = artait_config::ensure_dirs(&new_cfg) {
                tracing::warn!(error = %e, "ensure_dirs 失败");
            }
            ctx.save_cfg();

            *ctx.theme_id.borrow_mut() = new_cfg.ui.theme;
            ctx.user_active.store(
                matches!(new_cfg.ui.theme, ThemeId::User),
                std::sync::atomic::Ordering::Relaxed,
            );

            if let Some(app) = ctx.app.upgrade() {
                let loaded = crate::theme::load(new_cfg.ui.theme);
                crate::theme::apply(&app, &loaded);

                let s = app.global::<AppState>();
                s.set_features(crate::build_feature_model(
                    &ctx.cfg.borrow(),
                    "art",
                    ctx.cfg.borrow().last_project.is_some(),
                ));
                s.set_current_theme_id(crate::theme::id_str(new_cfg.ui.theme).into());
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                s.set_in_onboarding(false);
                s.set_current_page("welcome".into());
                s.set_status_text("欢迎来到 ArtForge Studio".into());
            }
        });
    }

    // ── 跳过 provider ──────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_onboarding_skip_provider(move || {
            if let Some(app) = ctx.app.upgrade() {
                app.global::<AppState>()
                    .set_status_text("已跳过 provider 配置 · 可在设置页补".into());
            }
        });
    }
}
