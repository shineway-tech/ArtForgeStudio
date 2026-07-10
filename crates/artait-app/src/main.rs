//! ArtForge Studio 桌面应用入口。

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use artait_model::{AppConfig, LastWorkspaceState, ReferenceImage, ReferenceImageSource, ThemeId};
use artait_provider::ProviderRegistry;
use artait_service::character_store::CharacterStore;
use artait_service::page_routing::{
    initial_page_from_config, is_restorable_page, is_workspace_page, is_ws_gen_page,
};
use artait_service::prompt_template::WorkspaceDraft;
use artait_service::scene_store::SceneStore;
use artait_service::sidecar::SidecarManager;
use artait_service::task_filter::task_matches_clear_filter;
use artait_task::TaskRunner;

use artait_service::prompt_template::default_template_category;
use artait_service::utils;
use bridge::TaskMetaMap;
use slint::{ComponentHandle, Model, ModelRc, Timer, VecModel};
use task_history::TaskHistory;
use tokio::runtime::Runtime;

mod assets;
mod bridge;
mod callbacks;
mod clipboard_reference;
mod generation;
mod onboarding;
#[allow(dead_code)]
mod prompt_template;
#[allow(dead_code)]
mod provider_helpers;
mod providers;
mod script;
mod system_drag;
mod task_history;
mod theme;

mod ui {
    slint::include_modules!();
}

use ui::{AppShell, AppState, AssetItem, FeatureItem, RefImageItem};

use provider_helpers::normalize_provider_secrets;

static RUNTIME_LOG_ENABLED: AtomicBool = AtomicBool::new(true);
static RUNTIME_DEBUG_LOG_ENABLED: AtomicBool = AtomicBool::new(false);
const BUILD_MARKER: &str = "image-endpoint-fallback-2026-06-10";

