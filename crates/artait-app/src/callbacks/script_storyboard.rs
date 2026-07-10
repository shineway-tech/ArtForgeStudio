//! 动画脚本 + 分镜板回调。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use artait_model::{CreationMode, ReferenceImage, TaskKind};
use artait_service::generation::run_image_generation;
use artait_service::{script, script_index};
use artait_task::{TaskError, TaskSpec};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::CbCtx;
use crate::navigate_to_page;
use crate::ui::{
    AppShell, AppState, RefImageItem, ScriptBlockItem, ScriptCharacterSummaryItem, ScriptFileItem,
    ScriptSceneSummaryItem, StoryboardPackageItem, StoryboardShotImageItem, StoryboardShotItem,
};
use artait_service::utils;

pub(crate) fn init(ctx: &CbCtx, app: &AppShell, sb_ref_images: Rc<RefCell<Vec<ReferenceImage>>>) {
    let state = app.global::<AppState>();
    let app_weak = ctx.app.clone();
    let cfg_ref = ctx.cfg.clone();
    let runner = ctx.runner.clone();
    let registry = ctx.registry.clone();
    let http = ctx.http.clone();
    let ref_images = ctx.ref_images.clone();
    let drafts = ctx.workspace_drafts.clone();
    let character_store = ctx.character_store.clone();
    let scene_store = ctx.scene_store.clone();

    // ── 脚本·文档管理 ────────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        state.on_script_add_doc(move || {
            let picked = rfd::FileDialog::new()
                .set_title("选择参考文档")
                .add_filter("文本文档", &["txt", "md"])
                .pick_files()
                .unwrap_or_default();
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let mut docs: Vec<slint::SharedString> = s.get_script_docs().iter().collect();
            for p in picked {
                let ps: slint::SharedString = p.display().to_string().into();
                if !docs.contains(&ps) {
                    docs.push(ps);
                }
            }
            s.set_script_docs(ModelRc::new(VecModel::from(docs)));
        });
    }
    {
        let app_weak = app_weak.clone();
        state.on_script_remove_doc(move |idx| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let mut docs: Vec<slint::SharedString> = s.get_script_docs().iter().collect();
            if (idx as usize) < docs.len() {
                docs.remove(idx as usize);
            }
            s.set_script_docs(ModelRc::new(VecModel::from(docs)));
        });
    }

    {
        let app_weak = app_weak.clone();
        state.on_script_load_format_example(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            s.set_script_mode("import".into());
            s.set_script_theme(script::screenplay_format_example().into());
            s.set_script_parse_status("idle".into());
            s.set_script_parse_summary("已载入标准剧本格式示例，可直接改写后导入解析".into());
        });
    }

    // ── 脚本·生成 ────────────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let registry = registry.clone();
        let runner = runner.clone();
        let http = http.clone();
        state.on_script_generate(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let theme = s.get_script_theme().to_string();
            let step = s.get_script_generating_step().to_string();
            let docs: Vec<std::path::PathBuf> = s
                .get_script_docs()
                .iter()
                .map(|d| std::path::PathBuf::from(d.as_str()))
                .collect();
            let output_dir = cfg_ref.borrow().paths.output_dir.join("animation_scripts");
            let inst = find_analysis_inst(&cfg_ref.borrow());
            let Some(inst) = inst else {
                s.set_script_generating_step("".into());
                s.set_status_text("未设置默认推理 provider".into());
                return;
            };

            let registry = registry.clone();
            let http = http.clone();
            let app_weak_inner = app_weak.clone();
            runner.spawn(
                TaskSpec::new(
                    format!("生成脚本 · {}", utils::short(&theme, 30)),
                    TaskKind::Analysis,
                )
                .with_timeout(Duration::from_secs(180)),
                move |ctx| {
                    let step_clone = step.clone();
                    let app_weak_for_err = app_weak_inner.clone();
                    async move {
                        let result = artait_service::script::generate_script_via_provider(
                            &inst,
                            &theme,
                            &docs,
                            &output_dir,
                            &registry,
                            http,
                            &ctx,
                        )
                        .await;
                        match result {
                            Ok(dest) => {
                                ctx.info(format!("已写入 {}", dest.display()));
                                ctx.progress(1.0);
                                let dest_str = dest.display().to_string();
                                let out_dir = output_dir.clone();
                                let app_weak_for_parse = app_weak_inner.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(app) = app_weak_inner.upgrade() {
                                        let s = app.global::<AppState>();
                                        push_script_files(&s, &out_dir);
                                        select_and_analyze_script(
                                            &s,
                                            &std::path::PathBuf::from(&dest_str),
                                            &out_dir,
                                            &app_weak_for_parse,
                                        );
                                        s.set_script_generating_step("".into());
                                        if step_clone == "format" {
                                            s.set_script_generate_dialog_open(false);
                                        }
                                        s.set_status_text(
                                            format!("脚本已生成 → {dest_str}").into(),
                                        );
                                    }
                                });
                                Ok(())
                            }
                            Err(e) => {
                                let err_msg = format!("{e}");
                                let err_clone = err_msg.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(app) = app_weak_for_err.upgrade() {
                                        let s = app.global::<AppState>();
                                        s.set_script_generating_step("".into());
                                        s.set_status_text(
                                            format!("脚本生成失败：{err_clone}").into(),
                                        );
                                    }
                                });
                                Err(TaskError::Failed(err_msg))
                            }
                        }
                    }
                },
            );
        });
    }

    // ── 剧本导入/解析/转入库 ────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_script_import_current(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let raw = s.get_script_theme().to_string();
            let output_dir = cfg_ref.borrow().paths.output_dir.join("animation_scripts");
            match script::save_imported_script(&raw, &output_dir) {
                Ok(dest) => {
                    push_script_files(&s, &output_dir);
                    select_and_analyze_script(&s, &dest, &output_dir, &app_weak);
                    s.set_status_text(
                        format!("剧本已导入，正在后台解析 → {}", dest.display()).into(),
                    );
                }
                Err(e) => {
                    s.set_script_parse_status("error".into());
                    s.set_script_parse_summary(format!("导入失败：{e}").into());
                    s.set_status_text(format!("剧本导入失败：{e}").into());
                }
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let character_store = character_store.clone();
        state.on_script_export_characters(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let raw = current_script_text(&s);
            if raw.trim().is_empty() {
                s.set_status_text("没有可导出的剧本内容".into());
                return;
            }
            let project_id = optional_project_id(&s);
            match script::extract_character_drafts(&raw, project_id.as_deref()) {
                Ok(chars) => {
                    let mut store = character_store.borrow_mut();
                    let mut created = 0usize;
                    for character in chars {
                        if store.create_character(character).is_ok() {
                            created += 1;
                        }
                    }
                    store.flush();
                    crate::callbacks::character_library::refresh_character_list(
                        &s,
                        store.all_characters(),
                    );
                    s.set_status_text(format!("已从剧本导出 {created} 个角色草稿").into());
                }
                Err(e) => s.set_status_text(format!("角色导出失败：{e}").into()),
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let scene_store = scene_store.clone();
        state.on_script_export_scenes(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let raw = current_script_text(&s);
            if raw.trim().is_empty() {
                s.set_status_text("没有可导出的剧本内容".into());
                return;
            }
            let project_id = optional_project_id(&s);
            match script::extract_scene_drafts(&raw, project_id.as_deref()) {
                Ok(scenes) => {
                    let mut store = scene_store.borrow_mut();
                    let mut created = 0usize;
                    for scene in scenes {
                        if store.create_scene(scene).is_ok() {
                            created += 1;
                        }
                    }
                    store.flush();
                    crate::callbacks::scene_library::refresh_scene_list(&s, store.all_scenes());
                    s.set_status_text(format!("已从剧本导出 {created} 个场景草稿").into());
                }
                Err(e) => s.set_status_text(format!("场景导出失败：{e}").into()),
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let registry = registry.clone();
        let runner = runner.clone();
        let http = http.clone();
        state.on_script_run_stage(move |stage| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            match stage.as_str() {
                "parse" => {
                    let raw = current_script_text(&s);
                    let selected = s.get_script_selected().to_string();
                    if selected.trim().is_empty() {
                        spawn_script_parse(&app_weak, raw, None);
                    } else {
                        spawn_script_parse(
                            &app_weak,
                            raw,
                            Some(std::path::PathBuf::from(selected)),
                        );
                    }
                }
                "shot" => {
                    let raw = current_script_text(&s);
                    if raw.trim().is_empty() {
                        s.set_status_text("没有可拆分的剧本内容".into());
                    } else {
                        let pkgs = script::split_storyboard_packages(&raw, 3);
                        let items: Vec<StoryboardPackageItem> = pkgs
                            .iter()
                            .map(|p| StoryboardPackageItem {
                                index: p.index as i32,
                                label: p.label.clone().into(),
                                shot_count: p.shot_count as i32,
                                markdown: p.markdown.clone().into(),
                            })
                            .collect();
                        s.set_script_packages(ModelRc::new(VecModel::from(items)));
                        s.set_status_text(
                            format!("分镜校准入口：已拆分 {} 个分镜包", pkgs.len()).into(),
                        );
                    }
                }
                "character" => s.invoke_script_export_characters(),
                "scene" => s.invoke_script_export_scenes(),
                "scene_calibrate" => {
                    let raw = current_script_text(&s);
                    if raw.trim().is_empty() {
                        s.set_status_text("没有可校准的剧本内容".into());
                        return;
                    }
                    let Some(inst) = find_analysis_inst(&cfg_ref.borrow()) else {
                        s.set_status_text("未设置默认推理 provider，无法进行 AI 场景校准".into());
                        return;
                    };
                    let project_id = optional_project_id(&s);
                    let registry = registry.clone();
                    let http = http.clone();
                    let app_weak_inner = app_weak.clone();
                    runner.spawn(
                        TaskSpec::new(String::from("AI 场景校准"), TaskKind::Analysis)
                            .with_timeout(Duration::from_secs(180)),
                        move |ctx| async move {
                            let scenes =
                                artait_service::script::calibrate_scene_drafts_via_provider(
                                    &inst,
                                    &raw,
                                    project_id.as_deref(),
                                    &registry,
                                    http,
                                    &ctx,
                                )
                                .await
                                .map_err(|e| TaskError::Failed(format!("{e}")))?;
                            let count = scenes.len();
                            let mut disk_store =
                                artait_service::scene_store::SceneStore::load_or_default();
                            let mut created = 0usize;
                            for scene in scenes {
                                if disk_store.create_scene(scene).is_ok() {
                                    created += 1;
                                }
                            }
                            disk_store.flush();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(app) = app_weak_inner.upgrade() {
                                    let s = app.global::<AppState>();
                                    let store =
                                        artait_service::scene_store::SceneStore::load_or_default();
                                    crate::callbacks::scene_library::refresh_scene_list(
                                        &s,
                                        store.all_scenes(),
                                    );
                                    s.set_status_text(
                                        format!("AI 场景校准完成：已入库 {created}/{count} 个场景")
                                            .into(),
                                    );
                                }
                            });
                            ctx.progress(1.0);
                            Ok(())
                        },
                    );
                }
                _ => s.set_status_text(format!("未知剧本阶段：{stage}").into()),
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_script_save_current(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let raw = s.get_script_content().to_string();
            let output_dir = cfg_ref.borrow().paths.output_dir.join("animation_scripts");
            let selected = s.get_script_selected().to_string();
            let selected_path =
                (!selected.trim().is_empty()).then(|| std::path::PathBuf::from(&selected));
            match script::save_script(&raw, selected_path.as_deref(), &output_dir) {
                Ok(dest) => {
                    select_and_analyze_script(&s, &dest, &output_dir, &app_weak);
                    s.set_status_text(format!("剧本已保存 → {}", dest.display()).into());
                }
                Err(e) => {
                    s.set_script_parse_status("error".into());
                    s.set_script_parse_summary(format!("保存失败：{e}").into());
                    s.set_status_text(format!("剧本保存失败：{e}").into());
                }
            }
        });
    }

    // ── 脚本·选择/拆分/发送/打开目录 ─────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        state.on_script_select(move |path| {
            let p = std::path::PathBuf::from(path.as_str());
            let output_dir = cfg_ref.borrow().paths.output_dir.join("animation_scripts");
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                select_and_analyze_script(&s, &p, &output_dir, &app_weak);
                s.set_script_selected(path);
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        state.on_script_split_packages(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let raw = s.get_script_content().to_string();
            if raw.is_empty() {
                return;
            }
            let pkgs = script::split_storyboard_packages(&raw, 3);
            let items: Vec<StoryboardPackageItem> = pkgs
                .iter()
                .map(|p| StoryboardPackageItem {
                    index: p.index as i32,
                    label: p.label.clone().into(),
                    shot_count: p.shot_count as i32,
                    markdown: p.markdown.clone().into(),
                })
                .collect();
            s.set_script_packages(ModelRc::new(VecModel::from(items)));
            s.set_status_text(format!("已拆分 {} 个分镜包", pkgs.len()).into());
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let ref_images = ref_images.clone();
        let drafts = drafts.clone();
        state.on_script_send_package(move |idx| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            let pkgs = s.get_script_packages();
            if let Some(pkg) = pkgs.iter().nth(idx as usize) {
                let item = StoryboardPackageItem {
                    index: pkg.index,
                    label: pkg.label.clone(),
                    shot_count: pkg.shot_count,
                    markdown: pkg.markdown.clone(),
                };
                let prompt = default_storyboard_prompt(&item.label, &item.markdown);
                s.set_sb_packages(ModelRc::new(VecModel::from(vec![item])));
                s.set_sb_selected(0);
                s.set_sb_shot_images(ModelRc::new(VecModel::from(
                    Vec::<StoryboardShotImageItem>::new(),
                )));
                s.set_sb_generating_shot(-1);
                refresh_storyboard_shots(&s);
                s.set_sb_prompt(prompt.into());
                navigate_to_page(&s, &cfg_ref.borrow(), &ref_images, &drafts, "storyboard");
                s.set_status_text(format!("分镜包 {} 已发送到分镜板", pkg.label).into());
            }
        });
    }
    {
        let cfg_ref = cfg_ref.clone();
        state.on_script_open_dir(move || {
            let dir = cfg_ref.borrow().paths.output_dir.join("animation_scripts");
            std::fs::create_dir_all(&dir).ok();
            crate::assets::open_with_default(&dir);
        });
    }
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let ref_images = ref_images.clone();
        let drafts = drafts.clone();
        state.on_script_send_to_storyboard(move |content| {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                let md = content.to_string();
                let packages = script::split_storyboard_packages(&md, 3);
                let items: Vec<StoryboardPackageItem> = packages
                    .into_iter()
                    .map(|p| StoryboardPackageItem {
                        index: p.index as i32,
                        label: p.label.into(),
                        shot_count: p.shot_count as i32,
                        markdown: p.markdown.into(),
                    })
                    .collect();
                let prompt = items
                    .first()
                    .map(|p| default_storyboard_prompt(&p.label, &p.markdown))
                    .unwrap_or_default();
                s.set_sb_packages(ModelRc::new(VecModel::from(items)));
                s.set_sb_selected(0);
                s.set_sb_shot_images(ModelRc::new(VecModel::from(
                    Vec::<StoryboardShotImageItem>::new(),
                )));
                s.set_sb_generating_shot(-1);
                refresh_storyboard_shots(&s);
                s.set_sb_prompt(prompt.into());
                navigate_to_page(&s, &cfg_ref.borrow(), &ref_images, &drafts, "storyboard");
                s.set_status_text("脚本已拆分并发送到分镜板".into());
            }
        });
    }

    // ── 脚本·文件管理 ───────────────────────────────────────────────
    let output_dir = cfg_ref.borrow().paths.output_dir.join("animation_scripts");
    {
        let s = app.global::<AppState>();
        refresh_script_files(&s, &output_dir);
        open_latest_script_if_needed(&s, &output_dir, &app_weak);
    }
    {
        let app_weak = app_weak.clone();
        let output_dir = output_dir.clone();
        state.on_script_rename_file(move |path| {
            let old = std::path::PathBuf::from(path.as_str());
            let picked = rfd::FileDialog::new()
                .set_title("重命名脚本")
                .set_directory(&output_dir)
                .set_file_name(
                    old.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("script.md"),
                )
                .add_filter("Markdown", &["md"])
                .save_file();
            let Some(new_path) = picked else { return };
            if new_path == old {
                return;
            }
            match std::fs::rename(&old, &new_path) {
                Ok(()) => {
                    if let Some(app) = app_weak.upgrade() {
                        let s = app.global::<AppState>();
                        push_script_files(&s, &output_dir);
                        s.set_status_text(format!("已重命名 → {}", new_path.display()).into());
                    }
                }
                Err(e) => {
                    if let Some(app) = app_weak.upgrade() {
                        app.global::<AppState>()
                            .set_status_text(format!("重命名失败: {e}").into());
                    }
                }
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let output_dir = output_dir.clone();
        state.on_script_delete_file(move |path| {
            let p = std::path::PathBuf::from(path.as_str());
            match std::fs::remove_file(&p) {
                Ok(()) => {
                    if let Some(app) = app_weak.upgrade() {
                        let s = app.global::<AppState>();
                        push_script_files(&s, &output_dir);
                        s.set_script_selected("".into());
                        s.set_script_content("".into());
                        s.set_script_content_plain("".into());
                        s.set_script_blocks(ModelRc::new(VecModel::from(
                            Vec::<ScriptBlockItem>::new(),
                        )));
                        clear_script_structure(&s);
                        s.set_status_text(format!("已删除 {}", path).into());
                    }
                }
                Err(e) => {
                    if let Some(app) = app_weak.upgrade() {
                        app.global::<AppState>()
                            .set_status_text(format!("删除失败: {e}").into());
                    }
                }
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let output_dir = output_dir.clone();
        state.on_script_refresh(move || {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                refresh_script_files(&s, &output_dir);
                open_latest_script_if_needed(&s, &output_dir, &app_weak);
                s.set_status_text("脚本列表已刷新".into());
            }
        });
    }

    // ── 分镜板 ─────────────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        state.on_sb_select(move |idx| {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                s.set_sb_selected(idx);
                let packages = s.get_sb_packages();
                if let Some(pkg) = packages.iter().nth(idx as usize) {
                    refresh_storyboard_shots(&s);
                    refresh_current_shot_image(&s);
                    s.set_sb_prompt(default_storyboard_prompt(&pkg.label, &pkg.markdown).into());
                }
            }
        });
    }

    {
        let app_weak = app_weak.clone();
        state.on_sb_select_shot(move |idx| {
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                s.set_sb_selected_shot(idx);
                let shots = s.get_sb_shots();
                if let Some(shot) = shots.iter().nth(idx as usize) {
                    s.set_sb_shot_prompt(
                        default_storyboard_prompt(&shot.label, &shot.markdown).into(),
                    );
                    s.set_sb_shot_note("".into());
                }
                refresh_current_shot_image(&s);
            }
        });
    }

    {
        let app_weak = app_weak.clone();
        let sb_ref = sb_ref_images.clone();
        state.on_sb_add_reference_images(move || {
            let picked = rfd::FileDialog::new()
                .set_title("选择分镜参考图")
                .add_filter("图片", &["png", "jpg", "jpeg", "webp"])
                .pick_files()
                .unwrap_or_default();
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut ri = sb_ref.borrow_mut();
            for p in picked {
                if !ri.iter().any(|r| r.local_path == p) {
                    let display_name = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("image")
                        .to_string();
                    let mime = utils::mime_for_path(&p);
                    ri.push(ReferenceImage {
                        local_path: p,
                        display_name,
                        mime_type: mime,
                        width: None,
                        height: None,
                        uploaded_url: None,
                        upload_cache_key: None,
                        source: artait_model::ReferenceImageSource::UserPicked,
                    });
                }
            }
            drop(ri);
            push_sb_ref_model(&sb_ref, &app.global::<AppState>());
        });
    }
    {
        let app_weak = app_weak.clone();
        let sb_ref = sb_ref_images.clone();
        state.on_sb_remove_reference_image(move |idx| {
            let i = idx as usize;
            let mut ri = sb_ref.borrow_mut();
            if i < ri.len() {
                ri.remove(i);
            }
            drop(ri);
            if let Some(app) = app_weak.upgrade() {
                push_sb_ref_model(&sb_ref, &app.global::<AppState>());
            }
        });
    }
    {
        let app_weak = app_weak.clone();
        let sb_ref = sb_ref_images.clone();
        state.on_sb_clear_reference_images(move || {
            sb_ref.borrow_mut().clear();
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>()
                    .set_sb_ref_images(ModelRc::new(VecModel::from(Vec::<RefImageItem>::new())));
            }
        });
    }

    // ── 分镜·生成 ──────────────────────────────────────────────────
    {
        let app_weak = app_weak.clone();
        let cfg_ref = cfg_ref.clone();
        let registry = registry.clone();
        let runner = runner.clone();
        let http = http.clone();
        let sb_ref = sb_ref_images.clone();
        state.on_sb_generate(move || {
            let Some(app) = app_weak.upgrade() else { return };
            let s = app.global::<AppState>();
            let idx = s.get_sb_selected() as usize;
            let packages = s.get_sb_packages();
            let Some(pkg) = packages.iter().nth(idx) else { return };
            let shot_idx = s.get_sb_selected_shot() as usize;
            let shots = s.get_sb_shots();
            let selected_shot = shots.iter().nth(shot_idx);
            let prompt = selected_shot
                .as_ref()
                .map(|_| s.get_sb_shot_prompt().to_string())
                .filter(|p| !p.trim().is_empty())
                .unwrap_or_else(|| s.get_sb_prompt().to_string());
            let shot_note = s.get_sb_shot_note().to_string();
            let aspect = s.get_sb_aspect().to_string();
            let label_str = selected_shot
                .as_ref()
                .map(|shot| format!("{} · {}", pkg.label, shot.label))
                .unwrap_or_else(|| pkg.label.to_string());
            let md = selected_shot
                .as_ref()
                .map(|shot| shot.markdown.to_string())
                .unwrap_or_else(|| pkg.markdown.to_string());
            let inst = cfg_ref.borrow().provider_defaults.generation.as_deref()
                .and_then(|id| cfg_ref.borrow().providers.iter().find(|p| p.id == id).cloned());
            let Some(inst) = inst else { s.set_status_text("未设置默认生图 provider".into()); return; };
            let output_dir = storyboard_output_dir(&cfg_ref.borrow(), &s);
            let sb_refs: Vec<ReferenceImage> = sb_ref.borrow().clone();
            let full_prompt = match (prompt.trim().is_empty(), shot_note.trim().is_empty()) {
                (true, true) => format!("根据以下分镜描述生成画面：\n{md}"),
                (false, true) => format!("根据以下分镜描述生成画面：\n{md}\n\n风格/构图要求：{prompt}"),
                (true, false) => format!("根据以下分镜描述生成画面：\n{md}\n\n镜头参数备注：{shot_note}"),
                (false, false) => format!("根据以下分镜描述生成画面：\n{md}\n\n风格/构图要求：{prompt}\n\n镜头参数备注：{shot_note}"),
            };

            let registry = registry.clone(); let http_for_provider = http.clone();
            let app_weak_inner = app_weak.clone();
            s.set_sb_generating_shot(shot_idx as i32);
            s.set_status_text(format!("正在生成镜头图：{label_str}").into());
            runner.spawn(
                TaskSpec::new(format!("分镜 · {} · {}", label_str, utils::short(&prompt, 20)), TaskKind::Image)
                    .with_timeout(Duration::from_secs(300)),
                move |ctx| async move {
                    let result = run_image_generation(
                        &inst, &full_prompt, &output_dir, &format!("storyboard-{}", utils::short_safe(&label_str, 20)),
                        CreationMode::Storyboard, &aspect, "2K", 1, 1,
                        &artait_model::DirectorControls::default(),
                        &sb_refs, &registry, http_for_provider, &ctx,
                    ).await;
                    ctx.progress(1.0);
                    match result {
                        Ok(info) => {
                            let saved_path = info.path;
                            let path_str = saved_path.display().to_string();
                            let app_weak_done = app_weak_inner.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(app) = app_weak_done.upgrade() {
                                    let img = slint::Image::load_from_path(&saved_path).unwrap_or_default();
                                    let s = app.global::<AppState>();
                                    s.set_sb_output(path_str.clone().into());
                                    s.set_sb_output_image(img.clone());
                                    upsert_shot_image(&s, idx as i32, shot_idx as i32, path_str.clone(), img);
                                    s.set_sb_generating_shot(-1);
                                    s.set_status_text(format!("分镜生成完成 → {path_str}").into());
                                }
                            });
                            Ok(())
                        }
                        Err(e) => {
                            let app_weak_err = app_weak_inner.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(app) = app_weak_err.upgrade() {
                                    app.global::<AppState>().set_sb_generating_shot(-1);
                                }
                            });
                            Err(e)
                        },
                    }
                },
            );
        });
    }
}

