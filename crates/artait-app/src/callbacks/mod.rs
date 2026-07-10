//! 封装所有共享状态，让回调闭包通过 `ctx.clone()` 捕获。

pub(crate) mod assets;
pub(crate) mod character_library;
pub(crate) mod generation;
pub(crate) mod onboarding;
pub(crate) mod project;
pub(crate) mod prompt_template;
pub(crate) mod provider;
pub(crate) mod scene_library;
pub(crate) mod script_storyboard;
pub(crate) mod settings;
pub(crate) mod tasks;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use artait_model::{AppConfig, ReferenceImage, ThemeId};
use artait_provider::ProviderRegistry;
use artait_service::character_store::CharacterStore;
use artait_service::scene_store::SceneStore;
use artait_service::sidecar::SidecarManager;
use artait_task::TaskRunner;

use crate::bridge::TaskMetaMap;
use crate::task_history::TaskHistory;

/// 封装所有共享状态，让回调闭包通过 `ctx.clone()` 捕获。
#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct CbCtx {
    pub(crate) app: slint::Weak<crate::ui::AppShell>,
    pub(crate) cfg: Rc<RefCell<AppConfig>>,
    pub(crate) rt_handle: tokio::runtime::Handle,
    pub(crate) ref_images: Rc<RefCell<Vec<ReferenceImage>>>,
    pub(crate) selected_assets: Rc<RefCell<HashSet<String>>>,
    pub(crate) workspace_drafts: Rc<RefCell<HashMap<String, crate::WorkspaceDraft>>>,
    pub(crate) theme_id: Rc<RefCell<ThemeId>>,
    pub(crate) user_active: Arc<AtomicBool>,
    pub(crate) theme_watcher_slot: Rc<RefCell<Option<notify::RecommendedWatcher>>>,
    pub(crate) registry: Arc<ProviderRegistry>,
    pub(crate) http: Arc<dyn artait_provider::HttpClient>,
    pub(crate) runner: Arc<TaskRunner>,
    pub(crate) history: Arc<tokio::sync::Mutex<TaskHistory>>,
    pub(crate) task_meta_map: TaskMetaMap,
    pub(crate) asset_watcher_slot: Rc<RefCell<Option<notify::RecommendedWatcher>>>,
    pub(crate) onb: Rc<RefCell<crate::onboarding::OnboardingDraft>>,
    pub(crate) sidecar: Arc<SidecarManager>,
    pub(crate) character_store: Rc<RefCell<CharacterStore>>,
    pub(crate) scene_store: Rc<RefCell<SceneStore>>,
}

impl CbCtx {
    /// 持久化配置。
    pub(crate) fn save_cfg(&self) {
        if let Err(e) = artait_config::save(&self.cfg.borrow()) {
            tracing::warn!(error = %e, "保存 app_config.toml 失败");
        }
    }
}