fn main() -> Result<()> {
    init_logging();
    let exe_path = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|e| format!("unknown: {e}"));
    tracing::info!(
        exe = %exe_path,
        version = env!("CARGO_PKG_VERSION"),
        build_marker = BUILD_MARKER,
        "ArtForge Studio 启动"
    );

    let app = AppShell::new()?;
    let state = app.global::<AppState>();

    let rt = Runtime::new()?;

    // 区分首启 vs 已配置：用 LoadOutcome::Missing 判断
    let load_outcome = artait_config::load_with_outcome();
    let config_missing = matches!(&load_outcome, Ok(artait_config::LoadOutcome::Missing(_)));

    let cfg = Rc::new(RefCell::new(match load_outcome {
        Ok(artait_config::LoadOutcome::Loaded(c)) => c,
        Ok(artait_config::LoadOutcome::Missing(c)) => {
            tracing::info!("未找到 app_config.toml，使用默认配置直接进入主界面");
            c
        }
        Ok(artait_config::LoadOutcome::Recovered { config, backup }) => {
            tracing::warn!("配置损坏，已备份到 {}", backup.display());
            config
        }
        Err(e) => {
            tracing::warn!(error = %e, "配置加载失败，使用默认配置");
            AppConfig::default()
        }
    }));
    if normalize_provider_secrets(&mut cfg.borrow_mut()) {
        persist(&cfg.borrow());
    }
    if config_missing {
        if let Err(e) = artait_config::ensure_dirs(&cfg.borrow()) {
            tracing::warn!(error = %e, "ensure_dirs 失败");
        }
        persist(&cfg.borrow());
    }
    if cfg.borrow_mut().features.migrate() {
        persist(&cfg.borrow());
        tracing::info!("功能配置已迁移：新增功能已启用");
    }
    tracing::info!(
        "配置加载：providers = {}, features = {}, config_missing = {}",
        cfg.borrow().providers.len(),
        cfg.borrow().features.enabled.len(),
        config_missing,
    );
    RUNTIME_LOG_ENABLED.store(cfg.borrow().runtime.log_enabled, Ordering::Relaxed);
    RUNTIME_DEBUG_LOG_ENABLED.store(cfg.borrow().runtime.debug_log_enabled, Ordering::Relaxed);

    let registry: Arc<ProviderRegistry> = Arc::new(providers::build_registry());
    let http = providers::shared_http();
    let runner: Arc<TaskRunner> = TaskRunner::new_with_handle(4, rt.handle().clone());
    let sidecar_mgr: Arc<SidecarManager> = Arc::new(SidecarManager::new(http.clone()));
    let character_store: Rc<RefCell<CharacterStore>> =
        Rc::new(RefCell::new(CharacterStore::load_or_default()));
    let scene_store: Rc<RefCell<SceneStore>> = Rc::new(RefCell::new(SceneStore::load_or_default()));
    let ref_images: Rc<RefCell<Vec<ReferenceImage>>> = Rc::new(RefCell::new(Vec::new()));
    let selected_assets: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
    let workspace_drafts: Rc<RefCell<HashMap<String, WorkspaceDraft>>> =
        Rc::new(RefCell::new(HashMap::new()));
    tracing::info!(
        "Registry: {} 协议族 · TaskRunner 并发 4",
        registry.list().len()
    );

    // 主题
    let theme_id = Rc::new(RefCell::new(cfg.borrow().ui.theme));
    let loaded = theme::load(*theme_id.borrow());
    theme::apply(&app, &loaded);
    theme::apply_font_overrides(&app, &cfg.borrow());

    // user.toml 样例与监听延后到首屏之后，避免非关键 IO 挡住启动反馈。
    let user_active = Arc::new(AtomicBool::new(matches!(*theme_id.borrow(), ThemeId::User)));
    let theme_watcher_slot: Rc<RefCell<Option<notify::RecommendedWatcher>>> =
        Rc::new(RefCell::new(None));

    // 任务历史持久化
    let history = Arc::new(tokio::sync::Mutex::new(TaskHistory::load_or_default()));
    let task_meta_map: TaskMetaMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    // 事件桥接
    bridge::spawn_bridge(
        runner.clone(),
        rt.handle(),
        app.as_weak(),
        Some(history.clone()),
        Some(task_meta_map.clone()),
    );

    // 资产桥接延后到首帧之后，避免启动时扫描 out/ 和加载缩略图挡住首次绘制。
    let asset_watcher_slot: Rc<RefCell<Option<notify::RecommendedWatcher>>> =
        Rc::new(RefCell::new(None));

    // AppState 初值
    state.set_features(build_feature_model(
        &cfg.borrow(),
        "art",
        cfg.borrow().last_project.is_some(),
    ));
    state.set_current_theme_id(theme::id_str(*theme_id.borrow()).into());
    state.set_sidebar_collapsed(cfg.borrow().ui.sidebar_collapsed);
    state.set_runtime_log_enabled(RUNTIME_LOG_ENABLED.load(Ordering::Relaxed));
    state.set_runtime_debug_log_enabled(RUNTIME_DEBUG_LOG_ENABLED.load(Ordering::Relaxed));
    state.set_runtime_log_path(runtime_log_path().display().to_string().into());
    state.set_status_text("就绪 · MVP 接近完成 · 批量生图 + 缩略图缓存 + provider 编辑".into());
    state.set_config_path(config_path_display().into());
    state.set_default_project_path(
        cfg.borrow()
            .paths
            .output_dir
            .join("projects")
            .display()
            .to_string()
            .into(),
    );
    let initial_page = initial_page_from_config(&cfg.borrow());
    state.set_current_page(initial_page.clone().into());
    set_prompt_history_model(&state, cfg.borrow().prompt_history.clone());
    if let Some(ws) = cfg
        .borrow()
        .last_workspace
        .as_ref()
        .filter(|ws| ws.page == initial_page && is_workspace_page(&ws.page))
    {
        apply_last_workspace_state(&state, ws);
    }
    providers::push_providers(&app, &cfg.borrow());

    // 保留引导 draft 供既有回调上下文使用，启动时不再自动进入引导。
    let onb = Rc::new(RefCell::new(onboarding::OnboardingDraft::from_default()));

    {
        let app_weak = app.as_weak();
        let rt_handle = rt.handle().clone();
        let user_active = user_active.clone();
        let slot = theme_watcher_slot.clone();
        Timer::single_shot(Duration::from_millis(2_200), move || {
            install_sample_user_theme();
            prompt_template::install_sample_prompt_optimization_template();
            let watcher = theme::spawn_user_theme_watcher(&rt_handle, app_weak, user_active);
            *slot.borrow_mut() = watcher;
        });
    }

    {
        let app_weak = app.as_weak();
        let rt_handle = rt.handle().clone();
        let output_dir = cfg.borrow().paths.output_dir.clone();
        let slot = asset_watcher_slot.clone();
        Timer::single_shot(Duration::from_millis(1_800), move || {
            let watcher = assets::spawn_asset_bridge(&rt_handle, output_dir, app_weak);
            *slot.borrow_mut() = Some(watcher);
        });
    }

    // 回调
    let ctx = Rc::new(callbacks::CbCtx {
        app: app.as_weak(),
        cfg: cfg.clone(),
        rt_handle: rt.handle().clone(),
        ref_images: ref_images.clone(),
        selected_assets: selected_assets.clone(),
        workspace_drafts: workspace_drafts.clone(),
        theme_id: theme_id.clone(),
        user_active: user_active.clone(),
        theme_watcher_slot: theme_watcher_slot.clone(),
        registry: registry.clone(),
        http: http.clone(),
        runner: runner.clone(),
        history: history.clone(),
        task_meta_map: task_meta_map.clone(),
        asset_watcher_slot: asset_watcher_slot.clone(),
        onb: onb.clone(),
        sidecar: sidecar_mgr.clone(),
        character_store: character_store.clone(),
        scene_store: scene_store.clone(),
    });

    // 设置 & 日志 & 主题 & 侧边栏 & 导航
    callbacks::settings::init(&ctx);
    // Provider 选择、测试、编辑、新增
    callbacks::provider::init(&ctx);
    // 首启引导
    callbacks::onboarding::init(&ctx);
    // 任务面板（取消、清除、重新获取）
    callbacks::tasks::init(&ctx, &app);
    // 生成
    callbacks::generation::init(&ctx, &app);
    // 视频生成
    callbacks::generation::init_video(&ctx, &app);
    callbacks::prompt_template::init(&ctx, &app);
    callbacks::assets::init(&ctx, &app);

    callbacks::project::init(&ctx, &app);

    let sb_ref_images: Rc<RefCell<Vec<ReferenceImage>>> = Rc::new(RefCell::new(Vec::new()));
    callbacks::script_storyboard::init(&ctx, &app, sb_ref_images.clone());
    callbacks::character_library::init(&ctx, &app);
    callbacks::scene_library::init(&ctx, &app);

    // 工作台模式切换
    {
        let state = app.global::<AppState>();
        let app_weak = app.as_weak();
        let cfg_rc = cfg.clone();
        state.on_switch_workspace_mode(move |mode| {
            let m = mode.to_string();
            if let Some(app) = app_weak.upgrade() {
                let s = app.global::<AppState>();
                s.set_workspace_mode(m.clone().into());
                let default_page: &str = match m.as_str() {
                    "film" => "project",
                    _ => "welcome",
                };
                s.set_current_page(default_page.into());
                s.set_features(crate::build_feature_model(
                    &cfg_rc.borrow(),
                    &m,
                    cfg_rc.borrow().last_project.is_some(),
                ));
            }
        });
    }

    // winit 文件拖放事件（workspace + storyboard 参考图）
    {
        use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
        let app_weak = app.as_weak();
        let ref_images_ws = ref_images.clone();
        let ref_images_sb = sb_ref_images.clone();
        let drafts = workspace_drafts.clone();
        let last_clipboard_image_sequence: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));
        let current_modifiers = Rc::new(RefCell::new(winit::keyboard::ModifiersState::empty()));

        app.window()
            .on_winit_window_event(move |_slint_win, event| {
                let Some(app) = app_weak.upgrade() else {
                    return EventResult::Propagate;
                };
                let s = app.global::<AppState>();
                match event {
                    winit::event::WindowEvent::ModifiersChanged(modifiers) => {
                        *current_modifiers.borrow_mut() = modifiers.state();
                        EventResult::Propagate
                    }
                    winit::event::WindowEvent::KeyboardInput { event, .. } => {
                        use winit::event::ElementState;
                        use winit::keyboard::{KeyCode, PhysicalKey};

                        if event.state != ElementState::Pressed
                            || event.physical_key != PhysicalKey::Code(KeyCode::KeyV)
                        {
                            return EventResult::Propagate;
                        }
                        let page = s.get_current_page().to_string();
                        if !is_ws_gen_page(&page) {
                            return EventResult::Propagate;
                        }
                        let modifiers = *current_modifiers.borrow();
                        if !modifiers.control_key() {
                            return EventResult::Propagate;
                        }
                        let added = callbacks::prompt_template::add_clipboard_reference_image(
                            &app_weak,
                            &ref_images_ws,
                            &drafts,
                            &last_clipboard_image_sequence,
                        );
                        if added {
                            EventResult::PreventDefault
                        } else {
                            EventResult::Propagate
                        }
                    }
                    winit::event::WindowEvent::HoveredFile(path) => {
                        let is_img = utils::is_image_path(path);
                        let page = s.get_current_page().to_string();
                        s.set_ws_drop_highlight(is_img && is_ws_gen_page(&page));
                        s.set_sb_drop_highlight(is_img && page == "storyboard");
                        EventResult::Propagate
                    }
                    winit::event::WindowEvent::HoveredFileCancelled => {
                        s.set_ws_drop_highlight(false);
                        s.set_sb_drop_highlight(false);
                        EventResult::Propagate
                    }
                    winit::event::WindowEvent::DroppedFile(path) => {
                        if !utils::is_image_path(path) {
                            s.set_ws_drop_highlight(false);
                            s.set_sb_drop_highlight(false);
                            return EventResult::Propagate;
                        }
                        let page = s.get_current_page().to_string();
                        let display_name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("image")
                            .to_string();
                        let mime_type = utils::mime_for_path(path);
                        if page == "storyboard" {
                            let mut ri = ref_images_sb.borrow_mut();
                            if !ri.iter().any(|r| r.local_path == *path) {
                                ri.push(ReferenceImage {
                                    local_path: path.clone(),
                                    display_name,
                                    mime_type,
                                    width: None,
                                    height: None,
                                    uploaded_url: None,
                                    upload_cache_key: None,
                                    source: ReferenceImageSource::DragAndDrop,
                                });
                            }
                            drop(ri);
                            let model: Vec<RefImageItem> = ref_images_sb
                                .borrow()
                                .iter()
                                .map(|r| RefImageItem {
                                    path: r.local_path.display().to_string().into(),
                                    name: r.display_name.clone().into(),
                                    thumb: slint::Image::load_from_path(&r.local_path)
                                        .unwrap_or_default(),
                                })
                                .collect();
                            s.set_sb_ref_images(ModelRc::new(VecModel::from(model)));
                        } else if is_ws_gen_page(&page) {
                            let mut ri = ref_images_ws.borrow_mut();
                            if !ri.iter().any(|r| r.local_path == *path) {
                                ri.push(ReferenceImage {
                                    local_path: path.clone(),
                                    display_name,
                                    mime_type,
                                    width: None,
                                    height: None,
                                    uploaded_url: None,
                                    upload_cache_key: None,
                                    source: ReferenceImageSource::DragAndDrop,
                                });
                            }
                            drop(ri);
                            let model: Vec<RefImageItem> = ref_images_ws
                                .borrow()
                                .iter()
                                .map(|r| RefImageItem {
                                    path: r.local_path.display().to_string().into(),
                                    name: r.display_name.clone().into(),
                                    thumb: slint::Image::load_from_path(&r.local_path)
                                        .unwrap_or_default(),
                                })
                                .collect();
                            s.set_ws_ref_images(ModelRc::new(VecModel::from(model)));
                            save_workspace_draft(&s, &ref_images_ws.borrow(), &drafts);
                        }
                        s.set_ws_drop_highlight(false);
                        s.set_sb_drop_highlight(false);
                        EventResult::Propagate
                    }
                    _ => EventResult::Propagate,
                }
            });
    }

    callbacks::settings::on_close_requested(&ctx, &app);

    tracing::info!("ArtForge Studio 进入事件循环");
    let run_result = app.run();
    persist_current_app_state(&app, &cfg, &ref_images, &workspace_drafts);
    run_result?;
    tracing::info!("ArtForge Studio 事件循环结束");

    // 关闭 sidecar 进程
    rt.block_on(async { sidecar_mgr.shutdown().await });

    Ok(())
}