// ── 辅助 ────────────────────────────────────────────────────────────

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

fn default_storyboard_prompt(label: &str, markdown: &str) -> String {
    let brief = markdown
        .lines()
        .map(|line| line.trim().trim_matches('*').trim())
        .filter(|line| !line.is_empty())
        .take(10)
        .collect::<Vec<_>>()
        .join(" ");
    let brief = utils::short(&brief, 360);
    format!(
        "分镜包：{label}\n\
         画面风格：电影感动画分镜，角色造型一致，构图清晰，光影自然。\n\
         视觉重点：{brief}\n\
         补充要求："
    )
}

fn refresh_storyboard_shots(s: &AppState) {
    let idx = s.get_sb_selected() as usize;
    let packages = s.get_sb_packages();
    let Some(pkg) = packages.iter().nth(idx) else {
        s.set_sb_shots(ModelRc::new(VecModel::from(
            Vec::<StoryboardShotItem>::new(),
        )));
        s.set_sb_selected_shot(0);
        s.set_sb_shot_prompt("".into());
        s.set_sb_shot_note("".into());
        return;
    };
    let shots = split_shots_from_markdown(&pkg.markdown);
    let first_prompt = shots
        .first()
        .map(|shot| default_storyboard_prompt(&shot.label, &shot.markdown))
        .unwrap_or_default();
    s.set_sb_shots(ModelRc::new(VecModel::from(shots)));
    s.set_sb_selected_shot(0);
    s.set_sb_shot_prompt(first_prompt.into());
    s.set_sb_shot_note("".into());
    refresh_current_shot_image(s);
}

