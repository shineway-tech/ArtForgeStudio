//! 图片生成回调：提交生图任务、批量模式、结果回调。

use std::time::Duration;

use artait_model::{
    AssetPurpose, ColorMoodPreset, CreationMode, DirectorControls, GameViewPreset, LightingPreset,
    ReferenceImage, ReferenceImageSource, TaskKind, TimeOfDayPreset, WeatherPreset,
};
use artait_service::director_prompt::append_director_prompt;
use artait_service::generation::{generation_task_meta, run_image_generation};
use artait_service::prompt_template::build_generation_prompt;
use artait_task::TaskSpec;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::CbCtx;
use crate::generation::set_gallery_generating_count_for_mode;
use crate::ui::{AppShell, AppState};
use crate::{debug_log, save_workspace_draft};
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
    let rt_handle = ctx.rt_handle.clone();
    let task_meta_map_gen = ctx.task_meta_map.clone();
    let app_weak_turnaround = app_weak.clone();
    let cfg_turnaround = cfg_ref.clone();
    let registry_turnaround = registry.clone();
    let runner_turnaround = runner.clone();
    let http_turnaround = http.clone();
    let task_meta_turnaround = task_meta_map_gen.clone();
    let rt_turnaround = rt_handle.clone();

    state.on_clear_prompt_history({
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_prompt_history(ModelRc::new(VecModel::from(
                    Vec::<slint::SharedString>::new(),
                )));
                state.set_status_text("已清空提示词历史".into());
            }
            cfg_ref.borrow_mut().prompt_history.clear();
            crate::persist(&cfg_ref.borrow());
        }
    });

    state.on_generate_character_turnaround_from_asset(move |path| {
        let requested_path = path.to_string();
        let output_root = cfg_turnaround.borrow().paths.output_dir.clone();
        let resolved =
            resolve_asset_path_for_turnaround(&app_weak_turnaround, &requested_path, &output_root);
        let path = resolved
            .path
            .canonicalize()
            .unwrap_or_else(|_| resolved.path.clone());
        tracing::info!(
            requested = %requested_path,
            resolved = %path.display(),
            is_file = path.is_file(),
            is_dir = path.is_dir(),
            "右键三视图参考图路径"
        );
        if !path.exists() {
            let checked = resolved.checked.join(" | ");
            let checked_summary = summarize_checked_paths(&resolved.checked);
            tracing::warn!(
                requested = %requested_path,
                checked = %checked,
                "生成三视图失败：参考图不存在"
            );
            debug_log(format!(
                "turnaround reference missing -> requested={requested_path}; checked={checked}"
            ));
            if let Some(app) = app_weak_turnaround.upgrade() {
                app.global::<AppState>().set_status_text(
                    format!(
                        "参考图不存在: {} · 已检查 {}",
                        requested_path, checked_summary
                    )
                    .into(),
                );
            }
            return;
        }
        tracing::info!(
            requested = %requested_path,
            resolved = %path.display(),
            "生成三视图参考图已解析"
        );

        let Some(app) = app_weak_turnaround.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_turnaround_source_path(path.display().to_string().into());
        let mode = CreationMode::Character;
        let meta = artait_service::assets::read_asset_metadata(
            &path,
            path.file_name().and_then(|s| s.to_str()),
            None,
            Some("character"),
        );
        let manual_prompt = if !meta.prompt.trim().is_empty() {
            meta.prompt
        } else if !state.get_ws_prompt().trim().is_empty() {
            state.get_ws_prompt().to_string()
        } else {
            "基于参考图生成角色三视图，保持角色设计一致，输出正面、侧面、背面完整设定图".into()
        };
        let selected_template = state.get_ws_template_file().to_string();
        let mut director_controls = director_controls_from_state(&state, mode);
        director_controls.purpose = Some(AssetPurpose::CharacterTurnaround);
        let prompt = match build_final_generation_prompt(
            &cfg_turnaround.borrow(),
            mode,
            &selected_template,
            &manual_prompt,
            &director_controls,
        ) {
            Ok(prompt) => prompt,
            Err(e) => {
                state.set_status_text(format!("读取目录提示词失败: {e}").into());
                return;
            }
        };
        let inst = {
            let c = cfg_turnaround.borrow();
            c.provider_defaults
                .generation
                .as_deref()
                .and_then(|id| c.providers.iter().find(|p| p.id == id).cloned())
        };
        let Some(inst) = inst else {
            state.set_status_text("未设置默认生图 provider".into());
            return;
        };

        let display_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("reference")
            .to_string();
        let refs = vec![ReferenceImage {
            local_path: path.clone(),
            display_name,
            mime_type: utils::mime_for_path(&path),
            width: None,
            height: None,
            uploaded_url: None,
            upload_cache_key: None,
            source: ReferenceImageSource::AddedFromAssetBrowser,
        }];
        // let aspect = state.get_ws_aspect().to_string();     走输入框设置
        let aspect = "16:9".to_string();
        let quality = state.get_ws_quality().to_string();
        let output_dir = cfg_turnaround
            .borrow()
            .paths
            .output_subdir(mode.output_subdir());
        let upload_cfg = cfg_turnaround.borrow().image_upload.clone();
        let request_metadata = serde_json::json!({ "director_controls": director_controls });
        let task_meta = generation_task_meta(
            &inst,
            &prompt,
            &output_dir,
            mode,
            &aspect,
            &quality,
            1,
            &upload_cfg,
            &request_metadata,
        );

        let registry = registry_turnaround.clone();
        let http_for_provider = http_turnaround.clone();
        let app_weak_inner = app_weak_turnaround.clone();
        let director_controls_task = director_controls.clone();
        let source_path_for_ui = path.display().to_string();
        let label = format!("生成 · 角色三视图 · {}", utils::short(&prompt, 24));
        let task_id = runner_turnaround.spawn(
            TaskSpec::new(label, TaskKind::Image).with_timeout(Duration::from_secs(300)),
            move |ctx| async move {
                let file_prefix = format!(
                    "{}-{}",
                    utils::short_safe(AssetPurpose::CharacterTurnaround.id(), 18),
                    utils::short_safe(&prompt, 24)
                );
                let result = run_image_generation(
                    &inst,
                    &prompt,
                    &output_dir,
                    &file_prefix,
                    mode,
                    &aspect,
                    &quality,
                    1,
                    1,
                    &director_controls_task,
                    &refs,
                    &registry,
                    http_for_provider,
                    &ctx,
                )
                .await;
                ctx.progress(1.0);
                match result {
                    Ok(info) => {
                        let path_str = info.path.display().to_string();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(app) = app_weak_inner.upgrade() {
                                let state = app.global::<AppState>();
                                if state.get_turnaround_source_path().as_str()
                                    == source_path_for_ui.as_str()
                                {
                                    state.set_turnaround_source_path("".into());
                                }
                                state.set_ws_last_output(path_str.clone().into());
                                state
                                    .set_status_text(format!("三视图生成完成 → {path_str}").into());
                            }
                        });
                        Ok(())
                    }
                    Err(e) => {
                        let err = e.to_string();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(app) = app_weak_inner.upgrade() {
                                let state = app.global::<AppState>();
                                if state.get_turnaround_source_path().as_str()
                                    == source_path_for_ui.as_str()
                                {
                                    state.set_turnaround_source_path("".into());
                                }
                                state.set_status_text(format!("三视图生成失败: {err}").into());
                            }
                        });
                        Err(e)
                    }
                }
            },
        );
        let meta_map = task_meta_turnaround.clone();
        rt_turnaround.spawn(async move {
            meta_map.lock().await.insert(task_id, task_meta);
        });
        state.set_gallery_generating_count(1);
        set_gallery_generating_count_for_mode(&state, mode.route_id(), 1);
        state.set_status_text(format!("已提交三视图生成任务 · 参考图 {}", path.display()).into());
    });

    state.on_generate_image(move |mode, prompt, aspect, quality, count| {
        let mode = CreationMode::from_route(&mode.to_string());
        let manual_prompt = prompt.to_string();
        let aspect = aspect.to_string();
        let quality = quality.to_string();
        let count = count.clamp(1, 4) as u32;
        debug_log(format!(
            "generate requested -> mode={}, aspect={aspect}, quality={quality}, count={count}, prompt_chars={}",
            mode.route_id(),
            manual_prompt.chars().count()
        ));
        let selected_template = app_weak.upgrade().map(|a| {
            let s = a.global::<AppState>();
            save_workspace_draft(&s, &ref_images.borrow(), &drafts);
            s.get_ws_template_file().to_string()
        }).unwrap_or_default();
        let director_controls = app_weak
            .upgrade()
            .map(|a| director_controls_from_state(&a.global::<AppState>(), mode))
            .unwrap_or_default();
        let request_metadata = serde_json::json!({ "director_controls": director_controls });
        let prompt = match build_final_generation_prompt(
            &cfg_ref.borrow(),
            mode,
            &selected_template,
            &manual_prompt,
            &director_controls,
        ) {
            Ok(prompt) => prompt,
            Err(e) => {
                    debug_log(format!("generate blocked: read prompt template failed -> {e}"));
                    if let Some(app) = app_weak.upgrade() {
                        app.global::<AppState>().set_status_text(format!("读取目录提示词失败: {e}").into());
                    }
                    return;
            }
        };
        if prompt.trim().is_empty() {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().set_status_text("请先选择目录提示词或输入提示词".into());
            }
            return;
        }

        if let Some(app) = app_weak.upgrade() {
            let history = record_prompt_history(&app.global::<AppState>(), &manual_prompt);
            cfg_ref.borrow_mut().prompt_history = history;
            crate::persist(&cfg_ref.borrow());
        }

        let inst = {
            let c = cfg_ref.borrow();
            c.provider_defaults.generation.as_deref()
                .and_then(|id| c.providers.iter().find(|p| p.id == id).cloned())
        };
        let Some(inst) = inst else {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().set_status_text("未设置默认生图 provider".into());
            }
            return;
        };

        let output_dir = cfg_ref.borrow().paths.output_subdir(mode.output_subdir());
        tracing::info!(mode = %mode.route_id(), output_dir = %output_dir.display(), "submit generate");

        let registry = registry.clone();
        let http_for_provider = http.clone();
        let app_weak_inner = app_weak.clone();
        let label = format!("生成 · {} · {}", mode.display_name(), utils::short(&prompt, 24));
        let refs: Vec<ReferenceImage> = ref_images.borrow().clone();

        // ── 批量模式 ──────────────────────────────────────────────
        if mode == CreationMode::ActionSequence {
            let skip_existing = app_weak.upgrade()
                .map(|a| a.global::<AppState>().get_ws_skip_existing()).unwrap_or(false);
            let lines: Vec<String> = prompt.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            if lines.is_empty() { return; }
            let count = lines.len();
            for (i, line) in lines.into_iter().enumerate() {
                let sub_label = format!("批量 {}/{} · {}", i + 1, count, utils::short(&line, 20));
                let out_dir = output_dir.clone();
                let inst_c = inst.clone();
                let reg_c = registry.clone();
                let http_p = http_for_provider.clone();
                let aw = app_weak_inner.clone();
                let asp = aspect.clone();
                let qual = quality.clone();
                let refs_c = refs.clone();
                let skip = skip_existing;
                let out_dir_check = out_dir.clone();
                let line_idx = i + 1;
                runner.spawn(
                    TaskSpec::new(sub_label, TaskKind::Image).with_timeout(Duration::from_secs(300)),
                    move |ctx| async move {
                        if skip {
                            let has_existing = std::fs::read_dir(&out_dir_check).map(|entries| {
                                entries.flatten().any(|e| e.file_name().to_string_lossy().starts_with(&format!("batch-{}", line_idx)))
                            }).unwrap_or(false);
                            if has_existing {
                                ctx.info(format!("跳过 batch-{}（已存在）", line_idx));
                                ctx.progress(1.0);
                                return Ok(());
                            }
                        }
                        let result = run_image_generation(
                            &inst_c, &line, &out_dir, &format!("batch-{}", i + 1),
                            CreationMode::ActionSequence, &asp, &qual, count as u32, line_idx as u32,
                            &DirectorControls::default(),
                            &refs_c, &reg_c, http_p, &ctx,
                        ).await;
                        ctx.progress(1.0);
                        match result {
                            Ok(info) => {
                                let ps = info.path.display().to_string();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(app) = aw.upgrade() {
                                        app.global::<AppState>().set_ws_last_output(ps.clone().into());
                                        app.global::<AppState>().set_status_text(format!("批量完成 → {ps}").into());
                                    }
                                });
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    },
                );
            }
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                s.set_gallery_generating_count(1);
                s.set_status_text(format!("已提交 {} 个批量任务", count).into());
            }
            return;
        }

        // ── 单图模式 ──────────────────────────────────────────────
        for image_index in 0..count {
            let task_label = if count > 1 { format!("{label} · {}/{}", image_index + 1, count) } else { label.clone() };
            let registry = registry.clone();
            let inst = inst.clone();
            let http_for_provider = http_for_provider.clone();
            let output_dir = output_dir.clone();
            let app_weak_inner = app_weak_inner.clone();
            let refs = refs.clone();
            let prompt = prompt.clone();
            let aspect = aspect.clone();
            let quality = quality.clone();
            let mode = mode;
            let director_controls = director_controls.clone();
            let upload_cfg = cfg_ref.borrow().image_upload.clone();
            let task_meta = generation_task_meta(
                &inst,
                &prompt,
                &output_dir,
                mode,
                &aspect,
                &quality,
                count,
                &upload_cfg,
                &request_metadata,
            );
            let task_id = runner.spawn(
                TaskSpec::new(task_label, TaskKind::Image).with_timeout(Duration::from_secs(300)),
                move |ctx| async move {
                    let file_prefix = if count > 1 {
                        format!("{}-{}-{}", utils::short_safe(mode.route_id(), 12), image_index + 1, utils::short_safe(&prompt, 24))
                    } else {
                        format!("{}-{}", utils::short_safe(mode.route_id(), 12), utils::short_safe(&prompt, 24))
                    };
                    let result = run_image_generation(
                        &inst, &prompt, &output_dir, &file_prefix,
                        mode, &aspect, &quality, count, image_index + 1,
                        &director_controls,
                        &refs, &registry, http_for_provider, &ctx,
                    ).await;
                    ctx.progress(1.0);
                    match result {
                        Ok(info) => {
                            let path_str = info.path.display().to_string();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(app) = app_weak_inner.upgrade() {
                                    app.global::<AppState>().set_ws_last_output(path_str.clone().into());
                                    app.global::<AppState>().set_status_text(format!("生成完成 → {path_str}").into());
                                }
                            });
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                },
            );
            let meta_map = task_meta_map_gen.clone();
            rt_handle.spawn(async move {
                meta_map.lock().await.insert(task_id, task_meta);
            });
        }
        if let Some(app) = app_weak.upgrade() {
            let msg = if count > 1 {
                format!("已提交 {count} 张生成任务 · 比例 {aspect} · 品质 {quality}")
            } else {
                format!("已提交生成任务 · 比例 {aspect} · 品质 {quality}")
            };
            let state = app.global::<AppState>();
            state.set_gallery_generating_count(1);
            set_gallery_generating_count_for_mode(&state, mode.route_id(), count as i32);
            state.set_status_text(msg.into());
        }
    });

    {
        let app_weak = ctx.app.clone();
        let cfg_ref = ctx.cfg.clone();
        state.on_refresh_workspace_prompt_preview(move |mode| {
            let mode = CreationMode::from_route(&mode.to_string());
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let manual_prompt = s.get_ws_prompt().to_string();
            let selected_template = s.get_ws_template_file().to_string();
            let director_controls = director_controls_from_state(&s, mode);
            match build_final_generation_prompt(
                &cfg_ref.borrow(),
                mode,
                &selected_template,
                &manual_prompt,
                &director_controls,
            ) {
                Ok(prompt) => {
                    s.set_ws_final_prompt_preview(prompt.into());
                    s.set_ws_prompt_preview_open(true);
                }
                Err(e) => {
                    s.set_ws_final_prompt_preview(format!("读取目录提示词失败: {e}").into());
                    s.set_ws_prompt_preview_open(true);
                }
            }
        });
    }
}