#[allow(dead_code)]
fn persist(cfg: &AppConfig) {
    if let Err(e) = artait_config::save(cfg) {
        tracing::warn!(error = %e, "保存 app_config.toml 失败");
    }
}

pub(crate) fn persist_current_app_state(
    app: &AppShell,
    cfg: &Rc<RefCell<AppConfig>>,
    ref_images: &Rc<RefCell<Vec<ReferenceImage>>>,
    drafts: &Rc<RefCell<HashMap<String, WorkspaceDraft>>>,
) {
    let s = app.global::<AppState>();
    let page = s.get_current_page().to_string();
    save_workspace_draft(&s, &ref_images.borrow(), drafts);

    {
        let mut c = cfg.borrow_mut();
        c.last_main_tab = Some(if is_restorable_page(&page) {
            page.clone()
        } else {
            "welcome".into()
        });
        if is_workspace_page(&page) {
            c.last_workspace = Some(LastWorkspaceState {
                page,
                prompt: s.get_ws_prompt().to_string(),
                negative: s.get_ws_negative().to_string(),
                aspect: s.get_ws_aspect().to_string(),
                quality: s.get_ws_quality().to_string(),
                count: s.get_ws_count(),
            });
        }
    }

    persist(&cfg.borrow());
}

pub(crate) fn set_prompt_history_model(state: &AppState, items: Vec<String>) {
    let items: Vec<slint::SharedString> = items
        .into_iter()
        .filter_map(|item| {
            let item = item.trim().to_string();
            if item.is_empty() {
                None
            } else {
                Some(item.into())
            }
        })
        .take(20)
        .collect();
    state.set_prompt_history(ModelRc::new(VecModel::from(items)));
}