fn upsert_shot_image(
    s: &AppState,
    package_index: i32,
    shot_index: i32,
    path: String,
    image: slint::Image,
) {
    let mut images: Vec<StoryboardShotImageItem> = s.get_sb_shot_images().iter().collect();
    images.retain(|item| !(item.package_index == package_index && item.shot_index == shot_index));
    images.push(StoryboardShotImageItem {
        package_index,
        shot_index,
        path: path.into(),
        image,
    });
    s.set_sb_shot_images(ModelRc::new(VecModel::from(images)));
    refresh_current_shot_image(s);
}

fn refresh_current_shot_image(s: &AppState) {
    let package_index = s.get_sb_selected();
    let shot_index = s.get_sb_selected_shot();
    let images = s.get_sb_shot_images();
    if let Some(item) = images
        .iter()
        .find(|item| item.package_index == package_index && item.shot_index == shot_index)
    {
        s.set_sb_current_shot_image_path(item.path.clone());
        s.set_sb_current_shot_image(item.image);
    } else {
        s.set_sb_current_shot_image_path("".into());
        s.set_sb_current_shot_image(slint::Image::default());
    }
}

fn storyboard_output_dir(cfg: &artait_model::AppConfig, s: &AppState) -> std::path::PathBuf {
    let project_id = s.get_current_project_id().to_string();
    let dir = if project_id.trim().is_empty() {
        cfg.paths.output_subdir("storyboards")
    } else {
        cfg.projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| std::path::PathBuf::from(&project.path).join("storyboards"))
            .unwrap_or_else(|| cfg.paths.output_subdir("storyboards"))
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn split_shots_from_markdown(markdown: &str) -> Vec<StoryboardShotItem> {
    let mut shots: Vec<(String, Vec<String>)> = Vec::new();
    let mut current_label = String::new();
    let mut current_lines: Vec<String> = Vec::new();

    for line in markdown.lines() {
        let trimmed = line.trim();
        if is_shot_header(trimmed) {
            if !current_lines.is_empty() {
                shots.push((current_label.clone(), std::mem::take(&mut current_lines)));
            }
            current_label = clean_shot_label(trimmed);
            current_lines.push(line.to_string());
        } else if !current_lines.is_empty() {
            current_lines.push(line.to_string());
        }
    }
    if !current_lines.is_empty() {
        shots.push((current_label, current_lines));
    }

    if shots.is_empty() {
        let summary = markdown_summary(markdown);
        return vec![StoryboardShotItem {
            index: 0,
            label: "全文".into(),
            markdown: markdown.into(),
            summary: summary.into(),
        }];
    }

    shots
        .into_iter()
        .enumerate()
        .map(|(idx, (label, lines))| {
            let markdown = lines.join("\n");
            StoryboardShotItem {
                index: idx as i32,
                label: if label.is_empty() {
                    format!("镜头 {}", idx + 1).into()
                } else {
                    label.into()
                },
                summary: markdown_summary(&markdown).into(),
                markdown: markdown.into(),
            }
        })
        .collect()
}

fn clean_shot_label(text: &str) -> String {
    text.trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches('*')
        .trim()
        .to_string()
}

fn is_shot_header(trimmed: &str) -> bool {
    let normalized = clean_shot_label(trimmed);

    normalized.starts_with("镜头")
        || normalized.starts_with("Shot")
        || is_scene_heading(&normalized)
}

fn is_scene_heading(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    let mut first_digits = 0usize;
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        first_digits += 1;
        chars.next();
    }
    if first_digits == 0 || chars.next() != Some('-') {
        return false;
    }
    let mut second_digits = 0usize;
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        second_digits += 1;
        chars.next();
    }
    second_digits > 0
}