pub(crate) fn build_final_generation_prompt(
    cfg: &artait_model::AppConfig,
    mode: CreationMode,
    selected_template: &str,
    manual_prompt: &str,
    director_controls: &DirectorControls,
) -> anyhow::Result<String> {
    if mode == CreationMode::ActionSequence {
        return Ok(manual_prompt.to_string());
    }
    let prompt = build_generation_prompt(cfg, mode.route_id(), selected_template, manual_prompt)?;
    Ok(append_director_prompt(&prompt, mode, director_controls))
}

pub(crate) fn director_controls_from_state(
    state: &AppState,
    mode: CreationMode,
) -> DirectorControls {
    let _ = mode;
    let purpose = AssetPurpose::from_id(&state.get_ws_asset_purpose().to_string());
    DirectorControls {
        purpose,
        color_mood: ColorMoodPreset::from_id(&state.get_ws_color_mood().to_string()),
        game_view: GameViewPreset::from_id(&state.get_ws_game_view().to_string()),
        weather: WeatherPreset::from_id(&state.get_ws_weather().to_string()),
        time_of_day: TimeOfDayPreset::from_id(&state.get_ws_time_of_day().to_string()),
        lighting: LightingPreset::from_id(&state.get_ws_lighting().to_string()),
    }
}

fn record_prompt_history(state: &AppState, prompt: &str) -> Vec<String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return prompt_history_items(state);
    }

    let current = state.get_prompt_history();
    let mut raw_items = vec![prompt.to_string()];
    for i in 0..current.row_count() {
        let Some(item) = current.row_data(i) else {
            continue;
        };
        let item = item.to_string();
        if item.trim().is_empty() || item == prompt {
            continue;
        }
        raw_items.push(item);
        if raw_items.len() >= 20 {
            break;
        }
    }

    let items: Vec<slint::SharedString> = raw_items.iter().cloned().map(Into::into).collect();
    state.set_prompt_history(ModelRc::new(VecModel::from(items)));
    raw_items
}