fn sync_asset_selection(state: &AppState, selected: &HashSet<String>) {
    sync_asset_model_selection(&state.get_assets_all(), selected);
    sync_asset_model_selection(&state.get_assets_scene(), selected);
    sync_asset_model_selection(&state.get_assets_character(), selected);
    sync_asset_model_selection(&state.get_assets_ui(), selected);
    sync_asset_model_selection(&state.get_assets_effect(), selected);
    sync_asset_model_selection(&state.get_assets_storyboard(), selected);
}

fn sync_asset_model_selection(model: &ModelRc<AssetItem>, selected: &HashSet<String>) {
    for row in 0..model.row_count() {
        let Some(mut item) = model.row_data(row) else {
            continue;
        };
        let next = selected.contains(item.path.as_str());
        if item.selected != next {
            item.selected = next;
            model.set_row_data(row, item);
        }
    }
}

fn find_asset_item(model: &ModelRc<AssetItem>, path: &str) -> Option<AssetItem> {
    for row in 0..model.row_count() {
        let item = model.row_data(row)?;
        if item.path.as_str() == path {
            return Some(item);
        }
    }
    None
}

/// 从 SQLite（主数据源）读取元数据并填入 AppState 元数据对话框。
/// AssetItem 在 AppState 里是显示缓存，不作为元数据来源。
fn populate_asset_metadata(state: &AppState, path: &str) {
    let file_path = Path::new(path);
    let fallback = find_asset_item(&state.get_assets_all(), path);

    let meta = artait_service::assets::read_asset_metadata(
        file_path,
        fallback.as_ref().map(|i| i.name.as_str()),
        fallback.as_ref().map(|i| i.bytes),
        fallback.as_ref().map(|i| i.domain.as_str()),
    );

    state.set_asset_meta_path(path.into());
    state.set_asset_meta_name(meta.file_name.into());
    state.set_asset_meta_domain(meta.domain.into());
    state.set_asset_meta_prompt(meta.prompt.into());
    state.set_asset_meta_quality(meta.quality.into());
    state.set_asset_meta_aspect_ratio(meta.aspect_ratio.into());
    state.set_asset_meta_model(meta.model.into());
    state.set_asset_meta_width(meta.width);
    state.set_asset_meta_height(meta.height);
    state.set_asset_meta_bytes(meta.bytes);
    state.set_asset_meta_director_summary(meta.director_summary.into());
}