fn markdown_summary(markdown: &str) -> String {
    let text = markdown
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches('#')
                .trim()
                .trim_matches('*')
                .trim()
        })
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" / ");
    utils::short(&text, 110)
}

fn refresh_script_files(s: &AppState, dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let files =
        match script_index::ScriptIndexStore::default().and_then(|store| store.sync_dir(dir)) {
            Ok(entries) => {
                let paths: Vec<std::path::PathBuf> =
                    entries.into_iter().map(|entry| entry.path).collect();
                s.set_script_files(ModelRc::new(VecModel::from(script_items(&paths))));
                paths
            }
            Err(_) => {
                let paths = script::list_scripts(dir);
                s.set_script_files(ModelRc::new(VecModel::from(script_items(&paths))));
                paths
            }
        };
    files
}

fn push_script_files(s: &AppState, dir: &std::path::Path) {
    let _ = refresh_script_files(s, dir);
}

fn open_latest_script_if_needed(
    s: &AppState,
    output_dir: &std::path::Path,
    app_weak: &slint::Weak<AppShell>,
) {
    let selected = s.get_script_selected().to_string();
    let has_content = !s.get_script_content().trim().is_empty();
    let selected_exists = !selected.trim().is_empty() && std::path::Path::new(&selected).exists();
    if has_content && selected_exists {
        return;
    }
    if let Some(path) = refresh_script_files(s, output_dir).into_iter().next() {
        select_and_analyze_script(s, &path, output_dir, app_weak);
    }
}