fn prompt_history_items(state: &AppState) -> Vec<String> {
    let current = state.get_prompt_history();
    let mut items = Vec::new();
    for i in 0..current.row_count() {
        let Some(item) = current.row_data(i) else {
            continue;
        };
        let item = item.trim().to_string();
        if item.is_empty() {
            continue;
        }
        items.push(item);
        if items.len() >= 20 {
            break;
        }
    }
    items
}

struct ResolvedAssetPath {
    path: std::path::PathBuf,
    checked: Vec<String>,
}

fn resolve_asset_path_for_turnaround(
    app_weak: &slint::Weak<AppShell>,
    requested: &str,
    output_root: &std::path::Path,
) -> ResolvedAssetPath {
    let raw = requested.trim().trim_matches('"');
    let mut checked = Vec::new();
    if raw.is_empty() {
        return ResolvedAssetPath {
            path: std::path::PathBuf::new(),
            checked: vec!["<空路径>".to_string()],
        };
    }
    let direct = std::path::PathBuf::from(raw);
    checked.push(direct.display().to_string());
    if direct.exists() {
        return ResolvedAssetPath {
            path: direct,
            checked,
        };
    }

    let slash_normalized = std::path::PathBuf::from(raw.replace('/', "\\"));
    push_checked(&mut checked, &slash_normalized);
    if slash_normalized.exists() {
        return ResolvedAssetPath {
            path: slash_normalized,
            checked,
        };
    }

    if direct.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            let cwd_candidate = cwd.join(&direct);
            push_checked(&mut checked, &cwd_candidate);
            if cwd_candidate.exists() {
                return ResolvedAssetPath {
                    path: cwd_candidate,
                    checked,
                };
            }
        }

        let data_candidate = artait_model::portable_data_dir().join(&direct);
        push_checked(&mut checked, &data_candidate);
        if data_candidate.exists() {
            return ResolvedAssetPath {
                path: data_candidate,
                checked,
            };
        }
    }

    let Some(file_name) = direct.file_name().and_then(|s| s.to_str()) else {
        return ResolvedAssetPath {
            path: direct,
            checked,
        };
    };

    if let Some(app) = app_weak.upgrade() {
        let state = app.global::<AppState>();
        let assets = state.get_assets_all();
        for row in 0..assets.row_count() {
            let Some(item) = assets.row_data(row) else {
                continue;
            };
            let candidate = std::path::PathBuf::from(item.path.as_str());
            if candidate.file_name().and_then(|s| s.to_str()) != Some(file_name) {
                continue;
            }
            push_checked(&mut checked, &candidate);
            if candidate.exists() {
                return ResolvedAssetPath {
                    path: candidate,
                    checked,
                };
            }
        }
    }

    let search_roots = [
        output_root.to_path_buf(),
        artait_model::portable_data_dir().join("out"),
    ];
    for root in search_roots {
        push_checked(&mut checked, &root);
        if let Some(found) = find_file_by_name(&root, file_name, 5, 1_000) {
            push_checked(&mut checked, &found);
            return ResolvedAssetPath {
                path: found,
                checked,
            };
        }
    }

    ResolvedAssetPath {
        path: direct,
        checked,
    }
}

