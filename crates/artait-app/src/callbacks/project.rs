//! 项目回调：创建、打开、关闭、列表刷新。

use artait_model::project::ProjectType;
use artait_service::project_store::ProjectStore;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::CbCtx;
use crate::debug_log;
use crate::ui::{AppShell, AppState, ProjectItem};

pub(crate) fn init(ctx: &CbCtx, app: &AppShell) {
    let state = app.global::<AppState>();
    let app_weak = ctx.app.clone();
    let cfg_ref = ctx.cfg.clone();

    // 从配置加载已有项目列表
    {
        let cfg = cfg_ref.borrow();
        let output_dir = &cfg.paths.output_dir;
        let store = ProjectStore::new(output_dir);
        let entries = store.list_entries();
        let items: Vec<ProjectItem> = entries
            .iter()
            .map(|e| ProjectItem {
                id: e.id.clone().into(),
                name: e.name.clone().into(),
                description: "".into(),
                created_at: e.created_at.clone().into(),
                scene_count: 0,
                project_type: e.project_type.to_string().into(),
            })
            .collect();
        state.set_projects(ModelRc::new(VecModel::from(items)));

        // 恢复上次打开的项目
        if let Some(ref last_id) = cfg.last_project {
            if store.find_by_id(last_id).is_some() {
                let name = cfg
                    .projects
                    .iter()
                    .find(|p| p.id == *last_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                state.set_current_project_id(last_id.clone().into());
                state.set_current_project_name(name.clone().into());
                state.set_has_project(true);
            }
        }
    }

    // `create-project(name, desc)` — 创建项目并应用
    {
        let cfg = cfg_ref.clone();
        let aw = app_weak.clone();
        state.on_create_project(move |name, _desc, custom_path, project_type_str| {
            let name = name.to_string();
            let custom = custom_path.to_string();
            let project_type = match project_type_str.as_str() {
                "short_film" => ProjectType::ShortFilm,
                "series" => ProjectType::Series,
                _ => ProjectType::Movie,
            };
            let output_dir = cfg.borrow().paths.output_dir.clone();
            let parent_dir = if custom.trim().is_empty() {
                output_dir.join("projects")
            } else {
                std::path::PathBuf::from(custom.trim())
            };
            let store = ProjectStore::with_root(parent_dir);
            match store.create(&name, project_type) {
                Ok(project) => {
                    debug_log(format!("项目已创建：{} ({})", project.name, project.id));
                    // 更新配置中的项目和 last_project
                    {
                        let mut c = cfg.borrow_mut();
                        c.projects.push((&project).into());
                        c.last_project = Some(project.id.clone());
                        if let Err(e) = artait_config::save(&c) {
                            tracing::warn!(error = %e, "保存项目配置失败");
                        }
                    }
                    if let Some(app) = aw.upgrade() {
                        let s = app.global::<AppState>();
                        s.set_current_project_id(project.id.clone().into());
                        s.set_current_project_name(project.name.clone().into());
                        s.set_has_project(true);
                        s.set_status_text(format!("项目「{}」已创建", project.name).into());
                        // 刷新项目列表
                        refresh_project_list(&s, &store);
                        // 刷新特征状态（启用创作区）
                        refresh_features(&s, &cfg.borrow());
                        // 导航到视频创作页
                        s.set_current_page("project_overview".into());
                    }
                }
                Err(e) => {
                    debug_log(format!("创建项目失败：{e}"));
                    if let Some(app) = aw.upgrade() {
                        app.global::<AppState>()
                            .set_status_text(format!("创建项目失败: {e}").into());
                    }
                }
            }
        });
    }

    // `open-project(id)` — 切换到已有项目
    {
        let cfg = cfg_ref.clone();
        let aw = app_weak.clone();
        state.on_open_project(move |id| {
            let id = id.to_string();
            let output_dir = cfg.borrow().paths.output_dir.clone();
            let store = ProjectStore::new(&output_dir);
            if let Some(project) = store.find_by_id(&id) {
                {
                    let mut c = cfg.borrow_mut();
                    c.last_project = Some(project.id.clone());
                    if let Err(e) = artait_config::save(&c) {
                        tracing::warn!(error = %e, "保存项目配置失败");
                    }
                }
                if let Some(app) = aw.upgrade() {
                    let s = app.global::<AppState>();
                    s.set_current_project_id(project.id.clone().into());
                    s.set_current_project_name(project.name.clone().into());
                    s.set_has_project(true);
                    s.set_status_text(format!("已切换到项目「{}」", project.name).into());
                    // 刷新特征状态（启用创作区）
                    refresh_features(&s, &cfg.borrow());
                    // 导航到视频创作页
                    s.set_current_page("project_overview".into());
                }
            }
        });
    }

    // `close-project()` — 回到通配模式
    {
        let cfg = cfg_ref.clone();
        let aw = app_weak.clone();
        state.on_close_project(move || {
            {
                let mut c = cfg.borrow_mut();
                c.last_project = None;
                if let Err(e) = artait_config::save(&c) {
                    tracing::warn!(error = %e, "保存项目配置失败");
                }
            }
            if let Some(app) = aw.upgrade() {
                let s = app.global::<AppState>();
                s.set_current_project_id("".into());
                s.set_current_project_name("".into());
                s.set_has_project(false);
                s.set_status_text("已关闭项目，回到全局模式".into());
                // 刷新特征状态（灰掉创作区）
                refresh_features(&s, &cfg.borrow());
                s.set_current_page("welcome".into());
            }
        });
    }

    // `open-create-project()` — 导航到创建项目页
    {
        let aw = app_weak.clone();
        state.on_open_create_project(move || {
            if let Some(app) = aw.upgrade() {
                app.global::<AppState>()
                    .set_current_page("create_project".into());
            }
        });
    }

    // `navigate-to-project()` — 导航到项目页
    {
        let aw = app_weak.clone();
        state.on_navigate_to_project(move || {
            if let Some(app) = aw.upgrade() {
                app.global::<AppState>().set_current_page("project".into());
            }
        });
    }
}

fn refresh_project_list(state: &AppState, store: &ProjectStore) {
    let entries = store.list_entries();
    let items: Vec<ProjectItem> = entries
        .iter()
        .map(|e| ProjectItem {
            id: e.id.clone().into(),
            name: e.name.clone().into(),
            description: "".into(),
            created_at: e.created_at.clone().into(),
            scene_count: 0,
            project_type: e.project_type.to_string().into(),
        })
        .collect();
    state.set_projects(ModelRc::new(VecModel::from(items)));
}

/// 根据当前项目状态刷新侧边栏特征项的 enabled 状态。
fn refresh_features(state: &AppState, cfg: &artait_model::AppConfig) {
    let mode = state.get_workspace_mode().to_string();
    let features = crate::build_feature_model(cfg, &mode, cfg.last_project.is_some());
    state.set_features(features);
}