fn select_and_analyze_script(
    s: &AppState,
    path: &std::path::Path,
    output_dir: &std::path::Path,
    app_weak: &slint::Weak<AppShell>,
) {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let plain = script::markdown_to_plain(&raw);
    push_script_files(s, output_dir);
    s.set_script_selected(path.display().to_string().into());
    s.set_script_content(raw.clone().into());
    s.set_script_content_plain(plain.into());
    s.set_script_story_ready(true);
    s.set_script_blocks(ModelRc::new(VecModel::from(script_blocks(&raw))));
    s.set_script_view_mode("preview".into());
    s.set_script_packages(ModelRc::new(VecModel::from(
        Vec::<StoryboardPackageItem>::new(),
    )));
    if let Ok(store) = script_index::ScriptIndexStore::default() {
        if let Ok(Some(cached)) = store.cached_analysis(path) {
            apply_cached_analysis(s, cached);
            return;
        }
    }
    s.set_script_parse_status("idle".into());
    s.set_script_parse_summary("已加载正文，解析将在后台完成".into());
    s.set_script_episode_count(0);
    s.set_script_parsed_scene_count(0);
    s.set_script_character_count(0);
    s.set_script_dialogue_count(0);
    clear_script_structure(s);
    spawn_script_parse(app_weak, raw, Some(path.to_path_buf()));
}

