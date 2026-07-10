//! 资产浏览、预览、删除、后处理回调。

use std::time::Duration;

use artait_model::{CreationMode, ReferenceImage, ReferenceImageSource, TaskKind};
use artait_task::{TaskError, TaskSpec};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::CbCtx;
use crate::ui::{AppShell, AppState};
use crate::{
    assets, debug_log, find_asset_item, populate_asset_metadata, push_ws_ref_images,
    save_workspace_draft, sync_asset_selection,
};
use artait_service::utils;

pub(crate) fn init(ctx: &CbCtx, app: &AppShell) {
    let state = app.global::<AppState>();
    let app_weak = ctx.app.clone();
    let cfg_ref = ctx.cfg.clone();
    let runner = ctx.runner.clone();
    let ref_images = ctx.ref_images.clone();
    let workspace_drafts = ctx.workspace_drafts.clone();
    let selected_assets = ctx.selected_assets.clone();

    // ── 预览 ───────────────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        state.on_preview_show(move |path, name| {
            let path_str = path.to_string();
            let p = std::path::Path::new(&path_str);
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            if matches!(ext.as_str(), "mp4" | "webm" | "mov" | "mkv") {
                assets::open_with_default(p);
                return;
            }
            let img = slint::Image::load_from_path(p).unwrap_or_default();
            let info = match std::fs::metadata(p) {
                Ok(m) => {
                    let size_kb = m.len() as f64 / 1024.0;
                    if size_kb > 1024.0 {
                        format!("{:.1} MB", size_kb / 1024.0)
                    } else {
                        format!("{:.0} KB", size_kb)
                    }
                }
                Err(_) => String::new(),
            };
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                let asset = find_asset_item(&s.get_assets_all(), &path_str);
                let meta = artait_service::assets::read_asset_metadata(
                    p,
                    asset.as_ref().map(|a| a.name.as_str()),
                    asset.as_ref().map(|a| a.bytes),
                    asset.as_ref().map(|a| a.domain.as_str()),
                );
                s.set_preview_path(path);
                s.set_preview_name(name);
                s.set_preview_image(img);
                s.set_preview_info(info.into());
                s.set_preview_prompt(meta.prompt.into());
                s.set_preview_quality(meta.quality.into());
                s.set_preview_aspect_ratio(meta.aspect_ratio.into());
                s.set_preview_model(meta.model.into());
                s.set_preview_width(meta.width);
                s.set_preview_height(meta.height);
                s.set_preview_bytes(meta.bytes);
                s.set_preview_domain(meta.domain.into());
                s.set_preview_director_summary(meta.director_summary.into());
                s.set_preview_open(true);
            }
        });
    }

    {
        let app_weak = app_weak.clone();
        state.on_open_asset_metadata(move |path| {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                populate_asset_metadata(&s, &path.to_string());
                s.set_asset_meta_open(true);
            }
        });
    }

    {
        let app_weak = app_weak.clone();
        state.on_preview_close(move || {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                s.set_preview_open(false);
                s.set_preview_prompt("".into());
                s.set_preview_quality("".into());
                s.set_preview_aspect_ratio("".into());
                s.set_preview_model("".into());
                s.set_preview_width(0);
                s.set_preview_height(0);
                s.set_preview_bytes(0);
                s.set_preview_domain("".into());
                s.set_preview_director_summary("".into());
            }
        });
    }

    // ── 选择 ───────────────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let selected_assets = selected_assets.clone();
        state.on_asset_clicked(move |path, additive| {
            let path = path.to_string();
            let selected_count = {
                let mut selected = selected_assets.borrow_mut();
                if additive {
                    if !selected.insert(path.clone()) {
                        selected.remove(&path);
                    }
                } else {
                    selected.clear();
                }
                selected.len()
            };
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                sync_asset_selection(&s, &selected_assets.borrow());
                if additive {
                    s.set_status_text(if selected_count > 0 {
                        format!("已选择 {selected_count} 张图片").into()
                    } else {
                        "已取消多选".into()
                    });
                }
            }
        });
    }

    {
        let app_weak = app_weak.clone();
        state.on_asset_open_preview(move |path, name| {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().invoke_preview_show(path, name);
            }
        });
    }

    // ── 刷新 / 打开目录 ──────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_refresh_assets(move || {
            let output_dir = cfg_ref.borrow().paths.output_dir.clone();
            debug_log(format!("refresh assets -> {}", output_dir.display()));
            assets::refresh_once(output_dir, app_weak.clone());
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>()
                    .set_status_text("正在刷新结果浏览...".into());
            }
        });
    }

    {
        let cfg_ref = cfg_ref.clone();
        state.on_open_output_dir(move |mode| {
            let paths = cfg_ref.borrow().paths.clone();
            let dir = if mode.as_str() == "all" {
                paths.output_dir
            } else {
                paths.output_subdir(CreationMode::from_route(&mode.to_string()).output_subdir())
            };
            debug_log(format!(
                "open output dir -> mode={}, dir={}",
                mode,
                dir.display()
            ));
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!(error = %e, dir = %dir.display(), "创建输出目录失败");
                return;
            }
            assets::open_with_default(&dir);
        });
    }

    // ── 添加到参考图 ──────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let ref_images = ref_images.clone();
        let drafts = workspace_drafts.clone();
        state.on_add_output_to_reference(move |path| {
            let p = std::path::PathBuf::from(path.as_str());
            if !p.exists() {
                if let Some(app) = app_weak.upgrade() {
                    app.global::<AppState>()
                        .set_status_text(format!("结果不存在: {}", p.display()).into());
                }
                return;
            }
            let mut ri = ref_images.borrow_mut();
            if !ri.iter().any(|r| r.local_path == p) {
                let display_name = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image")
                    .to_string();
                ri.push(ReferenceImage {
                    local_path: p.clone(),
                    display_name,
                    mime_type: utils::mime_for_path(&p),
                    width: None,
                    height: None,
                    uploaded_url: None,
                    upload_cache_key: None,
                    source: ReferenceImageSource::UserPicked,
                });
            }
            drop(ri);
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                push_ws_ref_images(&s, &ref_images.borrow());
                save_workspace_draft(&s, &ref_images.borrow(), &drafts);
                s.set_status_text(format!("已加入参考图: {}", p.display()).into());
            }
        });
    }

    // ── 文件操作 ──────────────────────────────────────────────────
    {
        state.on_open_asset(move |path| {
            assets::open_with_default(std::path::Path::new(path.as_str()));
        });
    }
    {
        state.on_reveal_asset(move |path| {
            assets::reveal_in_explorer(std::path::Path::new(path.as_str()));
        });
    }
    {
        let app_weak = app_weak.clone();
        state.on_copy_asset_path(move |path| {
            let path_text = path.to_string();
            #[cfg(windows)]
            let result = crate::copy_text_to_clipboard_windows(&path_text);
            #[cfg(not(windows))]
            let result: std::io::Result<()> = Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "unsupported",
            ));
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                match result {
                    Ok(()) => s.set_status_text("已复制文件路径".into()),
                    Err(e) => s.set_status_text(format!("复制路径失败: {e}").into()),
                }
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        state.on_start_asset_drag(move |path| {
            let p = std::path::PathBuf::from(path.as_str());
            if let Err(e) = crate::system_drag::start_file_drag(&p) {
                tracing::warn!(error = %e, path = %p.display(), "文件拖拽失败");
                if let Some(app) = app_weak.upgrade() {
                    app.global::<AppState>()
                        .set_status_text(format!("文件拖拽失败: {e}").into());
                }
            }
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>()
                    .invoke_asset_drag_finished(path.to_string().into());
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        state.on_asset_drag_finished(move |path| {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                if s.get_active_asset_drag_path() == path {
                    s.set_active_asset_drag_path("".into());
                }
            }
        });
    }

    // ── 删除 ──────────────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let selected_assets = selected_assets.clone();
        state.on_request_delete_asset(move |path| {
            let path_text = path.to_string();
            let mut paths: Vec<String> = {
                let selected = selected_assets.borrow();
                if selected.contains(&path_text) {
                    selected.iter().cloned().collect()
                } else {
                    vec![path_text.clone()]
                }
            };
            paths.sort();
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                let first = paths.first().cloned().unwrap_or_else(|| path_text.clone());
                let pending: Vec<slint::SharedString> =
                    paths.iter().map(|p| p.as_str().into()).collect();
                s.set_pending_delete_path(first.into());
                s.set_pending_delete_paths(ModelRc::new(VecModel::from(pending)));
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let selected_assets = selected_assets.clone();
        state.on_delete_asset(move |path| {
            let mut targets: Vec<String> = if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>()
                    .get_pending_delete_paths()
                    .iter()
                    .map(|p| p.to_string())
                    .collect()
            } else {
                Vec::new()
            };
            if targets.is_empty() {
                targets.push(path.to_string());
            }
            targets.sort();
            targets.dedup();

            let mut deleted = 0usize;
            let mut first_error = None;
            for target in &targets {
                match std::fs::remove_file(std::path::PathBuf::from(target)) {
                    Ok(()) => deleted += 1,
                    Err(e) => {
                        tracing::warn!(error = %e, path = %target, "删除失败");
                        if first_error.is_none() {
                            first_error = Some(format!("{target}: {e}"));
                        }
                    }
                }
            }
            {
                let mut s = selected_assets.borrow_mut();
                for target in &targets {
                    s.remove(target);
                }
            }
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                sync_asset_selection(&s, &selected_assets.borrow());
                if let Some(error) = first_error {
                    if deleted > 0 {
                        s.set_status_text(
                            format!("已删除 {deleted} 个文件，部分失败: {error}").into(),
                        );
                    } else {
                        s.set_status_text(format!("删除失败: {error}").into());
                    }
                } else if deleted > 1 {
                    s.set_status_text(format!("已删除 {deleted} 个文件").into());
                } else if let Some(target) = targets.first() {
                    s.set_status_text(format!("已删除 {target}").into());
                }
            }
        });
    }

    // ── 后处理 ────────────────────────────────────────────────────
    {
        let runner = runner.clone();
        state.on_unmult_asset(move |path| {
            let p = std::path::PathBuf::from(path.as_str());
            let label = format!(
                "去黑 · {}",
                p.file_name().and_then(|s| s.to_str()).unwrap_or("image")
            );
            runner.spawn(
                TaskSpec::new(label, TaskKind::Image).with_timeout(Duration::from_secs(60)),
                move |ctx| async move {
                    ctx.info("加载图片");
                    ctx.progress(0.2);
                    ctx.check_cancelled().map_err(|_| TaskError::Cancelled)?;
                    let p = p.clone();
                    let res =
                        tokio::task::spawn_blocking(move || artait_asset::unmult_to_sibling(&p))
                            .await;
                    match res {
                        Ok(Ok(dest)) => {
                            ctx.info(format!("已写入 {}", dest.display()));
                            ctx.progress(1.0);
                            Ok(())
                        }
                        Ok(Err(e)) => Err(TaskError::Failed(format!("{e}"))),
                        Err(e) => Err(TaskError::Failed(format!("{e}"))),
                    }
                },
            );
        });
    }

    {
        let runner = runner.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_remove_bg_asset(move |path| {
            let p = std::path::PathBuf::from(path.as_str());
            let cfg = cfg_ref.borrow();
            let rembg = cfg.remove_background.rembg_endpoint.clone();
            let photoroom_ref = cfg.remove_background.photoroom_secret_ref.clone();
            drop(cfg);
            let service: Option<(String, String)> =
                if let Some(endpoint) = rembg.filter(|e| !e.is_empty()) {
                    Some((format!("Rembg ({endpoint})"), endpoint))
                } else if let Some(ref key) = photoroom_ref {
                    match artait_config::secret_store::get(key) {
                        Ok(Some(api_key)) => Some((format!("PhotoRoom ({key})"), api_key)),
                        _ => None,
                    }
                } else {
                    None
                };

            if service.is_none() {
                let p_clone = p.clone();
                runner.spawn(
                    TaskSpec::new(
                        format!(
                            "去背（fallback 去黑）· {}",
                            p.file_name().and_then(|s| s.to_str()).unwrap_or("image")
                        ),
                        TaskKind::Image,
                    )
                    .with_timeout(Duration::from_secs(60)),
                    move |ctx| async move {
                        ctx.info("未配置去背服务，改用去黑 (unmult)");
                        ctx.progress(0.2);
                        ctx.check_cancelled().map_err(|_| TaskError::Cancelled)?;
                        let res = tokio::task::spawn_blocking(move || {
                            artait_asset::unmult_to_sibling(&p_clone)
                        })
                        .await;
                        match res {
                            Ok(Ok(dest)) => {
                                ctx.info(format!("已写入 {}", dest.display()));
                                ctx.progress(1.0);
                                Ok(())
                            }
                            Ok(Err(e)) => Err(TaskError::Failed(format!("{e}"))),
                            Err(e) => Err(TaskError::Failed(format!("{e}"))),
                        }
                    },
                );
                return;
            }
            let (svc_label, svc_value) = service.unwrap();
            let p_clone = p.clone();
            runner.spawn(
                TaskSpec::new(
                    format!(
                        "去背 · {svc_label} · {}",
                        p.file_name().and_then(|s| s.to_str()).unwrap_or("image")
                    ),
                    TaskKind::Image,
                )
                .with_timeout(Duration::from_secs(120)),
                move |ctx| async move {
                    ctx.info(format!("调用 {svc_label}"));
                    ctx.progress(0.1);
                    ctx.check_cancelled().map_err(|_| TaskError::Cancelled)?;
                    let res = if svc_label.starts_with("Rembg") {
                        artait_asset::postprocess::remove_background_http(
                            &p_clone,
                            artait_asset::postprocess::RemoveBackgroundService::Rembg {
                                endpoint: &svc_value,
                            },
                        )
                        .await
                    } else {
                        artait_asset::postprocess::remove_background_http(
                            &p_clone,
                            artait_asset::postprocess::RemoveBackgroundService::PhotoRoom {
                                api_key: &svc_value,
                            },
                        )
                        .await
                    };
                    match res {
                        Ok(dest) => {
                            ctx.info(format!("已写入 {}", dest.display()));
                            ctx.progress(1.0);
                            Ok(())
                        }
                        Err(e) => Err(TaskError::Failed(format!("{e}"))),
                    }
                },
            );
        });
    }

    {
        let runner = runner.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_perfect_unmult_asset(move |path| {
            let p = std::path::PathBuf::from(path.as_str());
            let cfg = cfg_ref.borrow();
            let rembg = cfg.remove_background.rembg_endpoint.clone();
            let photoroom_ref = cfg.remove_background.photoroom_secret_ref.clone();
            drop(cfg);
            let service: Option<(String, String)> =
                if let Some(endpoint) = rembg.filter(|e| !e.is_empty()) {
                    Some((format!("Rembg ({endpoint})"), endpoint))
                } else if let Some(ref key) = photoroom_ref {
                    match artait_config::secret_store::get(key) {
                        Ok(Some(api_key)) => Some((format!("PhotoRoom ({key})"), api_key)),
                        _ => None,
                    }
                } else {
                    None
                };

            runner.spawn(
                TaskSpec::new(
                    format!(
                        "高级去黑 · {}",
                        p.file_name().and_then(|s| s.to_str()).unwrap_or("image")
                    ),
                    TaskKind::Image,
                )
                .with_timeout(Duration::from_secs(180)),
                move |ctx| async move {
                    let Some((svc_label, svc_value)) = service else {
                        return Err(TaskError::Failed(
                            "高级去黑需要先配置 Rembg 或 PhotoRoom 去背景服务".into(),
                        ));
                    };
                    ctx.info(format!("调用 {svc_label} 并合成 UnMult"));
                    ctx.progress(0.1);
                    ctx.check_cancelled().map_err(|_| TaskError::Cancelled)?;
                    let res = if svc_label.starts_with("Rembg") {
                        artait_asset::postprocess::perfect_unmult_http(
                            &p,
                            artait_asset::postprocess::RemoveBackgroundService::Rembg {
                                endpoint: &svc_value,
                            },
                        )
                        .await
                    } else {
                        artait_asset::postprocess::perfect_unmult_http(
                            &p,
                            artait_asset::postprocess::RemoveBackgroundService::PhotoRoom {
                                api_key: &svc_value,
                            },
                        )
                        .await
                    };
                    match res {
                        Ok(dest) => {
                            ctx.info(format!("已写入 {}", dest.display()));
                            ctx.progress(1.0);
                            Ok(())
                        }
                        Err(e) => Err(TaskError::Failed(format!("{e}"))),
                    }
                },
            );
        });
    }
}
