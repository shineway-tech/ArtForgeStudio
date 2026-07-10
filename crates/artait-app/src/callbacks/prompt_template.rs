//! 工作区回调：参考图管理、提示词优化、提示词模板 CRUD。

use std::time::Duration;

use artait_model::{ReferenceImage, ReferenceImageSource, TaskKind};
use artait_service::prompt_template::{
    build_generation_prompt, build_prompt_optimization_user_prompt_with_context,
    default_prompt_template_content, default_template_category, prompt_optimization_system_prompt,
    prompt_template_relative_path, read_prompt_template, template_category_from_file,
    template_label_from_file, write_prompt_template, PromptOptimizationContext,
};
use artait_service::provider_helpers::run_analysis;
use artait_service::sidecar::{OptimizeResult, PromptOptimizerClient};
use artait_service::TaskMeta;
use artait_task::{TaskError, TaskSpec};
use slint::{ComponentHandle, Timer};

use super::CbCtx;
use crate::prompt_template::refresh_template_model;
use crate::ui::{AppShell, AppState};
use crate::{clipboard_reference, debug_log, push_ws_ref_images, save_workspace_draft};
use artait_service::utils;

pub(crate) fn init(ctx: &CbCtx, app: &AppShell) {
    let state = app.global::<AppState>();
    let app_weak = ctx.app.clone();
    let cfg_ref = ctx.cfg.clone();
    let registry = ctx.registry.clone();
    let runner = ctx.runner.clone();
    let http = ctx.http.clone();
    let ref_images = ctx.ref_images.clone();
    let drafts = ctx.workspace_drafts.clone();
    let last_clipboard_image_sequence = std::rc::Rc::new(std::cell::RefCell::new(0_u64));

    // ── 参考图 ───────────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let ref_images = ref_images.clone();
        let drafts = drafts.clone();
        state.on_add_reference_images(move || {
            debug_log("open reference image picker");
            let picked = rfd::FileDialog::new()
                .set_title("选择参考图")
                .add_filter("图片文件", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
                .pick_files()
                .unwrap_or_default();
            if picked.is_empty() {
                return;
            }
            debug_log(format!("reference images picked -> {}", picked.len()));
            let mut ri = ref_images.borrow_mut();
            for path in picked {
                push_reference_path(&mut ri, path, ReferenceImageSource::UserPicked);
            }
            drop(ri);
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                push_ws_ref_images(&s, &ref_images.borrow());
                save_workspace_draft(&s, &ref_images.borrow(), &drafts);
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let ref_images = ref_images.clone();
        let drafts = drafts.clone();
        let last_sequence = last_clipboard_image_sequence.clone();
        state.on_try_add_clipboard_reference_image(move || {
            add_clipboard_reference_image(&app_weak, &ref_images, &drafts, &last_sequence);
        });
    }
    {
        let app_weak = app_weak.clone();
        let ref_images = ref_images.clone();
        let drafts = drafts.clone();
        state.on_remove_reference_image(move |idx| {
            let i = idx as usize;
            if i < ref_images.borrow().len() {
                ref_images.borrow_mut().remove(i);
            }
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                push_ws_ref_images(&s, &ref_images.borrow());
                save_workspace_draft(&s, &ref_images.borrow(), &drafts);
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let ref_images = ref_images.clone();
        let drafts = drafts.clone();
        state.on_clear_reference_images(move || {
            ref_images.borrow_mut().clear();
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                push_ws_ref_images(&s, &ref_images.borrow());
                save_workspace_draft(&s, &ref_images.borrow(), &drafts);
            }
        });
    }

    // ── 分析参考图 ───────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let registry = registry.clone();
        let runner = runner.clone();
        let http = http.clone();
        let ref_images = ref_images.clone();
        state.on_analyze_reference_images(move || {
            let Some(app) = app_weak.upgrade() else { return };
            let s = app.global::<AppState>();
            let source_page = s.get_current_page().to_string();
            let inst = find_analysis_inst(&cfg_ref.borrow());
            let Some(inst) = inst else { s.set_status_text("未配置推理 provider，无法分析参考图".into()); return; };
            let imgs = ref_images.borrow().clone();
            if imgs.is_empty() { s.set_status_text("没有参考图可以分析".into()); return; }

            let registry = registry.clone(); let http = http.clone();
            let app_weak_inner = app_weak.clone();
            runner.spawn(TaskSpec::new(format!("分析参考图 · {} 张", imgs.len()), TaskKind::Analysis)
                .with_timeout(Duration::from_secs(60)), move |ctx| async move {
                ctx.info(format!("分析 {} 张参考图...", imgs.len())); ctx.progress(0.1);
                let result = run_analysis(&inst, artait_provider::request::AnalysisRequest {
                    system_prompt: Some("你是一个专业的AI绘画提示词专家。根据用户提供的参考图片，\
                        请详细描述画面的内容、构图、风格、色彩、光影和氛围，输出一段可直接用于AI生图的英文提示词。\
                        保持简洁但信息丰富，控制在200字以内。直接输出提示词，不要包含任何解释。".to_string()),
                    user_prompt: "请根据以下参考图生成生图提示词".to_string(),
                    images: imgs, model: None,
                    response_format: artait_provider::request::AnalysisResponseFormat::Plain,
                }, &registry, http, &ctx).await?;
                ctx.progress(1.0);
                let text = result.text.trim().to_string();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_weak_inner.upgrade() {
                        let s = app.global::<AppState>();
                        if s.get_current_page().as_str() == source_page {
                            s.set_ws_prompt(text.into());
                            s.set_status_text("参考图分析完成，提示词已填入".into());
                        }
                    }
                });
                ctx.info("分析完成"); Ok(())
            });
        });
    }

    // ── 提示词优化 ───────────────────────────────────────────────
    // 优先使用 Prompt Optimizer sidecar（多轮迭代），不可用时降级到单轮 Analyzer
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let registry = registry.clone();
        let runner = runner.clone();
        let http = http.clone();
        let ref_images = ref_images.clone();
        let sidecar_mgr = ctx.sidecar.clone();
        let task_meta_map = ctx.task_meta_map.clone();
        state.on_optimize_prompt(move |kind| {
            debug_log(format!("optimize prompt requested -> kind={kind}"));
            let Some(app) = app_weak.upgrade() else { return };
            let s = app.global::<AppState>();
            let kind = kind.to_string();
            let source_page = s.get_current_page().to_string();
            let manual_prompt = s.get_ws_prompt().to_string();
            let selected_template = s.get_ws_template_file().to_string();
            if manual_prompt.trim().is_empty() && selected_template.trim().is_empty() {
                s.set_status_text("请先输入提示词或选择目录提示词".into()); return;
            }
            let inst = find_analysis_inst(&cfg_ref.borrow());
            let Some(inst) = inst else { s.set_status_text("未配置推理 provider，无法优化提示词".into()); return; };
            let imgs = if kind == "image" {
                let imgs = ref_images.borrow().clone();
                if imgs.is_empty() { s.set_status_text("图文优化需要先添加参考图".into()); return; }
                imgs
            } else { Vec::new() };
            let preset_prompt = match build_generation_prompt(&cfg_ref.borrow(), &source_page, &selected_template, "") {
                Ok(v) => v, Err(e) => { s.set_status_text(format!("读取目录提示词失败: {e}").into()); return; }
            };
            let mode = artait_model::CreationMode::from_route(&source_page);
            let director_controls = super::generation::director_controls_from_state(&s, mode);
            let director_summary = artait_service::assets::director_summary(&director_controls);
            let final_prompt_preview = match super::generation::build_final_generation_prompt(
                &cfg_ref.borrow(),
                mode,
                &selected_template,
                &manual_prompt,
                &director_controls,
            ) {
                Ok(v) => v,
                Err(e) => {
                    s.set_status_text(format!("读取最终 Prompt 预览失败: {e}").into());
                    return;
                }
            };
            let title = if kind == "image" { format!("图文优化 · {} 张参考图", imgs.len()) } else { "提示词优化".to_string() };
            s.set_ws_prompt_optimizing(true); s.set_ws_prompt_opt_title(title.clone().into());
            s.set_ws_prompt_opt_summary("尝试启动深度优化引擎…".into());
            s.set_ws_prompt_opt_changes("".into()); s.set_status_text(format!("{title}中...").into());

            let registry = registry.clone(); let http = http.clone();
            let app_weak_inner = app_weak.clone();
            let sidecar_mgr = sidecar_mgr.clone();
            let sidecar_cfg = cfg_ref.borrow().sidecar.clone();
            let meta_model = inst
                .models
                .analysis_model
                .clone()
                .or_else(|| inst.models.generation_model.clone())
                .unwrap_or_default();
            let meta_prompt = if !manual_prompt.trim().is_empty() {
                manual_prompt.clone()
            } else {
                preset_prompt.clone()
            };
            let meta_kind = kind.clone();
            let meta_provider_instance_id = inst.name.clone();
            let meta_provider_id = inst.provider_id.clone();

            let task_id = runner.spawn(TaskSpec::new(title.clone(), TaskKind::PromptOptimization)
                .with_timeout(Duration::from_secs(360)), move |ctx| async move {
                // ── 尝试 Sidecar 深度优化 ──
                // Sidecar 目前只接收文本 prompt；图文优化必须走视觉分析，否则参考图不会参与结果。
                if should_skip_text_sidecar_for_prompt_optimization(&kind) {
                    ctx.info("图文优化跳过文本 sidecar，直接使用视觉分析");
                } else {
                    match sidecar_mgr.ensure_prompt_optimizer(&sidecar_cfg).await {
                        Ok(client) => {
                            // 同步 Provider 设置
                            let ep = inst.endpoint.as_deref().unwrap_or("https://api.openai.com/v1");
                            let secret = artait_service::provider_helpers::load_provider_secret(&inst, Some(&ctx))
                                .ok().flatten().unwrap_or_default();
                            let _ = client.sync_provider_settings(ep, &secret).await;

                            let combined = build_prompt_optimization_user_prompt_with_context(PromptOptimizationContext {
                                page: &source_page,
                                preset_prompt: &preset_prompt,
                                user_prompt: &manual_prompt,
                                director_controls: &director_summary,
                                final_prompt_preview: &final_prompt_preview,
                                with_images: false,
                            });

                            ctx.info("Sidecar 深度优化引擎已启动");
                            let _ = slint::invoke_from_event_loop({
                                let app_weak = app_weak_inner.clone();
                                move || { if let Some(app) = app_weak.upgrade() { let s = app.global::<AppState>();
                                    s.set_ws_prompt_opt_summary("深度优化引擎运行中，多轮迭代…".into());
                                }}
                            });

                            match sidecar_optimize(&client, &combined, &inst, &ctx).await {
                                Ok(result) => {
                                    let summary = result.summary.unwrap_or_default();
                                    let changes = format!("{} 轮迭代 · 得分 {}/100",
                                        result.rounds,
                                        result.score.map(|s| s as u32).unwrap_or(0));
                                    let app_weak_for_ui = app_weak_inner.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(app) = app_weak_for_ui.upgrade() {
                                            let s = app.global::<AppState>();
                                            s.set_ws_prompt(result.optimized_prompt.into());
                                            s.set_ws_prompt_optimizing(false);
                                            s.set_ws_prompt_opt_summary(summary.into());
                                            s.set_ws_prompt_opt_changes(changes.into());
                                            s.set_status_text("深度优化完成".into());
                                        }
                                    });
                                    schedule_prompt_optimization_close(&app_weak_inner);
                                    ctx.info("深度优化完成"); return Ok(());
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "sidecar 优化失败，降级到单轮分析");
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "sidecar 不可用，降级到单轮分析");
                        }
                    }
                }

                // ── 降级：单轮 Analyzer 优化 ──
                ctx.info("使用单轮分析进行提示词优化"); ctx.progress(0.1);
                let system_prompt = prompt_optimization_system_prompt(kind == "image");
                let user_prompt = build_prompt_optimization_user_prompt_with_context(PromptOptimizationContext {
                    page: &source_page,
                    preset_prompt: &preset_prompt,
                    user_prompt: &manual_prompt,
                    director_controls: &director_summary,
                    final_prompt_preview: &final_prompt_preview,
                    with_images: kind == "image",
                });
                { let _ = slint::invoke_from_event_loop({ let app_weak = app_weak_inner.clone(); let n = inst.name.clone();
                    move || { if let Some(app) = app_weak.upgrade() { let s = app.global::<AppState>();
                    s.set_ws_prompt_opt_summary(format!("使用单轮分析：{n}").into());
                    s.set_ws_prompt_opt_changes("（如需多轮深度优化，请放置 prompt-optimizer-server.exe）".into()); } } }); }
                ctx.progress(0.35);
                let analyze_fut = run_analysis(&inst, artait_provider::request::AnalysisRequest {
                    system_prompt: Some(system_prompt), user_prompt, images: imgs, model: None,
                    response_format: artait_provider::request::AnalysisResponseFormat::Json,
                }, &registry, http, &ctx);
                let result = match tokio::time::timeout(Duration::from_secs(35), analyze_fut).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => {
                        set_prompt_optimization_failed(&app_weak_inner, &format!("{e}"));
                        return Err(TaskError::Failed(format!("{e}")));
                    }
                    Err(_) => {
                        let msg = "提示词优化请求超时（35s），请检查推理 provider 响应速度或重试";
                        set_prompt_optimization_failed(&app_weak_inner, msg);
                        return Err(TaskError::Failed(msg.into()));
                    }
                };
                ctx.progress(0.95);
                let parsed = artait_service::prompt_template::parse_prompt_optimization_output(&result);
                let app_weak_for_ui = app_weak_inner.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_weak_for_ui.upgrade() {
                        let s = app.global::<AppState>();
                        s.set_ws_prompt(parsed.optimized_prompt.into());
                        s.set_ws_prompt_optimizing(false);
                        s.set_ws_prompt_opt_summary(parsed.summary.into());
                        s.set_ws_prompt_opt_changes(parsed.changes.into());
                        s.set_status_text("提示词优化完成".into());
                    }
                });
                schedule_prompt_optimization_close(&app_weak_inner);
                ctx.info("优化完成"); Ok(())
            });
            if let Ok(mut meta_map) = task_meta_map.try_lock() {
                meta_map.insert(task_id, TaskMeta {
                    provider_instance_id: meta_provider_instance_id,
                    provider_id: meta_provider_id,
                    model: meta_model,
                    prompt: meta_prompt,
                    extra_json: format!(r#"{{"mode":"prompt_opt","kind":"{}"}}"#, meta_kind),
                    ..TaskMeta::default()
                });
            }
        });
    }

    // ── 提示词模板 CRUD ─────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_save_prompt_template(move || {
            save_template_impl(&app_weak, &cfg_ref);
        });
    }
    {
        let app_weak = app_weak.clone();
        state.on_open_create_prompt_template(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let page = s.get_current_page().to_string();
            debug_log(format!("open create prompt template dialog -> page={page}"));
            s.set_prompt_template_dialog_open(true);
            s.set_prompt_template_editing(false);
            s.set_prompt_template_original_file("".into());
            s.set_prompt_template_title("创建提示词".into());
            s.set_prompt_template_name("".into());
            s.set_prompt_template_category(s.get_ws_template_active_category());
            s.set_prompt_template_show_categories(false);
            s.set_prompt_template_format("json".into());
            s.set_prompt_template_mode("manual".into());
            s.set_prompt_template_content(default_prompt_template_content(&page).into());
            s.set_prompt_template_negative("".into());
            s.set_prompt_template_error("".into());
            s.set_status_text("已打开创建提示词窗口".into());
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_open_edit_prompt_template(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let page = s.get_current_page().to_string();
            let file = s.get_ws_template_file().to_string();
            debug_log(format!(
                "open edit prompt template dialog -> page={page}, file={file}"
            ));
            if file.trim().is_empty() {
                s.set_status_text("请先选择一个要编辑的提示词模板".into());
                refresh_template_model(&s, &cfg_ref.borrow(), &page);
                s.set_ws_show_templates(true);
                return;
            }
            match read_prompt_template(&cfg_ref.borrow(), &page, &file) {
                Ok(tmpl) => {
                    s.set_prompt_template_dialog_open(true);
                    s.set_prompt_template_editing(true);
                    s.set_prompt_template_original_file(file.into());
                    s.set_prompt_template_title("编辑提示词".into());
                    s.set_prompt_template_name(tmpl.name.into());
                    s.set_prompt_template_category(tmpl.category.into());
                    s.set_prompt_template_show_categories(false);
                    s.set_prompt_template_format(tmpl.format.into());
                    s.set_prompt_template_mode("manual".into());
                    s.set_prompt_template_content(tmpl.positive.into());
                    s.set_prompt_template_negative(tmpl.negative.into());
                    s.set_prompt_template_error("".into());
                    s.set_status_text("已打开编辑提示词窗口".into());
                }
                Err(e) => s.set_status_text(format!("读取模板失败: {e}").into()),
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        state.on_close_prompt_template_dialog(move || {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                s.set_prompt_template_dialog_open(false);
                s.set_prompt_template_show_categories(false);
                s.set_prompt_template_error("".into());
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let ref_images = ref_images.clone();
        let drafts = drafts.clone();
        state.on_submit_prompt_template(move || {
            submit_template_impl(&app_weak, &cfg_ref, &ref_images, &drafts);
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let ref_images = ref_images.clone();
        let drafts = drafts.clone();
        state.on_load_prompt_template(move |name| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let page = s.get_current_page().to_string();
            let file_name = name.to_string();
            if file_name.trim().is_empty() {
                s.set_ws_template_file("".into());
                s.set_ws_template_name("".into());
                s.set_ws_template_category(default_template_category().into());
                s.set_ws_template_active_category(default_template_category().into());
                s.set_ws_negative("".into());
                save_workspace_draft(&s, &ref_images.borrow(), &drafts);
                s.set_status_text("未使用目录提示词".into());
                return;
            }
            match read_prompt_template(&cfg_ref.borrow(), &page, &file_name) {
                Ok(tmpl) => {
                    let label = template_label_from_file(&file_name);
                    s.set_ws_template_file(file_name.into());
                    s.set_ws_template_category(tmpl.category.clone().into());
                    s.set_ws_template_active_category(tmpl.category.into());
                    s.set_ws_template_name(label.clone().into());
                    s.set_ws_negative("".into());
                    save_workspace_draft(&s, &ref_images.borrow(), &drafts);
                    s.set_status_text(
                        format!("已选择目录提示词 {}，生成时会自动拼接", label).into(),
                    );
                }
                Err(e) => s.set_status_text(format!("读取失败: {e}").into()),
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_refresh_template_list(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            refresh_template_model(&s, &cfg_ref.borrow(), &s.get_current_page().to_string());
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let registry = registry.clone();
        let runner = runner.clone();
        let http = http.clone();
        let ref_images = ref_images.clone();
        state.on_analyze_prompt_template_images(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let name = s.get_prompt_template_name().to_string();
            if name.trim().is_empty() {
                s.set_prompt_template_error("请先输入提示词名称".into());
                return;
            }
            let inst = find_analysis_inst(&cfg_ref.borrow());
            let Some(inst) = inst else {
                s.set_prompt_template_error("未配置推理 provider，无法分析图片".into());
                return;
            };
            let imgs = ref_images.borrow().clone();
            if imgs.is_empty() {
                s.set_prompt_template_error("请先上传至少一张风格参考图".into());
                return;
            }
            s.set_prompt_template_error("正在分析图片风格...".into());

            let registry = registry.clone();
            let http = http.clone();
            let app_weak_inner = app_weak.clone();
            runner.spawn(
                TaskSpec::new(
                    format!("模板风格分析 · {} · {} 张", name, imgs.len()),
                    TaskKind::Analysis,
                )
                .with_timeout(Duration::from_secs(60)),
                move |ctx| async move {
                    ctx.info(format!("分析 {} 张参考图...", imgs.len()));
                    ctx.progress(0.1);
                    let result = run_analysis(
                        &inst,
                        artait_provider::request::AnalysisRequest {
                            system_prompt: Some(
                                "你是一个专业的AI美术提示词模板设计师。根据参考图片，\
                        总结可复用的美术风格、构图、材质、色彩、光影和关键视觉特征。\
                        输出一段适合保存为生图模板的中文提示词，结构清晰，可直接编辑复用。"
                                    .to_string(),
                            ),
                            user_prompt: format!("请为「{}」生成一份可复用提示词模板", name),
                            images: imgs,
                            model: None,
                            response_format:
                                artait_provider::request::AnalysisResponseFormat::Plain,
                        },
                        &registry,
                        http,
                        &ctx,
                    )
                    .await?;
                    ctx.progress(1.0);
                    let text = result.text.trim().to_string();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak_inner.upgrade() {
                            let s = app.global::<AppState>();
                            s.set_prompt_template_content(text.into());
                            s.set_prompt_template_error("".into());
                            s.set_status_text("模板图片分析完成，可编辑后保存".into());
                        }
                    });
                    ctx.info("分析完成");
                    Ok(())
                },
            );
        });
    }
}

// ── 辅助 ────────────────────────────────────────────────────────────

/// 通过 sidecar 执行多轮提示词优化。
async fn sidecar_optimize(
    client: &PromptOptimizerClient,
    prompt: &str,
    inst: &artait_model::ProviderInstance,
    ctx: &artait_task::TaskContext,
) -> Result<OptimizeResult, TaskError> {
    let optimizer_model = inst
        .models
        .analysis_model
        .as_deref()
        .or(inst.models.generation_model.as_deref());
    let judge_model = inst.models.analysis_model.as_deref();

    let job_id = client
        .submit_optimization(prompt, optimizer_model, judge_model)
        .await
        .map_err(|e| TaskError::Failed(format!("sidecar 提交失败: {e}")))?;

    ctx.info(format!("Sidecar 任务已提交: {job_id}"));
    client.poll_until_done(&job_id, ctx).await
}

fn set_prompt_optimization_failed(app_weak: &slint::Weak<AppShell>, message: &str) {
    let message = message.to_string();
    let _ = slint::invoke_from_event_loop({
        let app_weak = app_weak.clone();
        move || {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                s.set_ws_prompt_optimizing(false);
                s.set_ws_prompt_opt_summary("优化失败".into());
                s.set_ws_prompt_opt_changes(message.clone().into());
                s.set_status_text(message.into());
            }
        }
    });
}

fn schedule_prompt_optimization_close(app_weak: &slint::Weak<AppShell>) {
    let app_weak = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        Timer::single_shot(Duration::from_millis(3_000), move || {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                if !s.get_ws_prompt_optimizing() {
                    s.set_ws_prompt_opt_title("".into());
                    s.set_ws_prompt_opt_summary("".into());
                    s.set_ws_prompt_opt_changes("".into());
                }
            }
        });
    });
}

fn should_skip_text_sidecar_for_prompt_optimization(kind: &str) -> bool {
    kind == "image"
}

fn find_analysis_inst(cfg: &artait_model::AppConfig) -> Option<artait_model::ProviderInstance> {
    cfg.provider_defaults
        .analysis
        .as_deref()
        .and_then(|id| cfg.providers.iter().find(|p| p.id == id).cloned())
        .or_else(|| {
            cfg.providers
                .iter()
                .find(|p| p.scopes.contains(&artait_model::ProviderScope::Analysis))
                .cloned()
        })
}

fn push_reference_path(
    refs: &mut Vec<ReferenceImage>,
    path: std::path::PathBuf,
    source: ReferenceImageSource,
) {
    if refs.iter().any(|r| r.local_path == path) {
        return;
    }
    let display_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string();
    refs.push(ReferenceImage {
        local_path: path.clone(),
        display_name,
        mime_type: utils::mime_for_path(&path),
        width: None,
        height: None,
        uploaded_url: None,
        upload_cache_key: None,
        source,
    });
}

pub(crate) fn add_clipboard_reference_image(
    app_weak: &slint::Weak<AppShell>,
    refs: &std::rc::Rc<std::cell::RefCell<Vec<ReferenceImage>>>,
    drafts: &std::rc::Rc<
        std::cell::RefCell<std::collections::HashMap<String, crate::WorkspaceDraft>>,
    >,
    last_sequence: &std::rc::Rc<std::cell::RefCell<u64>>,
) -> bool {
    let sequence = clipboard_reference::clipboard_sequence_number();
    if sequence != 0 && *last_sequence.borrow() == sequence {
        return false;
    }
    let path = match clipboard_reference::save_clipboard_image_png(sequence) {
        Ok(Some(path)) => path,
        Ok(None) => return false,
        Err(e) => {
            tracing::debug!(error = %e, "读取剪贴板参考图失败");
            return false;
        }
    };
    if sequence != 0 {
        *last_sequence.borrow_mut() = sequence;
    }
    let display = path.display().to_string();
    {
        let mut ri = refs.borrow_mut();
        push_reference_path(&mut ri, path, ReferenceImageSource::UserPicked);
    }
    if let Some(app) = app_weak.upgrade() {
        let s = app.global::<AppState>();
        push_ws_ref_images(&s, &refs.borrow());
        save_workspace_draft(&s, &refs.borrow(), drafts);
        s.set_status_text(format!("已从剪贴板加入参考图: {display}").into());
    }
    true
}

fn save_template_impl(
    app_weak: &slint::Weak<crate::ui::AppShell>,
    cfg_ref: &std::rc::Rc<std::cell::RefCell<artait_model::AppConfig>>,
) {
    let Some(app) = app_weak.upgrade() else {
        return;
    };
    let s = app.global::<AppState>();
    let prompt = s.get_ws_prompt().to_string();
    if prompt.trim().is_empty() {
        s.set_status_text("提示词为空，不保存".into());
        return;
    }
    let page = s.get_current_page().to_string();
    let name = if s.get_ws_template_name().trim().is_empty() {
        chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
    } else {
        s.get_ws_template_name().to_string()
    };
    let category = s.get_ws_template_active_category().to_string();
    match write_prompt_template(
        &cfg_ref.borrow(),
        &page,
        &category,
        &name,
        "txt",
        &prompt,
        "",
        None,
    ) {
        Ok(dest) => {
            s.set_status_text(format!("模板已保存 → {}", dest.display()).into());
            refresh_template_model(&s, &cfg_ref.borrow(), &page);
            s.set_ws_show_templates(true);
        }
        Err(e) => s.set_status_text(format!("保存失败: {e}").into()),
    }
}

fn submit_template_impl(
    app_weak: &slint::Weak<crate::ui::AppShell>,
    cfg_ref: &std::rc::Rc<std::cell::RefCell<artait_model::AppConfig>>,
    ref_images: &std::rc::Rc<std::cell::RefCell<Vec<ReferenceImage>>>,
    drafts: &std::rc::Rc<
        std::cell::RefCell<std::collections::HashMap<String, crate::WorkspaceDraft>>,
    >,
) {
    let Some(app) = app_weak.upgrade() else {
        return;
    };
    let s = app.global::<AppState>();
    let page = s.get_current_page().to_string();
    let name = s.get_prompt_template_name().to_string();
    let category = s.get_prompt_template_category().to_string();
    let format = s.get_prompt_template_format().to_string();
    let content = s.get_prompt_template_content().to_string();
    let negative = s.get_prompt_template_negative().to_string();
    let original = if s.get_prompt_template_editing() {
        let v = s.get_prompt_template_original_file().to_string();
        (!v.trim().is_empty()).then_some(v)
    } else {
        None
    };
    match write_prompt_template(
        &cfg_ref.borrow(),
        &page,
        &category,
        &name,
        &format,
        &content,
        &negative,
        original.as_deref(),
    ) {
        Ok(dest) => {
            let rel = prompt_template_relative_path(&cfg_ref.borrow(), &page, &dest);
            let cat = template_category_from_file(&rel);
            let label = template_label_from_file(&rel);
            s.set_ws_template_file(rel.into());
            s.set_ws_template_name(label.clone().into());
            s.set_ws_template_category(cat.clone().into());
            s.set_ws_template_active_category(cat.into());
            refresh_template_model(&s, &cfg_ref.borrow(), &page);
            save_workspace_draft(&s, &ref_images.borrow(), &drafts);
            s.set_ws_show_templates(true);
            s.set_prompt_template_dialog_open(false);
            s.set_prompt_template_show_categories(false);
            s.set_prompt_template_error("".into());
            s.set_status_text(format!("模板已保存并选中 → {label}").into());
        }
        Err(e) => s.set_prompt_template_error(format!("保存失败: {e}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::should_skip_text_sidecar_for_prompt_optimization;

    #[test]
    fn image_prompt_optimization_skips_text_only_sidecar() {
        assert!(should_skip_text_sidecar_for_prompt_optimization("image"));
        assert!(!should_skip_text_sidecar_for_prompt_optimization("text"));
    }
}