fn debug_log(message: impl AsRef<str>) {
    if RUNTIME_DEBUG_LOG_ENABLED.load(Ordering::Relaxed) {
        tracing::info!("[DEBUG] {}", message.as_ref());
    }
}

fn init_logging() {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = fmt::layer()
        .with_target(false)
        .with_ansi(false)
        .with_writer(|| RuntimeLogWriter);

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .init();
}

struct RuntimeLogWriter;

impl std::io::Write for RuntimeLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !RUNTIME_LOG_ENABLED.load(Ordering::Relaxed) {
            return Ok(buf.len());
        }
        let path = runtime_log_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn runtime_log_path() -> PathBuf {
    artait_model::portable_data_dir()
        .join("logs")
        .join("ArtForgeStudio.log")
}

fn push_runtime_log(s: &AppState) {
    let filter = s.get_runtime_log_filter().to_string();
    let content = read_runtime_log(&filter);
    s.set_runtime_log_path(runtime_log_path().display().to_string().into());
    s.set_runtime_log_content(content.into());
}

fn read_runtime_log(filter: &str) -> String {
    const MAX_LINES: usize = 1_500;
    let path = runtime_log_path();
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) => {
            return format!(
                "无法读取运行日志：{e}\n路径：{}",
                runtime_log_path().display()
            )
        }
    };
    let needle = filter.trim().to_lowercase();
    let mut lines: Vec<&str> = raw.lines().collect();
    if !needle.is_empty() {
        lines.retain(|line| line.to_lowercase().contains(&needle));
    }
    let start = lines.len().saturating_sub(MAX_LINES);
    lines[start..].join("\n")
}

fn config_path_display() -> String {
    artait_config::app_config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "（无法定位配置目录）".into())
}

/// 第一次启动时把内置 dark 主题作为 user.toml 样例写入，方便用户照着改。
fn install_sample_user_theme() {
    let Some(path) = theme::user_theme_path() else {
        return;
    };
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, "创建 themes/ 目录失败");
            return;
        }
    }
    const SAMPLE: &str = include_str!("../../../themes/dark.toml");
    let with_header = format!(
        "# ArtForge Studio 用户自定义主题样例。\n# 修改并保存后，把顶栏主题循环到\"自定义\"即可生效，无需重启。\n# 也可在主题为\"自定义\"时实时编辑此文件。\n\n{SAMPLE}"
    );
    if let Err(e) = std::fs::write(&path, with_header) {
        tracing::warn!(error = %e, path = %path.display(), "写入 user.toml 样例失败");
    } else {
        tracing::info!("已写入用户主题样例 → {}", path.display());
    }
}