fn spawn_script_parse(
    app_weak: &slint::Weak<AppShell>,
    raw: String,
    path: Option<std::path::PathBuf>,
) {
    if raw.trim().is_empty() {
        return;
    }
    if let Some(app) = app_weak.upgrade() {
        let s = app.global::<AppState>();
        s.set_script_parse_status("idle".into());
        s.set_script_parse_summary("正在后台解析剧本结构...".into());
    }

    let app_weak = app_weak.clone();
    let expected_path = path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    std::thread::spawn(move || {
        let result = parse_script_for_ui(&raw, path.as_deref());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let s = app.global::<AppState>();
            if !expected_path.is_empty() && s.get_script_selected().as_str() != expected_path {
                return;
            }
            match result {
                Ok((report, structure)) => apply_analysis(&s, &report, structure),
                Err(e) => {
                    s.set_script_episode_count(0);
                    s.set_script_parsed_scene_count(0);
                    s.set_script_character_count(0);
                    s.set_script_dialogue_count(0);
                    s.set_script_parse_status("error".into());
                    s.set_script_parse_summary(format!("解析失败：{e}").into());
                    clear_script_structure(&s);
                }
            }
        });
    });
}

fn parse_script_for_ui(
    raw: &str,
    path: Option<&std::path::Path>,
) -> anyhow::Result<(script::ScriptParseReport, script::ScriptStructureSummary)> {
    let report = script::analyze_script(raw)?;
    let structure = script::summarize_script_structure(raw)?;
    if let Some(path) = path {
        if let Ok(store) = script_index::ScriptIndexStore::default() {
            let _ = store.upsert_analysis(path, &report, &structure);
        }
    }
    Ok((report, structure))
}

