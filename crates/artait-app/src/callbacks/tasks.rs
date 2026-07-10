//! 任务面板回调：取消、清除、重新获取。

use artait_model::ProviderInstance;
use artait_service::generation::{extract_url_from_error, update_history_completed};
use slint::ComponentHandle;

use super::CbCtx;
use crate::ui::{AppShell, AppState};
use crate::{clear_tasks_from_state, remove_task_from_state};
use artait_service::task_filter::clear_task_label;

pub(crate) fn init(ctx: &CbCtx, app: &AppShell) {
    let state = app.global::<AppState>();
    let app_weak = ctx.app.clone();
    let runner = ctx.runner.clone();
    let rt_handle = ctx.rt_handle.clone();
    let history = ctx.history.clone();
    let registry = ctx.registry.clone();
    let http = ctx.http.clone();
    let cfg = ctx.cfg.clone();

    {
        let app_weak = app_weak.clone();
        state.on_show_request_list(move |mode| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_request_list_mode(mode);
            crate::update_request_list_counts(&state);
            state.set_request_list_open(true);
        });
    }

    // ── 取消单个 ──────────────────────────────────────────────────
    {
        let runner = runner.clone();
        let rt_handle = rt_handle.clone();
        state.on_cancel_task(move |id| {
            let id_str = id.to_string();
            let runner = runner.clone();
            rt_handle.spawn(async move {
                runner.cancel(&id_str).await;
            });
        });
    }

    // ── 取消全部 ──────────────────────────────────────────────────
    {
        let runner = runner.clone();
        let rt_handle = rt_handle.clone();
        state.on_cancel_active_task(move || {
            let runner = runner.clone();
            rt_handle.spawn(async move {
                let active = runner.snapshot().await;
                // 取消所有活跃任务（批量生成 count>1 时会有多个并发）。
                for task in active {
                    runner.cancel(&task.id).await;
                }
            });
        });
    }

    // ── 清除历史 ──────────────────────────────────────────────────
    {
        let history = history.clone();
        let app_weak = app_weak.clone();
        let rt_handle = rt_handle.clone();
        state.on_clear_tasks(move |filter| {
            let filter = filter.to_string();
            let history = history.clone();
            let app_weak = app_weak.clone();
            rt_handle.spawn(async move {
                let removed = {
                    let mut h = history.lock().await;
                    h.remove_by_filter(&filter)
                };
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_weak.upgrade() {
                        let s = app.global::<AppState>();
                        clear_tasks_from_state(&s, &filter);
                        s.set_status_text(
                            format!("已删除 {} 条{}任务记录", removed, clear_task_label(&filter))
                                .into(),
                        );
                    }
                });
            });
        });
    }

    // ── 删除单条请求记录 ───────────────────────────────────────────
    {
        let history = history.clone();
        let app_weak = app_weak.clone();
        let runner = runner.clone();
        let rt_handle = rt_handle.clone();
        state.on_remove_task(move |id| {
            let id = id.to_string();
            let history = history.clone();
            let app_weak = app_weak.clone();
            let runner = runner.clone();
            rt_handle.spawn(async move {
                let removed_history = {
                    let mut h = history.lock().await;
                    h.remove_by_id(&id)
                };
                if !removed_history {
                    runner.cancel(&id).await;
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_weak.upgrade() {
                        let s = app.global::<AppState>();
                        let removed_ui = remove_task_from_state(&s, &id);
                        if removed_history || removed_ui {
                            s.set_status_text("已删除 1 条请求记录".into());
                        }
                    }
                });
            });
        });
    }

    // ── 重新获取 / 重试 ──────────────────────────────────────────
    {
        let history = history.clone();
        let registry = registry.clone();
        let http = http.clone();
        let rt_handle = rt_handle.clone();
        let provider_instances: Vec<ProviderInstance> = cfg.borrow().providers.clone();
        let output_base = cfg.borrow().paths.output_dir.clone();
        state.on_reacquire_task(move |id| {
            let id_str = id.to_string();
            let history = history.clone();
            let registry = registry.clone();
            let http = http.clone();
            let provider_instances = provider_instances.clone();
            let output_base = output_base.clone();
            rt_handle.spawn(async move {
                let entry = { let hg = history.lock().await; hg.get(&id_str).cloned() };
                let Some(entry) = entry else {
                    tracing::warn!(task_id = %id_str, "re-acquire: 未找到任务历史"); return;
                };
                let provider_id = &entry.provider_id;
                let Some(provider) = registry.get(provider_id) else {
                    tracing::warn!(provider_id, "re-acquire: 未找到 provider"); return;
                };
                let mut pctx = artait_provider::ProviderContext::with_http(
                    entry.provider_instance_id.clone(), provider_id.clone(), http.clone());
                pctx.endpoint = if entry.endpoint.is_empty() { None } else { Some(entry.endpoint.clone()) };
                pctx.extra = serde_json::from_str(&entry.extra_json).unwrap_or_default();
                pctx.cancellation = tokio_util::sync::CancellationToken::new();

                // A: 有 provider_task_id → 重新轮询
                if !entry.provider_task_id.is_empty() {
                    let Some(pollable) = provider.as_pollable() else {
                        tracing::warn!(provider_id, "re-acquire: provider 不支持轮询"); return;
                    };
                    tracing::info!(task_id = %id_str, provider_task_id = %entry.provider_task_id, "re-acquire: 开始重新轮询");
                    let strategy = artait_provider::PollingStrategy {
                        interval: std::time::Duration::from_secs(5),
                        max_polls: 30,
                        ..Default::default()
                    };
                    match pollable.poll_until_done(&entry.provider_task_id, &pctx, &strategy).await {
                        Ok(output) => {
                            let dir = std::path::PathBuf::from(&entry.output_path);
                            let saver = artait_task::ResultSaver::new(
                                if dir.is_dir() { dir } else { dir.parent().unwrap_or(&dir).to_path_buf() },
                                format!("reacquire-{}", &entry.label[..entry.label.len().min(16)]), http);
                            match saver.save(output).await {
                                Ok(saved) => {
                                    tracing::info!(path = %saved.path.display(), "re-acquire: 成功获取结果");
                                    update_history_completed(&history, &id_str, &saved.path.display().to_string()).await;
                                    let _ = slint::invoke_from_event_loop(|| {});
                                }
                                Err(e) => tracing::warn!(error = %e, "re-acquire: 保存结果失败"),
                            }
                        }
                        Err(e) => tracing::warn!(error = %e, "re-acquire: 轮询失败"),
                    }
                    return;
                }

                // B: 无 provider_task_id → 先重下URL，不行再重新生成
                let source_url = if !entry.retry_source_url.is_empty() {
                    Some(entry.retry_source_url.clone())
                } else { extract_url_from_error(&entry.error) };

                if let Some(url) = source_url {
                    tracing::info!(task_id = %id_str, url = %url, "retry: 重新下载原始输出 URL");
                    let out = { let p = std::path::PathBuf::from(&entry.output_path);
                        if p.is_dir() || p.to_string_lossy().contains(&['/', '\\'][..])
                        { p.parent().map(|pp| pp.to_path_buf()).unwrap_or_else(|| output_base.clone()) }
                        else { output_base.clone() } };
                    let _ = std::fs::create_dir_all(&out);
                    let saver = artait_task::ResultSaver::new(out,
                        format!("retry-dl-{}", &entry.label[..entry.label.len().min(12)]), http.clone());
                    let output = artait_provider::request::GenerationOutput::Url { url: url.clone(), metadata: serde_json::Value::Null };
                    match saver.save(output).await {
                        Ok(saved) => {
                            tracing::info!(path = %saved.path.display(), "retry: URL 重下成功");
                            update_history_completed(&history, &id_str, &saved.path.display().to_string()).await;
                            let _ = slint::invoke_from_event_loop(|| {}); return;
                        }
                        Err(e) => tracing::warn!(error = %e, "retry: URL 重下也失败，尝试重新生成"),
                    }
                }

                // 最后手段：重新生成
                tracing::info!(task_id = %id_str, prompt = %entry.prompt, "retry: 重新提交生图");
                let extra: serde_json::Value = serde_json::from_str(&entry.extra_json).unwrap_or_default();
                let aspect = extra.get("aspect").and_then(|v| v.as_str()).unwrap_or("1:1").to_string();
                let quality = extra.get("quality").and_then(|v| v.as_str()).unwrap_or("2K").to_string();
                let maybe_inst = provider_instances.iter().find(|p| p.id == entry.provider_instance_id)
                    .or_else(|| provider_instances.iter().find(|p| p.provider_id == entry.provider_id)).cloned();
                if let Some(ref inst) = maybe_inst {
                    if !inst.api_key.as_deref().map(|s| s.trim()).unwrap_or("").is_empty() {
                        pctx.secret = inst.api_key.clone();
                    } else if let Some(key) = inst.secret_ref.as_deref() {
                        pctx.secret = artait_config::secret_store::get(key).ok().flatten().filter(|s| !s.trim().is_empty());
                    }
                }
                let gen = match provider.as_image_generator() {
                    Some(g) => g, None => { tracing::warn!(provider_id, "retry: provider 不支持生图"); return; }
                };
                let req = artait_provider::request::ImageGenerationRequest {
                    prompt: entry.prompt.clone(), negative_prompt: None, reference_images: Vec::new(),
                    aspect_ratio: Some(aspect), resolution: None, size: None, quality: Some(quality),
                    count: 1, mode: artait_model::CreationMode::Scene, action_name: None,
                    metadata: serde_json::Value::Null,
                };
                match gen.generate(req, &pctx).await {
                    Ok(output) => {
                        let dir = { let p = std::path::PathBuf::from(&entry.output_path); if p.is_dir() { p } else { output_base } };
                        let saver = artait_task::ResultSaver::new(dir,
                            format!("retry-{}", &entry.label[..entry.label.len().min(16)]), http);
                        match saver.save(output).await {
                            Ok(saved) => {
                                tracing::info!(path = %saved.path.display(), "retry: 成功生成");
                                update_history_completed(&history, &id_str, &saved.path.display().to_string()).await;
                                let _ = slint::invoke_from_event_loop(|| {});
                            }
                            Err(e) => tracing::warn!(error = %e, "retry: 保存结果失败"),
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "retry: 生成失败"),
                }
            });
        });
    }
}