fn build_feature_model(cfg: &AppConfig, mode: &str, has_project: bool) -> ModelRc<FeatureItem> {
    use artait_model::FeatureId;

    // 影视创作模式 + 有项目 → 显示项目功能模块
    if has_project && mode == "film" {
        let items = vec![
            FeatureItem {
                id: "project_overview".into(),
                name: "概览".into(),
                enabled: true,
                visible: true,
            },
            FeatureItem {
                id: "project_script".into(),
                name: "剧本".into(),
                enabled: true,
                visible: true,
            },
            FeatureItem {
                id: "project_storyboard".into(),
                name: "分镜".into(),
                enabled: true,
                visible: true,
            },
            FeatureItem {
                id: "project_characters".into(),
                name: "角色".into(),
                enabled: true,
                visible: true,
            },
            FeatureItem {
                id: "project_scenes".into(),
                name: "场景".into(),
                enabled: true,
                visible: true,
            },
            FeatureItem {
                id: "project_video".into(),
                name: "视频生成".into(),
                enabled: true,
                visible: true,
            },
        ];
        return ModelRc::new(VecModel::from(items));
    }

    let order: &[FeatureId] = &[
        FeatureId::Scene,
        FeatureId::Character,
        FeatureId::CharacterLibrary,
        FeatureId::SceneLibrary,
        FeatureId::UiConcept,
        FeatureId::Effect,
        FeatureId::ActionSequence,
        FeatureId::AnimationScript,
        FeatureId::Storyboard,
        FeatureId::AssetBrowser,
    ];
    let items: Vec<FeatureItem> = order
        .iter()
        .filter(|id| {
            let m = id.workspace_mode();
            m == "both" || m == mode
        })
        .map(|id| FeatureItem {
            id: id.route_id().into(),
            name: id.display_name().into(),
            enabled: cfg.features.is_enabled(*id),
            visible: cfg.features.is_sidebar_visible(*id),
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

fn clear_tasks_from_state(state: &AppState, filter: &str) {
    let tasks = state.get_tasks();
    let mut kept = Vec::new();
    for index in 0..tasks.row_count() {
        if let Some(task) = tasks.row_data(index) {
            if !task_matches_clear_filter(task.status.as_str(), filter) {
                kept.push(task);
            }
        }
    }
    update_task_counts(state, &kept);
    state.set_tasks(ModelRc::new(VecModel::from(kept)));
}

pub(crate) fn remove_task_from_state(state: &AppState, id: &str) -> bool {
    let tasks = state.get_tasks();
    let mut kept = Vec::new();
    let mut removed = false;
    for index in 0..tasks.row_count() {
        if let Some(task) = tasks.row_data(index) {
            if task.id.as_str() == id {
                removed = true;
            } else {
                kept.push(task);
            }
        }
    }
    if removed {
        update_task_counts(state, &kept);
        state.set_tasks(ModelRc::new(VecModel::from(kept)));
    }
    removed
}

fn update_task_counts(state: &AppState, tasks: &[crate::ui::TaskItem]) {
    let running = tasks
        .iter()
        .filter(|task| artait_model::is_active_task_status(task.status.as_str()))
        .count() as i32;
    let completed = tasks
        .iter()
        .filter(|task| task.status.as_str() == "completed")
        .count() as i32;
    let failed = tasks
        .iter()
        .filter(|task| {
            let status = task.status.as_str();
            status == "failed" || status == "cancelled"
        })
        .count() as i32;
    state.set_tasks_count_running(running);
    state.set_tasks_count_completed(completed);
    state.set_tasks_count_failed(failed);
    update_request_list_counts(state);
}

pub(crate) fn update_request_list_counts(state: &AppState) {
    let mode = state.get_request_list_mode().to_string();
    let tasks = state.get_tasks();
    let mut total = 0;
    let mut running = 0;
    let mut completed = 0;
    let mut failed = 0;

    for index in 0..tasks.row_count() {
        let Some(task) = tasks.row_data(index) else {
            continue;
        };
        if !request_task_matches_mode(task.kind.as_str(), task.mode.as_str(), &mode) {
            continue;
        }
        total += 1;
        if artait_model::is_active_task_status(task.status.as_str()) {
            running += 1;
        } else if task.status.as_str() == "completed" {
            completed += 1;
        } else if task.status.as_str() == "failed" || task.status.as_str() == "cancelled" {
            failed += 1;
        }
    }

    state.set_request_list_count_total(total);
    state.set_request_list_count_running(running);
    state.set_request_list_count_completed(completed);
    state.set_request_list_count_failed(failed);
}

fn request_task_matches_mode(kind: &str, task_mode: &str, mode: &str) -> bool {
    if mode.is_empty() {
        return true;
    }
    if mode == "prompt_opt" {
        return kind == "prompt_opt" || task_mode == "prompt_opt";
    }
    task_mode == mode
}

fn navigate_to_page(
    s: &AppState,
    cfg: &AppConfig,
    ref_images: &Rc<RefCell<Vec<ReferenceImage>>>,
    drafts: &Rc<RefCell<HashMap<String, WorkspaceDraft>>>,
    target: &str,
) {
    save_workspace_draft(s, &ref_images.borrow(), drafts);
    s.set_current_page(target.into());
    if is_workspace_page(target) {
        restore_workspace_draft(s, cfg, ref_images, drafts, target);
    }
}

fn save_workspace_draft(
    s: &AppState,
    refs: &[ReferenceImage],
    drafts: &Rc<RefCell<HashMap<String, WorkspaceDraft>>>,
) {
    let page = s.get_current_page().to_string();
    if !is_workspace_page(&page) {
        return;
    }

    let draft = WorkspaceDraft {
        prompt: s.get_ws_prompt().to_string(),
        negative: s.get_ws_negative().to_string(),
        aspect: s.get_ws_aspect().to_string(),
        quality: s.get_ws_quality().to_string(),
        count: s.get_ws_count(),
        template_file: s.get_ws_template_file().to_string(),
        template_name: s.get_ws_template_name().to_string(),
        template_category: s.get_ws_template_category().to_string(),
        template_active_category: s.get_ws_template_active_category().to_string(),
        asset_purpose: s.get_ws_asset_purpose().to_string(),
        color_mood: s.get_ws_color_mood().to_string(),
        game_view: s.get_ws_game_view().to_string(),
        weather: s.get_ws_weather().to_string(),
        time_of_day: s.get_ws_time_of_day().to_string(),
        lighting: s.get_ws_lighting().to_string(),
        advanced_open: s.get_ws_advanced_open(),
        prompt_preview_open: s.get_ws_prompt_preview_open(),
        final_prompt_preview: s.get_ws_final_prompt_preview().to_string(),
        ref_images: refs.to_vec(),
    };
    drafts.borrow_mut().insert(page, draft);
}

fn restore_workspace_draft(
    s: &AppState,
    cfg: &AppConfig,
    ref_images: &Rc<RefCell<Vec<ReferenceImage>>>,
    drafts: &Rc<RefCell<HashMap<String, WorkspaceDraft>>>,
    page: &str,
) {
    if let Some(draft) = drafts.borrow().get(page).cloned() {
        s.set_ws_prompt(draft.prompt.into());
        s.set_ws_negative(draft.negative.into());
        s.set_ws_aspect(draft.aspect.into());
        s.set_ws_quality(draft.quality.into());
        s.set_ws_count(draft.count);
        s.set_ws_template_file(draft.template_file.into());
        s.set_ws_template_name(draft.template_name.into());
        s.set_ws_template_category(draft.template_category.into());
        s.set_ws_template_active_category(draft.template_active_category.into());
        s.set_ws_asset_purpose(draft.asset_purpose.into());
        s.set_ws_color_mood(draft.color_mood.into());
        s.set_ws_game_view(draft.game_view.into());
        s.set_ws_weather(draft.weather.into());
        s.set_ws_time_of_day(draft.time_of_day.into());
        s.set_ws_lighting(draft.lighting.into());
        s.set_ws_advanced_open(draft.advanced_open);
        s.set_ws_prompt_preview_open(draft.prompt_preview_open);
        s.set_ws_final_prompt_preview(draft.final_prompt_preview.into());
        *ref_images.borrow_mut() = draft.ref_images;
    } else {
        s.set_ws_prompt("".into());
        s.set_ws_negative("".into());
        s.set_ws_aspect("1:1".into());
        s.set_ws_quality("2K".into());
        s.set_ws_count(1);
        s.set_ws_asset_purpose("".into());
        s.set_ws_color_mood("".into());
        s.set_ws_game_view("".into());
        s.set_ws_weather("".into());
        s.set_ws_time_of_day("".into());
        s.set_ws_lighting("".into());
        s.set_ws_advanced_open(false);
        s.set_ws_prompt_preview_open(false);
        s.set_ws_final_prompt_preview("".into());
        s.set_ws_template_file("".into());
        s.set_ws_template_name("".into());
        s.set_ws_template_category(default_template_category().into());
        s.set_ws_template_active_category(default_template_category().into());
        ref_images.borrow_mut().clear();
    }
    crate::prompt_template::refresh_template_model(s, cfg, page);
    normalize_director_controls_for_page(s, page);
    push_ws_ref_images(s, &ref_images.borrow());
    s.set_ws_show_templates(false);
}

fn normalize_director_controls_for_page(s: &AppState, page: &str) {
    let purpose = s.get_ws_asset_purpose().to_string();
    if purpose.is_empty() {
        return;
    }
    let valid = match page {
        "ui_concept" => matches!(
            purpose.as_str(),
            "hud" | "main_menu" | "inventory" | "shop" | "icon" | "loading_ui" | "dialog"
        ),
        "effect" => matches!(
            purpose.as_str(),
            "skill_effect"
                | "buff_effect"
                | "explosion"
                | "scene_effect"
                | "ui_effect"
                | "weapon_trail"
        ),
        "character" | "animation_character" | "character_turnaround" => matches!(
            purpose.as_str(),
            "character_portrait"
                | "character_turnaround"
                | "eight_direction"
                | "sprite_sheet"
                | "spine_parts"
                | "npc_avatar"
                | "character_poster"
        ),
        _ => matches!(
            purpose.as_str(),
            "scene_concept"
                | "tileset"
                | "level_design_reference"
                | "promo_art"
                | "loading_art"
                | "mini_map"
                | "building_kit"
        ),
    };
    if !valid {
        s.set_ws_asset_purpose("".into());
    }
}

fn apply_last_workspace_state(s: &AppState, ws: &LastWorkspaceState) {
    s.set_ws_prompt(ws.prompt.clone().into());
    s.set_ws_negative(ws.negative.clone().into());
    s.set_ws_aspect(ws.aspect.clone().into());
    s.set_ws_quality(ws.quality.clone().into());
    s.set_ws_count(ws.count);
}

fn push_ws_ref_images(s: &AppState, refs: &[ReferenceImage]) {
    let model: Vec<crate::ui::RefImageItem> = refs
        .iter()
        .map(|r| crate::ui::RefImageItem {
            path: r.local_path.display().to_string().into(),
            name: r.display_name.clone().into(),
            thumb: slint::Image::load_from_path(&r.local_path).unwrap_or_default(),
        })
        .collect();
    s.set_ws_ref_images(ModelRc::new(VecModel::from(model)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last_workspace(page: &str) -> LastWorkspaceState {
        LastWorkspaceState {
            page: page.into(),
            prompt: "prompt".into(),
            negative: String::new(),
            aspect: "1:1".into(),
            quality: "2K".into(),
            count: 1,
        }
    }

    #[test]
    fn initial_page_prefers_last_main_tab() {
        let mut cfg = AppConfig::default();
        cfg.last_main_tab = Some("settings".into());
        cfg.last_workspace = Some(last_workspace("scene"));

        assert_eq!(initial_page_from_config(&cfg), "settings");
    }

    #[test]
    fn initial_page_falls_back_to_last_workspace() {
        let mut cfg = AppConfig::default();
        cfg.last_workspace = Some(last_workspace("character"));

        assert_eq!(initial_page_from_config(&cfg), "character");
    }

    #[test]
    fn initial_page_ignores_invalid_routes() {
        let mut cfg = AppConfig::default();
        cfg.last_main_tab = Some("missing_page".into());
        cfg.last_workspace = Some(last_workspace("not_workspace"));

        assert_eq!(initial_page_from_config(&cfg), "welcome");
    }

    #[test]
    fn art_features_stay_enabled_without_project() {
        let cfg = AppConfig::default();
        let features = build_feature_model(&cfg, "art", false);
        let mut enabled_ids = HashSet::new();
        for i in 0..features.row_count() {
            let item = features.row_data(i).expect("feature item");
            if item.enabled {
                enabled_ids.insert(item.id.to_string());
            }
        }

        assert!(enabled_ids.contains("scene"));
        assert!(enabled_ids.contains("character"));
        assert!(enabled_ids.contains("ui_concept"));
        assert!(enabled_ids.contains("effect"));
    }

    #[test]
    fn parses_prompt_optimization_json() {
        let output = artait_provider::request::AnalysisOutput {
            text: String::new(),
            structured: Some(serde_json::json!({
                "optimized_prompt": "cinematic dragon",
                "summary": "结合预设强化主体。",
                "changes": ["强化主体", "补充光影"]
            })),
            usage: None,
        };

        let parsed = artait_service::prompt_template::parse_prompt_optimization_output(&output);

        assert_eq!("cinematic dragon", parsed.optimized_prompt);
        assert_eq!("结合预设强化主体。", parsed.summary);
        assert_eq!("强化主体；补充光影", parsed.changes);
    }

    #[test]
    fn parses_prompt_optimization_json_from_code_fence() {
        let output = artait_provider::request::AnalysisOutput {
            text: "```json\n{\"optimized_prompt\":\"soft forest\",\"summary\":\"已优化。\",\"changes\":\"补充环境\"}\n```".into(),
            structured: None,
            usage: None,
        };

        let parsed = artait_service::prompt_template::parse_prompt_optimization_output(&output);

        assert_eq!("soft forest", parsed.optimized_prompt);
        assert_eq!("已优化。", parsed.summary);
        assert_eq!("补充环境", parsed.changes);
    }

    #[test]
    fn falls_back_to_plain_prompt_optimization_text() {
        let output = artait_provider::request::AnalysisOutput {
            text: "a clean product render".into(),
            structured: None,
            usage: None,
        };

        let parsed = artait_service::prompt_template::parse_prompt_optimization_output(&output);

        assert_eq!("a clean product render", parsed.optimized_prompt);
        assert!(parsed.summary.contains("非结构化"));
    }
}

#[cfg(windows)]
fn copy_text_to_clipboard_windows(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("clip.exe").stdin(Stdio::piped()).spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("clip.exe exited with {status}"),
        ))
    }
}
