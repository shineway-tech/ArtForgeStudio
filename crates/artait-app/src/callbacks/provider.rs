//! Provider 选择、测试、编辑、添加对话框回调。

use std::time::Duration;

use artait_model::{ProviderScope, TaskKind};
use artait_provider::ProviderContext;
use artait_task::{TaskError, TaskSpec};
use slint::ComponentHandle;

use super::CbCtx;
use crate::provider_helpers::*;
use crate::ui::AppState;

pub(crate) fn init(ctx: &CbCtx) {
    let app = ctx.app.upgrade().expect("AppShell 应在 init 前存活");
    let state = app.global::<AppState>();

    // ── 打开设置页 ──────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_open_settings(move || {
            if let Some(app) = ctx.app.upgrade() {
                crate::debug_log("open settings");
                let s = app.global::<AppState>();
                crate::navigate_to_page(
                    &s,
                    &ctx.cfg.borrow(),
                    &ctx.ref_images,
                    &ctx.workspace_drafts,
                    "settings",
                );
                s.set_status_text("打开设置".into());
            }
        });
    }

    // ── 打开任务面板 ──────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_open_tasks(move || {
            if let Some(app) = ctx.app.upgrade() {
                crate::debug_log("open tasks panel");
                let s = app.global::<AppState>();
                crate::navigate_to_page(
                    &s,
                    &ctx.cfg.borrow(),
                    &ctx.ref_images,
                    &ctx.workspace_drafts,
                    "tasks",
                );
                s.set_status_text("打开任务面板".into());
            }
        });
    }

    // ── 添加 Mock 实例 ──────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_add_mock_instance(move || {
            let new_inst = crate::providers::make_mock_instance(&ctx.cfg.borrow().providers);
            ctx.cfg.borrow_mut().providers.push(new_inst.clone());
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                app.global::<AppState>()
                    .set_status_text(format!("已新增 {}", new_inst.name).into());
            }
        });
    }

    // ── 设置默认生图 ──────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_edit_instance(move |id| {
            let id = id.to_string();
            let inst = ctx
                .cfg
                .borrow()
                .providers
                .iter()
                .find(|p| p.id == id)
                .cloned();
            let Some(inst) = inst else {
                if let Some(app) = ctx.app.upgrade() {
                    app.global::<AppState>()
                        .set_status_text(format!("未找到 Provider 实例：{id}").into());
                }
                return;
            };

            if let Some(app) = ctx.app.upgrade() {
                let state = app.global::<AppState>();
                state.set_add_dialog_open(false);
                state.set_edit_instance_id(inst.id.clone().into());
                state.set_add_name(inst.name.clone().into());
                state.set_add_endpoint(inst.endpoint.clone().unwrap_or_default().into());
                state.set_add_api_key(provider_display_api_key(&inst).unwrap_or_default().into());
                state.set_add_node_kind(node_kind_from_scopes(&inst.scopes).into());
                state.set_add_api_style(provider_api_style(&inst).into());
                state.set_add_generation_model(
                    format_model_options(
                        inst.models.generation_model.as_deref(),
                        &inst.models.generation_model_options,
                    )
                    .into(),
                );
                state.set_add_analysis_model(
                    format_model_options(
                        inst.models.analysis_model.as_deref(),
                        &inst.models.analysis_model_options,
                    )
                    .into(),
                );
                state.set_add_error("".into());
                state.set_edit_dialog_open(true);
                state.set_status_text(format!("正在编辑 {}", inst.name).into());
            }
        });
    }

    {
        let ctx = ctx.clone();
        state.on_set_default_generation(move |id| {
            let id = id.to_string();
            let accepted = {
                let mut c = ctx.cfg.borrow_mut();
                let accepted = c
                    .providers
                    .iter_mut()
                    .find(|p| p.id == id && p.scopes.contains(&ProviderScope::Generation))
                    .map(|inst| {
                        inst.show_in_main_ui = true;
                        true
                    })
                    .unwrap_or(false);
                if accepted {
                    c.provider_defaults.generation = Some(id.clone());
                }
                accepted
            };
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                let status = if accepted {
                    format!("默认生图 → {id}")
                } else {
                    format!("{id} 不是生图节点")
                };
                app.global::<AppState>().set_status_text(status.into());
            }
        });
    }

    // ── 选择生图 provider ──────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_select_generation_provider(move |id| {
            let id = id.to_string();
            let display = {
                let mut c = ctx.cfg.borrow_mut();
                let display = c
                    .providers
                    .iter_mut()
                    .find(|p| p.id == id && p.scopes.contains(&ProviderScope::Generation))
                    .map(|p| {
                        p.show_in_main_ui = true;
                        if p.models.generation_model.is_none() {
                            p.models.generation_model =
                                p.models.generation_model_options.first().cloned();
                            sync_provider_model_extra(p);
                        }
                        p.name.clone()
                    });
                if display.is_some() {
                    c.provider_defaults.generation = Some(id.clone());
                }
                display
            };
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                if let Some(name) = display {
                    app.global::<AppState>()
                        .set_status_text(format!("生图接口 → {name}").into());
                }
            }
        });
    }

    // ── 选择生图模型 ────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_select_generation_model(move |model| {
            let model = model.to_string();
            let mut selected_name = None;
            {
                let mut c = ctx.cfg.borrow_mut();
                let selected_id = c.provider_defaults.generation.clone();
                if let Some(id) = selected_id {
                    if let Some(inst) = c.providers.iter_mut().find(|p| p.id == id) {
                        ensure_model_option(&mut inst.models.generation_model_options, &model);
                        inst.models.generation_model = Some(model.clone());
                        sync_provider_model_extra(inst);
                        selected_name = Some(inst.name.clone());
                    }
                }
            }
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                let label = selected_name
                    .map(|name| format!("{name} · {model}"))
                    .unwrap_or(model);
                app.global::<AppState>()
                    .set_status_text(format!("生图模型 → {label}").into());
            }
        });
    }

    // ── 设置 provider 主界面可见性 ──────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_set_provider_main_visible(move |id, visible| {
            let id = id.to_string();
            let name = {
                let mut c = ctx.cfg.borrow_mut();
                let name = c.providers.iter_mut().find(|p| p.id == id).map(|p| {
                    p.show_in_main_ui = visible;
                    p.name.clone()
                });
                if !visible && c.provider_defaults.generation.as_deref() == Some(&id) {
                    c.provider_defaults.generation = c
                        .providers
                        .iter()
                        .find(|p| {
                            p.show_in_main_ui && p.scopes.contains(&ProviderScope::Generation)
                        })
                        .map(|p| p.id.clone());
                }
                if !visible && c.provider_defaults.analysis.as_deref() == Some(&id) {
                    c.provider_defaults.analysis = c
                        .providers
                        .iter()
                        .find(|p| p.show_in_main_ui && p.scopes.contains(&ProviderScope::Analysis))
                        .map(|p| p.id.clone());
                }
                fix_provider_defaults(&mut c);
                name
            };
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                if let Some(name) = name {
                    let action = if visible {
                        "已显示到主页"
                    } else {
                        "已从主页隐藏"
                    };
                    app.global::<AppState>()
                        .set_status_text(format!("{name} · {action}").into());
                }
            }
        });
    }

    // ── 选择工作区质量 ──────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_select_workspace_quality(move |quality| {
            let quality = quality.to_string();
            let (generation_model, api_style) = {
                let c = ctx.cfg.borrow();
                let provider = c
                    .provider_defaults
                    .generation
                    .as_deref()
                    .and_then(|id| c.providers.iter().find(|p| p.id == id));
                let model = provider
                    .and_then(|p| p.models.generation_model.as_deref())
                    .unwrap_or("")
                    .to_string();
                let style = provider
                    .map(provider_api_style)
                    .unwrap_or("auto")
                    .to_string();
                (model, style)
            };
            if let Some(app) = ctx.app.upgrade() {
                let s = app.global::<AppState>();
                crate::providers::select_workspace_quality(
                    &s,
                    &generation_model,
                    &api_style,
                    &quality,
                );
            }
        });
    }

    // ── 设置默认推理 ──────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_set_default_analysis(move |id| {
            let id = id.to_string();
            let accepted = {
                let mut c = ctx.cfg.borrow_mut();
                let accepted = c
                    .providers
                    .iter_mut()
                    .find(|p| p.id == id && p.scopes.contains(&ProviderScope::Analysis))
                    .map(|inst| {
                        inst.show_in_main_ui = true;
                        true
                    })
                    .unwrap_or(false);
                if accepted {
                    c.provider_defaults.analysis = Some(id.clone());
                }
                accepted
            };
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                let status = if accepted {
                    format!("默认推理 → {id}")
                } else {
                    format!("{id} 不是推理节点")
                };
                app.global::<AppState>().set_status_text(status.into());
            }
        });
    }

    // ── 选择推理 provider ──────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_select_analysis_provider(move |id| {
            let id = id.to_string();
            let display = {
                let mut c = ctx.cfg.borrow_mut();
                let display = c
                    .providers
                    .iter_mut()
                    .find(|p| p.id == id && p.scopes.contains(&ProviderScope::Analysis))
                    .map(|p| {
                        p.show_in_main_ui = true;
                        if p.models.analysis_model.is_none() {
                            p.models.analysis_model =
                                p.models.analysis_model_options.first().cloned();
                            sync_provider_model_extra(p);
                        }
                        p.name.clone()
                    });
                if display.is_some() {
                    c.provider_defaults.analysis = Some(id.clone());
                }
                display
            };
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                if let Some(name) = display {
                    app.global::<AppState>()
                        .set_status_text(format!("推理接口 → {name}").into());
                }
            }
        });
    }

    // ── 选择推理模型 ────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_select_analysis_model(move |model| {
            let model = model.to_string();
            let mut selected_name = None;
            {
                let mut c = ctx.cfg.borrow_mut();
                let selected_id = c.provider_defaults.analysis.clone();
                if let Some(id) = selected_id {
                    if let Some(inst) = c.providers.iter_mut().find(|p| p.id == id) {
                        ensure_model_option(&mut inst.models.analysis_model_options, &model);
                        inst.models.analysis_model = Some(model.clone());
                        sync_provider_model_extra(inst);
                        selected_name = Some(inst.name.clone());
                    }
                }
            }
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                let label = selected_name
                    .map(|name| format!("{name} · {model}"))
                    .unwrap_or(model);
                app.global::<AppState>()
                    .set_status_text(format!("推理模型 → {label}").into());
            }
        });
    }

    // ── 移除 provider 实例 ──────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_remove_instance(move |id| {
            let id_str = id.to_string();
            let mut c = ctx.cfg.borrow_mut();
            c.providers.retain(|p| p.id != id_str);
            if c.provider_defaults.generation.as_deref() == Some(&id_str) {
                c.provider_defaults.generation = None;
            }
            if c.provider_defaults.analysis.as_deref() == Some(&id_str) {
                c.provider_defaults.analysis = None;
            }
            fix_provider_defaults(&mut c);
            drop(c);
            ctx.save_cfg();
            if let Some(app) = ctx.app.upgrade() {
                crate::providers::push_providers(&app, &ctx.cfg.borrow());
                app.global::<AppState>()
                    .set_status_text(format!("已删除 {id}").into());
            }
        });
    }

    // ── 测试连接 ────────────────────────────────────────────────────────────
    {
        let ctx = ctx.clone();
        state.on_test_connection(move |id| {
            let id_str = id.to_string();
            let inst = ctx
                .cfg
                .borrow()
                .providers
                .iter()
                .find(|p| p.id == id_str)
                .cloned();
            let Some(inst) = inst else { return };

            let registry = ctx.registry.clone();
            let http = ctx.http.clone();
            let runner = ctx.runner.clone();
            runner.spawn(
                TaskSpec::new(format!("连接测试 · {}", inst.name), TaskKind::Analysis)
                    .with_timeout(Duration::from_secs(20)),
                move |task_ctx| async move {
                    artait_service::provider_helpers::run_connection_test(
                        &inst, &registry, http, &task_ctx,
                    )
                    .await?;
                    Ok(())
                },
            );
        });
    }

    // ── 新增/编辑 Provider 对话框 ───────────────────────────────
    {
        let app_weak = ctx.app.clone();
        state.on_open_add_dialog(move || {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                s.set_edit_dialog_open(false);
                s.set_add_error("".into());
                s.set_add_dialog_open(true);
            }
        });
    }

    {
        let app_weak = ctx.app.clone();
        state.on_quick_add_provider(move |template| {
            let t = template.to_string();
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<crate::ui::AppState>();
                s.set_edit_dialog_open(false);
                s.set_add_error("".into());
                match t.as_str() {
                    "openai" => {
                        s.set_add_name("OpenAI 兼容".into());
                        s.set_add_endpoint("https://api.openai.com/v1".into());
                        s.set_add_node_kind("both".into());
                        s.set_add_api_style("auto".into());
                        s.set_add_generation_model("gpt-image-1".into());
                        s.set_add_analysis_model("gpt-4o-mini".into());
                    }
                    "gemini" => {
                        s.set_add_name("Google Gemini".into());
                        s.set_add_endpoint(
                            "https://generativelanguage.googleapis.com/v1beta".into(),
                        );
                        s.set_add_node_kind("both".into());
                        s.set_add_api_style("gemini".into());
                        s.set_add_generation_model("gemini-2.5-flash-image-preview".into());
                        s.set_add_analysis_model("gemini-2.5-flash".into());
                    }
                    "deepseek" => {
                        s.set_add_name("DeepSeek".into());
                        s.set_add_endpoint("https://api.deepseek.com/v1".into());
                        s.set_add_node_kind("analysis".into());
                        s.set_add_api_style("auto".into());
                        s.set_add_generation_model("".into());
                        s.set_add_analysis_model("deepseek-chat".into());
                    }
                    "volcengine" => {
                        s.set_add_name("火山引擎 Seedance".into());
                        s.set_add_endpoint("https://visual.volcengineapi.com".into());
                        s.set_add_node_kind("generation".into());
                        s.set_add_api_style("volcengine".into());
                        s.set_add_generation_model("seedancetoimage_v2".into());
                        s.set_add_analysis_model("".into());
                    }
                    _ => {}
                }
                s.set_add_api_key("".into());
                s.set_add_dialog_open(true);
            }
        });
    }

    {
        let app_weak = ctx.app.clone();
        state.on_close_add_dialog(move || {
            if let Some(app) = app_weak.upgrade() {
                app.global::<crate::ui::AppState>()
                    .set_add_dialog_open(false);
            }
        });
    }

    {
        let app_weak = ctx.app.clone();
        let cfg = ctx.cfg.clone();
        let runner = ctx.runner.clone();
        let registry = ctx.registry.clone();
        let http = ctx.http.clone();
        state.on_submit_add_openai(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<crate::ui::AppState>();
            let name = s.get_add_name().to_string();
            let endpoint = s.get_add_endpoint().to_string();
            let api_key = s.get_add_api_key().to_string();
            let node_kind = s.get_add_node_kind().to_string();
            let api_style = s.get_add_api_style().to_string();
            let gen_model = s.get_add_generation_model().to_string();
            let ana_model = s.get_add_analysis_model().to_string();
            let is_edit = s.get_edit_dialog_open();

            if is_edit {
                let edit_id = s.get_edit_instance_id().to_string();
                let params = artait_service::provider::EditProviderParams {
                    name,
                    endpoint,
                    api_key,
                    api_style,
                    node_kind,
                    generation_model: Some(gen_model),
                    analysis_model: Some(ana_model),
                };
                let mut cfg_ref = cfg.borrow_mut();
                match artait_service::provider::edit_provider(&mut cfg_ref, &edit_id, params) {
                    Ok(artait_service::provider::ProviderOpResult::Updated { display_name }) => {
                        drop(cfg_ref);
                        crate::persist(&cfg.borrow());
                        crate::providers::push_providers(&app, &cfg.borrow());
                        s.set_edit_dialog_open(false);
                        s.set_status_text(format!("已更新 {display_name}").into());
                    }
                    Err(e) => {
                        s.set_add_error(e.into());
                    }
                    _ => {}
                }
            } else {
                let params = artait_service::provider::CreateProviderParams {
                    name,
                    endpoint,
                    api_key,
                    api_style,
                    node_kind,
                    generation_model: Some(gen_model),
                    analysis_model: Some(ana_model),
                };
                let mut cfg_ref = cfg.borrow_mut();
                match artait_service::provider::create_provider(&mut cfg_ref, params) {
                    Ok(artait_service::provider::ProviderOpResult::Created {
                        instance, ..
                    }) => {
                        drop(cfg_ref);
                        crate::persist(&cfg.borrow());
                        crate::providers::push_providers(&app, &cfg.borrow());
                        s.set_add_dialog_open(false);
                        s.set_status_text(
                            format!("已新增 {}，正在测试连接…", instance.name).into(),
                        );
                        let registry = registry.clone();
                        let http = http.clone();
                        let inst_clone = instance;
                        runner.spawn(
                            TaskSpec::new(
                                format!("连接测试 · {}", inst_clone.name),
                                TaskKind::Analysis,
                            )
                            .with_timeout(Duration::from_secs(20)),
                            move |task_ctx| async move {
                                artait_service::provider_helpers::run_connection_test(
                                    &inst_clone,
                                    &registry,
                                    http,
                                    &task_ctx,
                                )
                                .await?;
                                Ok(())
                            },
                        );
                    }
                    Err(e) => {
                        s.set_add_error(e.into());
                    }
                    _ => {}
                }
            }
        });
    }

    // ── 获取/应用 Provider 模型列表 ─────────────────────────────
    {
        let app_weak = ctx.app.clone();
        let cfg = ctx.cfg.clone();
        state.on_apply_fetched_provider_models(move |id, generation, analysis| {
            let id = id.to_string();
            let generation = artait_service::provider_helpers::parse_model_options(&generation);
            let analysis = artait_service::provider_helpers::parse_model_options(&analysis);
            if generation.is_empty() && analysis.is_empty() {
                return;
            }
            let display = {
                let mut c = cfg.borrow_mut();
                c.providers.iter_mut().find(|p| p.id == id).map(|inst| {
                    if inst
                        .scopes
                        .contains(&artait_model::ProviderScope::Generation)
                    {
                        artait_service::provider_helpers::merge_model_options(
                            &mut inst.models.generation_model_options,
                            &generation,
                        );
                    }
                    if inst.scopes.contains(&artait_model::ProviderScope::Analysis) {
                        artait_service::provider_helpers::merge_model_options(
                            &mut inst.models.analysis_model_options,
                            &analysis,
                        );
                    }
                    if inst
                        .scopes
                        .contains(&artait_model::ProviderScope::Generation)
                        && inst.models.generation_model.is_none()
                    {
                        inst.models.generation_model =
                            inst.models.generation_model_options.first().cloned();
                    }
                    if inst.scopes.contains(&artait_model::ProviderScope::Analysis)
                        && inst.models.analysis_model.is_none()
                    {
                        inst.models.analysis_model =
                            inst.models.analysis_model_options.first().cloned();
                    }
                    artait_service::provider_helpers::normalize_provider_for_scopes(inst);
                    (
                        inst.name.clone(),
                        inst.models.generation_model_options.len(),
                        inst.models.analysis_model_options.len(),
                    )
                })
            };
            crate::persist(&cfg.borrow());
            if let Some(app) = app_weak.upgrade() {
                crate::providers::push_providers(&app, &cfg.borrow());
                if let Some((name, gc, ac)) = display {
                    app.global::<crate::ui::AppState>().set_status_text(
                        format!("已更新 {name} 模型 · 生图 {gc} · 推理 {ac}").into(),
                    );
                }
            }
        });
    }

    {
        let app_weak = ctx.app.clone();
        let cfg = ctx.cfg.clone();
        let runner = ctx.runner.clone();
        let registry = ctx.registry.clone();
        let http = ctx.http.clone();
        state.on_fetch_provider_models(move |id| {
            let id_str = id.to_string();
            let inst = cfg
                .borrow()
                .providers
                .iter()
                .find(|p| p.id == id_str)
                .cloned();
            let Some(inst) = inst else { return };
            let registry = registry.clone();
            let http = http.clone();
            let app_weak_inner = app_weak.clone();
            runner.spawn(
                TaskSpec::new(format!("获取模型 · {}", inst.name), TaskKind::Analysis)
                    .with_timeout(Duration::from_secs(45)),
                move |ctx| async move {
                    ctx.info("准备读取模型列表");
                    ctx.progress(0.1);
                    let provider = registry.get(&inst.provider_id).ok_or_else(|| {
                        TaskError::Failed(format!("未找到 provider 实现 {}", inst.provider_id))
                    })?;
                    let mut pctx =
                        ProviderContext::with_http(inst.id.clone(), inst.provider_id.clone(), http);
                    pctx.endpoint = inst.endpoint.clone();
                    pctx.extra = inst.extra.clone();
                    pctx.cancellation = ctx.cancel.clone();
                    pctx.secret =
                        artait_service::provider_helpers::load_provider_secret(&inst, Some(&ctx))?;
                    ctx.info("调用模型列表接口");
                    ctx.progress(0.4);
                    let models = provider
                        .list_models(&pctx)
                        .await
                        .map_err(|e| TaskError::Failed(format!("获取模型失败: {e}")))?;
                    let generation = models.generation.join("\n");
                    let analysis = models.analysis.join("\n");
                    ctx.info(format!(
                        "获取完成 · 生图 {} · 推理 {}",
                        models.generation.len(),
                        models.analysis.len()
                    ));
                    ctx.progress(1.0);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak_inner.upgrade() {
                            app.global::<crate::ui::AppState>()
                                .invoke_apply_fetched_provider_models(
                                    id_str.into(),
                                    generation.into(),
                                    analysis.into(),
                                );
                        }
                    });
                    Ok(())
                },
            );
        });
    }
}