fn apply_cached_analysis(s: &AppState, cached: script_index::CachedScriptAnalysis) {
    apply_analysis(s, &cached.report, cached.structure);
}

fn apply_analysis(
    s: &AppState,
    report: &script::ScriptParseReport,
    structure: script::ScriptStructureSummary,
) {
    s.set_script_episode_count(report.episode_count as i32);
    s.set_script_parsed_scene_count(report.scene_count as i32);
    s.set_script_character_count(report.character_count as i32);
    s.set_script_dialogue_count(report.dialogue_count as i32);
    s.set_script_parse_status("ready".into());
    s.set_script_parse_summary(report.summary.clone().into());
    apply_structure_summary_value(s, structure);
}

fn apply_structure_summary_value(s: &AppState, summary: script::ScriptStructureSummary) {
    let scene_items: Vec<ScriptSceneSummaryItem> = summary
        .scenes
        .into_iter()
        .map(|scene| ScriptSceneSummaryItem {
            id: scene.id.into(),
            episode: scene.episode.into(),
            label: scene.label.into(),
            characters: scene.characters.into(),
            action_preview: scene.action_preview.into(),
            dialogue_count: scene.dialogue_count as i32,
        })
        .collect();
    let character_items: Vec<ScriptCharacterSummaryItem> = summary
        .characters
        .into_iter()
        .map(|character| ScriptCharacterSummaryItem {
            name: character.name.into(),
            role: character.role.into(),
            scene_count: character.scene_count as i32,
            dialogue_count: character.dialogue_count as i32,
            sample: character.sample.into(),
        })
        .collect();
    s.set_script_scenes(ModelRc::new(VecModel::from(scene_items)));
    s.set_script_characters(ModelRc::new(VecModel::from(character_items)));
}