fn push_checked(checked: &mut Vec<String>, path: &std::path::Path) {
    let value = path.display().to_string();
    if !checked.iter().any(|item| item == &value) {
        checked.push(value);
    }
}

fn find_file_by_name(
    root: &std::path::Path,
    file_name: &str,
    max_depth: usize,
    max_entries: usize,
) -> Option<std::path::PathBuf> {
    if !root.exists() || max_depth == 0 {
        return None;
    }

    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > max_entries {
                return None;
            }
            let path = entry.path();
            if path.file_name().and_then(|s| s.to_str()) == Some(file_name) && path.is_file() {
                return Some(path);
            }
            if depth + 1 < max_depth && path.is_dir() {
                stack.push((path, depth + 1));
            }
        }
    }
    None
}

fn summarize_checked_paths(checked: &[String]) -> String {
    const MAX_ITEMS: usize = 5;
    if checked.len() <= MAX_ITEMS {
        return checked.join(" | ");
    }
    format!(
        "{} | ... 共 {} 项",
        checked[..MAX_ITEMS].join(" | "),
        checked.len()
    )
}

// ── 视频生成 ──────────────────────────────────────────────────────────────

pub(crate) fn init_video(ctx: &CbCtx, app: &AppShell) {
    use artait_model::seedance::SeedanceVideoParams;
    use artait_service::generation::video_task_meta;

    let state = app.global::<AppState>();
    let app_weak = ctx.app.clone();
    let cfg_ref = ctx.cfg.clone();
    let registry = ctx.registry.clone();
    let runner = ctx.runner.clone();
    let http = ctx.http.clone();

    let task_meta_map = ctx.task_meta_map.clone();

    state.on_generate_video(move |prompt, aspect, resolution, duration, audio| {
        let manual_prompt = prompt.to_string();
        let aspect = aspect.to_string();
        let resolution = resolution.to_string();
        let duration = duration.clamp(4, 15) as u32;
        let enable_audio = audio;
        debug_log(format!(
            "generate video requested -> prompt_chars={}, aspect={aspect}, resolution={resolution}, duration={duration}s, audio={enable_audio}",
            manual_prompt.chars().count()
        ));

        if manual_prompt.trim().is_empty() {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().set_status_text("请先输入视频提示词".into());
            }
            return;
        }

        let inst = {
            let c = cfg_ref.borrow();
            c.provider_defaults.video.as_deref()
                .and_then(|id| c.providers.iter().find(|p| p.id == id).cloned())
        };
        let Some(inst) = inst else {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().set_status_text("未设置默认视频 provider".into());
            }
            return;
        };

        let output_dir = {
            let c = cfg_ref.borrow();
            if let Some(ref pid) = c.last_project {
                if let Some(entry) = c.projects.iter().find(|p| p.id == *pid) {
                    std::path::Path::new(&entry.path).join("videos")
                } else {
                    c.paths.output_dir.join("videos")
                }
            } else {
                c.paths.output_dir.join("videos")
            }
        };
        tracing::info!(output_dir = %output_dir.display(), "submit video generate");

        let registry = registry.clone();
        let http_for_provider = http.clone();
        let app_weak_inner = app_weak.clone();

        let label = format!("视频 · {}", artait_service::utils::short(&manual_prompt, 24));
        let video_model = inst.models.video_model.clone().unwrap_or_else(|| "doubao-seedance-1-5-pro-251215".into());

        let seedance_params = SeedanceVideoParams {
            model: video_model,
            prompt: manual_prompt.clone(),
            resolution: resolution.clone(),
            aspect_ratio: aspect.clone(),
            duration_secs: duration,
            camera_fixed: false,
            enable_audio,
            references: vec![],
            negative_prompt: None,
            count: 1,
        };

        let task_meta = video_task_meta(&inst, &manual_prompt, &output_dir, &resolution, &aspect, duration, enable_audio);
        let task_id = runner.spawn(
            TaskSpec::new(label.clone(), TaskKind::Video).with_timeout(Duration::from_secs(900)),
            move |ctx| async move {
                let file_prefix = format!("video-{}", artait_service::utils::short_safe(&manual_prompt, 24));
                let result = artait_service::generation::run_video_generation(
                    &inst,
                    &manual_prompt,
                    &output_dir,
                    &file_prefix,
                    seedance_params.clone(),
                    &registry,
                    http_for_provider,
                    &ctx,
                ).await;
                ctx.progress(1.0);
                match result {
                    Ok(info) => {
                        let path_str = info.path.display().to_string();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(app) = app_weak_inner.upgrade() {
                                app.global::<AppState>().set_ws_last_output(path_str.clone().into());
                                app.global::<AppState>().set_status_text(format!("视频生成完成 → {path_str}").into());
                            }
                        });
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            },
        );
        if let Some(mut meta_map) = task_meta_map.try_lock().ok() {
            meta_map.insert(task_id, task_meta);
        }
        if let Some(app) = app_weak.upgrade() {
            let state = app.global::<AppState>();
            state.set_gallery_generating_count(1);
            state.set_gallery_generating_video_count(1);
            state.set_status_text(format!("已提交视频生成任务 · {resolution} · {aspect} · {duration}s").into());
        }
    });
}