fn clear_script_structure(s: &AppState) {
    s.set_script_scenes(ModelRc::new(VecModel::from(
        Vec::<ScriptSceneSummaryItem>::new(),
    )));
    s.set_script_characters(ModelRc::new(VecModel::from(Vec::<
        ScriptCharacterSummaryItem,
    >::new())));
    s.set_script_packages(ModelRc::new(VecModel::from(
        Vec::<StoryboardPackageItem>::new(),
    )));
}

fn script_blocks(raw: &str) -> Vec<ScriptBlockItem> {
    let mut blocks = Vec::new();
    let mut current_title = String::new();
    let mut current_lines: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if current_lines.len() >= 8 {
                push_script_block(&mut blocks, &mut current_title, &mut current_lines);
            }
            continue;
        }
        if is_script_block_header(trimmed) && !current_lines.is_empty() {
            push_script_block(&mut blocks, &mut current_title, &mut current_lines);
        }
        if current_title.is_empty() && is_script_block_header(trimmed) {
            current_title = clean_shot_label(trimmed);
        }
        current_lines.push(trimmed.to_string());
        let char_count: usize = current_lines.iter().map(|line| line.chars().count()).sum();
        if char_count >= 900 {
            push_script_block(&mut blocks, &mut current_title, &mut current_lines);
        }
    }
    if !current_lines.is_empty() {
        push_script_block(&mut blocks, &mut current_title, &mut current_lines);
    }

    if blocks.is_empty() && !raw.trim().is_empty() {
        blocks.push(ScriptBlockItem {
            index: 0,
            title: "正文".into(),
            body: utils::short(raw.trim(), 900).into(),
        });
    }
    blocks
}

fn push_script_block(
    blocks: &mut Vec<ScriptBlockItem>,
    title: &mut String,
    lines: &mut Vec<String>,
) {
    let index = blocks.len();
    let body = lines.join("\n");
    let fallback_title = body
        .lines()
        .next()
        .map(clean_shot_label)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("段落 {}", index + 1));
    blocks.push(ScriptBlockItem {
        index: index as i32,
        title: if title.is_empty() {
            fallback_title.into()
        } else {
            title.clone().into()
        },
        body: body.into(),
    });
    title.clear();
    lines.clear();
}

fn is_script_block_header(text: &str) -> bool {
    let normalized = clean_shot_label(text);
    normalized.starts_with("第")
        || is_scene_heading(&normalized)
        || normalized.starts_with("镜头")
        || normalized.starts_with("Shot")
        || text.starts_with('#')
}

fn current_script_text(s: &AppState) -> String {
    let content = s.get_script_content().to_string();
    if content.trim().is_empty() {
        s.get_script_theme().to_string()
    } else {
        content
    }
}

fn optional_project_id(s: &AppState) -> Option<String> {
    let id = s.get_current_project_id().to_string();
    if id.trim().is_empty() {
        None
    } else {
        Some(id)
    }
}

fn script_items(files: &[std::path::PathBuf]) -> Vec<ScriptFileItem> {
    files
        .iter()
        .map(|p| ScriptFileItem {
            path: p.display().to_string().into(),
            name: p.file_name().and_then(|s| s.to_str()).unwrap_or("").into(),
        })
        .collect()
}

fn push_sb_ref_model(sb_ref: &Rc<RefCell<Vec<ReferenceImage>>>, s: &AppState) {
    let model: Vec<RefImageItem> = sb_ref
        .borrow()
        .iter()
        .map(|r| RefImageItem {
            path: r.local_path.display().to_string().into(),
            name: r.display_name.clone().into(),
            thumb: slint::Image::load_from_path(&r.local_path).unwrap_or_default(),
        })
        .collect();
    s.set_sb_ref_images(ModelRc::new(VecModel::from(model)));
}
