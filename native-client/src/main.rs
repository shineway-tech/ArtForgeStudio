#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use slint::{Image, Model, ModelRc, SharedString, VecModel, Weak};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};
use uuid::Uuid;

mod drag_preview;

slint::include_modules!();

const TEST_CODE: &str = "123456";
const PROMPT_SERVICE_CREDIT_COST: i32 = 5;
const DAILY_FREE_CREDITS: i32 = 250;
const IMAGE_GENERATION_WAIT_SECS: u64 = 900;
const IMAGE_REQUEST_TIMEOUT_SECS: u64 = IMAGE_GENERATION_WAIT_SECS;
const IMAGE_POLL_INTERVAL_MS: u64 = 2000;
const IMAGE_POLL_ATTEMPTS: usize = 450;
const MAX_REFERENCE_IMAGES: usize = 8;
const IMAGE_DRAG_MIME: &str = "application/x-artforge-image-path";
const URI_LIST_MIME: &str = "text/uri-list";
const TEXT_PLAIN_MIME: &str = "text/plain";
const ACTION_SEQUENCE_RATIOS: [(&'static str, i32, i32); 3] =
    [("1:1", 1, 1), ("9:16", 9, 16), ("16:9", 16, 9)];

#[derive(Clone, Default, Serialize, Deserialize)]
struct ProviderData {
    id: String,
    name: String,
    endpoint: String,
    remark: String,
    website: String,
    api_key: String,
    models: Vec<String>,
    #[serde(default)]
    used_models: Vec<String>,
    selected_model: String,
}

#[derive(Clone)]
struct AssetData {
    id: String,
    conversation_id: String,
    title: String,
    category: String,
    kind: String,
    time: String,
    prompt: String,
    ratio: String,
    quality: String,
    model: String,
    width: i32,
    height: i32,
    image: Image,
    source_path: String,
    cutout_done: bool,
    remove_black_done: bool,
    upscale_done: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct NotificationData {
    id: String,
    title: String,
    model: String,
    time: String,
    reason: String,
    success: bool,
    read: bool,
}

#[derive(Clone)]
struct ReferenceData {
    id: String,
    image: Image,
    source_path: String,
}

#[derive(Clone, Default)]
struct ReferenceGroups {
    character: Vec<ReferenceData>,
    scene: Vec<ReferenceData>,
    ui: Vec<ReferenceData>,
    effect: Vec<ReferenceData>,
    action_sequence: Vec<ReferenceData>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CreditRecordData {
    title: String,
    amount: String,
    time: String,
    note: String,
    positive: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct InvitedUserData {
    email: String,
    username: String,
    rebate_points: i32,
    register_time: String,
}

#[derive(Clone)]
struct QuoteContext {
    title: String,
    prompt: String,
    ratio: String,
    quality: String,
    width: i32,
    height: i32,
}

#[derive(Clone)]
struct PromptControls {
    category: String,
    creation: String,
    style: String,
    view: String,
    weather: String,
    time: String,
    light: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptLanguage {
    Chinese,
    English,
}

enum GenerationOutcome {
    Success {
        images: Vec<Vec<u8>>,
        failed_count: usize,
        failure_reason: String,
        optimized: String,
        time: String,
    },
    ImageSuccess {
        bytes: Vec<u8>,
        optimized: String,
        time: String,
    },
    ImageFailure {
        reason: String,
        optimized: String,
        time: String,
    },
    Finished {
        time: String,
    },
    Failure {
        reason: String,
        time: String,
    },
}

#[derive(Clone, Default)]
struct ActiveGeneration {
    task_id: String,
    category: String,
    conversation_id: String,
    prompt: String,
    credit_cost: i32,
    credit_per_image: i32,
    total_count: i32,
    loading_count: i32,
    completed_count: i32,
    success_count: i32,
    failed_count: i32,
    progress: i32,
    eta: i32,
}

thread_local! {
    static ACTIVE_GENERATIONS: RefCell<BTreeMap<String, ActiveGeneration>> = RefCell::new(BTreeMap::new());
    static GENERATION_STATUS_BY_CATEGORY: RefCell<BTreeMap<String, String>> = RefCell::new(BTreeMap::new());
}

enum ApiResponse {
    Json(Value),
    Bytes(Vec<u8>),
}

struct ImageRequestResult {
    images: Vec<Vec<u8>>,
}

struct ImageBatchResult {
    images: Vec<Vec<u8>>,
    failed_count: usize,
    last_error: Option<String>,
}

#[derive(Default)]
struct Store {
    providers: Vec<ProviderData>,
    generations: Vec<AssetData>,
    assets: Vec<AssetData>,
    inspiration: Vec<AssetData>,
    notifications: Vec<NotificationData>,
    references: ReferenceGroups,
    prompt_drafts: PromptDrafts,
}

#[derive(Default, Serialize, Deserialize)]
struct LocalStoreData {
    #[serde(default)]
    providers: Vec<ProviderData>,
    #[serde(default)]
    generations: Vec<StoredAssetData>,
    #[serde(default)]
    assets: Vec<StoredAssetData>,
    #[serde(default)]
    notifications: Vec<NotificationData>,
    #[serde(default)]
    image_provider_id: String,
    #[serde(default)]
    image_model: String,
    #[serde(default)]
    reasoning_provider_id: String,
    #[serde(default)]
    reasoning_model: String,
    #[serde(default)]
    prompt_drafts: PromptDrafts,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct PromptDrafts {
    #[serde(default)]
    character: String,
    #[serde(default)]
    scene: String,
    #[serde(default)]
    ui: String,
    #[serde(default)]
    effect: String,
    #[serde(default)]
    action_sequence: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredAssetData {
    id: String,
    conversation_id: String,
    title: String,
    category: String,
    kind: String,
    time: String,
    prompt: String,
    ratio: String,
    quality: String,
    model: String,
    width: i32,
    height: i32,
    source_path: String,
    #[serde(default)]
    cutout_done: bool,
    #[serde(default)]
    remove_black_done: bool,
    #[serde(default)]
    upscale_done: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct UserProfileData {
    logged_in: bool,
    phone_mask: String,
    nickname: String,
    credit_balance: i32,
    credit_records: Vec<CreditRecordData>,
    #[serde(default)]
    last_daily_credit_date: String,
    #[serde(default)]
    theme_id: String,
    #[serde(default = "default_card_style")]
    card_style: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    asset_type: String,
    #[serde(default)]
    invite_code: String,
    #[serde(default)]
    invite_link: String,
    #[serde(default)]
    invited_users: Vec<InvitedUserData>,
}

fn default_card_style() -> String {
    "rounded".to_string()
}

#[derive(Default, Deserialize)]
struct UpdateManifest {
    version: String,
}

fn main() -> Result<()> {
    std::env::set_var("SLINT_BACKEND", "winit-software");
    let app = AppWindow::new()?;
    app.window().set_size(slint::PhysicalSize::new(1440, 900));
    init_version_state(&app);
    apply_theme(&app, "light");
    init_portable_dirs(&app)?;
    load_user_profile(&app);
    load_showcase_images(&app);

    let store = Rc::new(RefCell::new(Store::default()));
    load_local_store(&app, &store);
    seed_inspiration(&app, &store)?;
    push_all(&app, &store.borrow());

    wire_callbacks(&app, store.clone());
    app.run()?;
    store_current_prompt_draft(
        &app,
        &store,
        &resolve_category(&app.global::<AppState>().get_asset_type().to_string(), ""),
    );
    save_user_profile(&app);
    save_local_store(&app, &store.borrow());
    Ok(())
}

fn wire_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        state.on_use_now(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_logged_in() {
                navigate_to(&app, "generation");
            } else {
                state.set_auth_open(true);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_request_code(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                if state.get_auth_phone().trim().len() < 11 {
                    state.set_auth_error("请输入正确的手机号".into());
                    return;
                }
                state.set_auth_error("验证码已发送".into());
                state.set_auth_countdown(60);
                start_countdown(app.as_weak());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_login_or_register(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let phone = state.get_auth_phone().trim().to_string();
            let code = state.get_auth_code().trim().to_string();
            if phone.len() < 11 {
                state.set_auth_error("请输入正确的手机号".into());
                return;
            }
            if code != TEST_CODE {
                state.set_auth_error("验证码不正确".into());
                return;
            }
            state.set_logged_in(true);
            state.set_phone_mask(mask_phone(&phone).into());
            state.set_nickname(mask_phone(&phone).into());
            ensure_credit_account(&app);
            grant_daily_free_credits(&app);
            state.set_auth_open(false);
            state.set_auth_error("".into());
            save_user_profile(&app);
            navigate_to(&app, "generation");
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_logout(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_logged_in(false);
                state.set_page("welcome".into());
                state.set_theme_id("light".into());
                apply_theme(&app, "light");
                state.set_profile_open(false);
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_save_profile(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let name = state.get_profile_name().trim().to_string();
                state.set_nickname(if name.is_empty() {
                    state.get_phone_mask()
                } else {
                    name.into()
                });
                state.set_profile_open(false);
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_recharge_credits(move |amount| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if amount <= 0 {
                return;
            }
            recharge_credits(&app, amount);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_navigate(move |page| {
            if let Some(app) = app_weak.upgrade() {
                navigate_to_with_store(&app, &store.borrow(), &page);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_back(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let page = state.get_page().to_string();
                if page == "generation" {
                    return;
                }
                state.set_page("generation".into());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_theme(move |theme| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_theme_id(theme.clone());
                apply_theme(&app, &theme);
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_card_style(move |style| {
            if let Some(app) = app_weak.upgrade() {
                let style = if style.as_str() == "square" { "square" } else { "rounded" };
                app.global::<AppState>().set_card_style(style.into());
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_language(move |lang| {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().set_language(lang);
                save_user_profile(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_workspace_category(move |category| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let category = resolve_category(&category.to_string(), "");
            let state = app.global::<AppState>();
            let previous_category = resolve_category(&state.get_asset_type().to_string(), "");
            if previous_category != category {
                store_current_prompt_draft(&app, &store, &previous_category);
                state.set_creation_mode("free".into());
                state.set_style_mode("free".into());
                state.set_view_mode("free".into());
                state.set_weather_mode("natural".into());
                state.set_time_mode("natural".into());
                state.set_light_mode("natural".into());
                state.set_advanced_preview_open(false);
                state.set_advanced_prompt_preview("".into());
            }
            if category == "action-sequence" {
                state.set_creation_mode("anim-idle".into());
                state.set_count(1);
                state.set_ratio_more_open(false);
                if !action_sequence_ratio_allowed(&state.get_ratio().to_string()) {
                    state.set_ratio("1:1".into());
                }
                let mut store_mut = store.borrow_mut();
                references_for_category_mut(&mut store_mut.references, &category)
                    .truncate(max_reference_images_for_category(&category));
            }
            state.set_asset_type(category.clone().into());
            state.set_mode("game".into());
            if previous_category != category {
                let prompt = prompt_draft_for_category(&store.borrow().prompt_drafts, &category);
                state.set_prompt(prompt.into());
            }
            push_references(&app, &store.borrow());
            save_local_store(&app, &store.borrow());
            save_user_profile(&app);
            push_generations(&app, &store.borrow());
            sync_generation_state_for_current_category(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_pick_dir(move |kind| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                let text = path.display().to_string();
                let state = app.global::<AppState>();
                match kind.as_str() {
                    "input" => state.set_input_dir(text.into()),
                    "output" => state.set_output_dir(text.into()),
                    "prompt" => state.set_prompt_dir(text.into()),
                    _ => {}
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_load_fonts(move || {
            if let Some(app) = app_weak.upgrade() {
                let fonts = load_system_fonts()
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>();
                app.global::<AppState>()
                    .set_font_list(ModelRc::new(VecModel::from(fonts)));
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_check_version(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_update_checking() {
                return;
            }
            state.set_update_checking(true);
            state.set_update_result_open(false);
            let app_weak = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(700), move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let has_update = refresh_update_state(&app);
                let state = app.global::<AppState>();
                state.set_update_checking(false);
                if has_update {
                    state.set_update_message(
                        format!("发现新版本 {}，可点击更新。", state.get_latest_version()).into(),
                    );
                } else {
                    state.set_update_message("当前已经是最新版本".into());
                }
                state.set_update_result_open(true);
            });
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_start_update(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_update_available() {
                state.set_update_message("当前没有可更新的版本".into());
                state.set_update_result_open(true);
                return;
            }
            state.set_update_progress(0);
            state.set_update_ready(false);
            state.set_update_progress_open(true);
            advance_update_progress(app.as_weak());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_restart_after_update(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_update_ready() {
                return;
            }
            if relaunch_current_exe().is_ok() {
                std::process::exit(0);
            }
            state.set_update_progress_open(false);
            state.set_update_message("重启失败，请手动重新打开客户端。".into());
            state.set_update_result_open(true);
        });
    }

    wire_provider_callbacks(app, store.clone());
    wire_reference_callbacks(app, store.clone());
    wire_prompt_preview_callbacks(app);
    wire_generation_callbacks(app, store.clone());
    wire_viewer_callbacks(app, store.clone());
    wire_notification_callbacks(app, store);
}

fn wire_prompt_preview_callbacks(app: &AppWindow) {
    let state = app.global::<AppState>();
    let app_weak = app.as_weak();
    state.on_refresh_advanced_prompt_preview(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        refresh_advanced_prompt_preview(&app);
    });
    refresh_advanced_prompt_preview(app);
}

fn wire_provider_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_refresh_assets(move || {
            if let Some(app) = app_weak.upgrade() {
                push_assets(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_refresh_inspiration(move || {
            if let Some(app) = app_weak.upgrade() {
                push_inspiration(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_provider_editor(move |id| {
            if let Some(app) = app_weak.upgrade() {
                open_provider_editor(&app, &store.borrow(), id.as_str());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_provider_editor(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_pending_image_model("".into());
                state.set_pending_reasoning_model("".into());
                state.set_provider_used_models(ModelRc::new(VecModel::from(
                    Vec::<SharedString>::new(),
                )));
                state.set_provider_model_options(ModelRc::new(VecModel::from(Vec::<
                    ProviderModelOption,
                >::new(
                ))));
                state.set_provider_editor_open(false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_edit_provider(move |id| {
            if let Some(app) = app_weak.upgrade() {
                open_provider_editor(&app, &store.borrow(), id.as_str());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_save_provider(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let name = state.get_provider_name().trim().to_string();
            if name.is_empty() {
                state.set_provider_message("供应商名称不能为空".into());
                return;
            }
            let existing_models = state.get_fetched_models().iter().collect::<Vec<_>>();
            let endpoint = state.get_provider_endpoint().to_string();
            let key = state.get_provider_api_key().to_string();
            if existing_models.is_empty() && !endpoint.trim().is_empty() && !key.trim().is_empty() {
                state.set_provider_message("正在获取模型...".into());
                let (sender, receiver) =
                    mpsc::channel::<std::result::Result<Vec<String>, String>>();
                std::thread::spawn(move || {
                    let result = list_models(&endpoint, &key).map_err(|err| err.to_string());
                    let _ = sender.send(result);
                });
                poll_save_provider_models(
                    app.as_weak(),
                    store.clone(),
                    Rc::new(RefCell::new(Some(receiver))),
                );
                return;
            }
            save_provider_from_state(&app, &store, None);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_fetch_models(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let endpoint = state.get_provider_endpoint().to_string();
            let key = state.get_provider_api_key().to_string();
            if endpoint.trim().is_empty() || key.trim().is_empty() {
                state.set_provider_message("请先填写 API Key 和 API 请求地址".into());
                return;
            }
            state.set_provider_message("正在获取模型...".into());
            let app_weak = app.as_weak();
            std::thread::spawn(move || {
                let result = list_models(&endpoint, &key);
                let _ = app_weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<AppState>();
                    match result {
                        Ok(models) => {
                            let shared = models
                                .iter()
                                .cloned()
                                .map(SharedString::from)
                                .collect::<Vec<_>>();
                            state.set_fetched_models(ModelRc::new(VecModel::from(shared)));
                            push_provider_model_options(&app);
                            state.set_provider_message("获取模型完成".into());
                        }
                        Err(err) => state.set_provider_message(
                            format!("获取模型失败：{}", zh_error(&err.to_string())).into(),
                        ),
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_test_provider(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let provider = if id.is_empty() {
                ProviderData {
                    id: String::new(),
                    name: state.get_provider_name().to_string(),
                    endpoint: state.get_provider_endpoint().to_string(),
                    remark: String::new(),
                    website: String::new(),
                    api_key: state.get_provider_api_key().to_string(),
                    selected_model: state.get_provider_model_name().to_string(),
                    models: vec![],
                    used_models: vec![],
                }
            } else {
                let id = id.to_string();
                store
                    .borrow()
                    .providers
                    .iter()
                    .find(|p| p.id == id)
                    .cloned()
                    .unwrap_or_else(|| ProviderData {
                        id: String::new(),
                        name: String::new(),
                        endpoint: String::new(),
                        remark: String::new(),
                        website: String::new(),
                        api_key: String::new(),
                        selected_model: String::new(),
                        models: vec![],
                        used_models: vec![],
                    })
            };
            state.set_provider_message("正在测速...".into());
            let app_weak = app.as_weak();
            std::thread::spawn(move || {
                let result = test_provider_connection(&provider);
                let _ = app_weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<AppState>();
                    match result {
                        Ok(_) => state.set_provider_message("测速完成，API 连接正常。".into()),
                        Err(err) => state.set_provider_message(
                            format!("测速失败：{}", zh_error(&err.to_string())).into(),
                        ),
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_image_model(move |provider_id, model| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let provider_id = provider_id.to_string();
                let model = model.to_string();
                state.set_image_provider_id(provider_id.into());
                state.set_image_model(model.clone().into());
                if state.get_reasoning_model().as_str() == model.as_str() {
                    state.set_reasoning_provider_id("".into());
                    state.set_reasoning_model("".into());
                }
                save_local_store(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_reasoning_model(move |provider_id, model| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let provider_id = provider_id.to_string();
                let model = model.to_string();
                state.set_reasoning_provider_id(provider_id.into());
                state.set_reasoning_model(model.clone().into());
                if state.get_image_model().as_str() == model.as_str() {
                    state.set_image_provider_id("".into());
                    state.set_image_model("".into());
                }
                save_local_store(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_set_provider_model_role(move |role, model| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let provider_id = state.get_edit_provider_id().to_string();
                let model = model.to_string();
                state.set_provider_model_name(model.clone().into());
                let (pending_image, pending_reasoning) = next_pending_models(
                    role.as_str(),
                    model.as_str(),
                    state.get_pending_image_model().as_str(),
                    state.get_pending_reasoning_model().as_str(),
                );
                state.set_pending_image_model(pending_image.clone().into());
                state.set_pending_reasoning_model(pending_reasoning.clone().into());
                state.set_pending_model_role(role.clone());
                state.set_pending_model_name(model.clone().into());
                match apply_model_role(role.as_str(), model.as_str()) {
                    ModelRole::Image => {
                        if !provider_id.is_empty() {
                            state.set_image_provider_id(provider_id.clone().into());
                        }
                        state.set_image_model(model.clone().into());
                        if state.get_reasoning_model().as_str() == model.as_str() {
                            state.set_reasoning_provider_id("".into());
                            state.set_reasoning_model("".into());
                        }
                    }
                    ModelRole::Reasoning => {
                        if !provider_id.is_empty() {
                            state.set_reasoning_provider_id(provider_id.clone().into());
                        }
                        state.set_reasoning_model(model.clone().into());
                        if state.get_image_model().as_str() == model.as_str() {
                            state.set_image_provider_id("".into());
                            state.set_image_model("".into());
                        }
                    }
                    ModelRole::None => {}
                }
                save_local_store(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_toggle_provider_model_used(move |model| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let model = model.to_string();
                if model.trim().is_empty() {
                    return;
                }
                let mut used = state
                    .get_provider_used_models()
                    .iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>();
                if used.iter().any(|item| item == &model) {
                    used.retain(|item| item != &model);
                } else {
                    used.push(model);
                }
                let models = state
                    .get_fetched_models()
                    .iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>();
                let used = normalized_used_models(used, &models);
                state.set_provider_used_models(ModelRc::new(VecModel::from(
                    used.into_iter().map(SharedString::from).collect::<Vec<_>>(),
                )));
                push_provider_model_options(&app);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_delete_provider(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            if id.trim().is_empty() {
                return;
            }
            {
                let state = app.global::<AppState>();
                if state.get_image_provider_id().as_str() == id.as_str() {
                    state.set_image_provider_id("".into());
                    state.set_image_model("".into());
                }
                if state.get_reasoning_provider_id().as_str() == id.as_str() {
                    state.set_reasoning_provider_id("".into());
                    state.set_reasoning_model("".into());
                }
                if state.get_edit_provider_id().as_str() == id.as_str() {
                    state.set_provider_editor_open(false);
                }
            }
            {
                let mut store_mut = store.borrow_mut();
                store_mut.providers.retain(|provider| provider.id != id);
                save_local_store(&app, &store_mut);
            }
            app.global::<AppState>()
                .set_provider_message("模型已删除".into());
            push_all(&app, &store.borrow());
        });
    }
}

fn save_provider_from_state(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    fetched_models: Option<Vec<String>>,
) {
    let state = app.global::<AppState>();
    let name = state.get_provider_name().trim().to_string();
    if name.is_empty() {
        state.set_provider_message("供应商名称不能为空".into());
        return;
    }
    let mut models = fetched_models.unwrap_or_else(|| {
        state
            .get_fetched_models()
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
    });
    let selected = state.get_provider_model_name().trim().to_string();
    if !selected.is_empty() && !models.iter().any(|m| m == &selected) {
        models.push(selected.clone());
    }
    let mut store = store.borrow_mut();
    let id = state.get_edit_provider_id().to_string();
    let used_models = normalized_used_models(
        state
            .get_provider_used_models()
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>(),
        &models,
    );
    let provider = ProviderData {
        id: if id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            id.clone()
        },
        name,
        endpoint: state.get_provider_endpoint().trim().to_string(),
        remark: state.get_provider_remark().trim().to_string(),
        website: state.get_provider_website().trim().to_string(),
        api_key: state.get_provider_api_key().trim().to_string(),
        selected_model: selected,
        models,
        used_models,
    };
    let pending_image_model = state.get_pending_image_model().to_string();
    let pending_reasoning_model = state.get_pending_reasoning_model().to_string();
    if model_belongs_to_provider(&provider, &pending_image_model) {
        state.set_image_provider_id(provider.id.clone().into());
        state.set_image_model(pending_image_model.clone().into());
        if pending_reasoning_model == pending_image_model {
            state.set_reasoning_provider_id("".into());
            state.set_reasoning_model("".into());
        }
    }
    if model_belongs_to_provider(&provider, &pending_reasoning_model)
        && pending_reasoning_model != pending_image_model
    {
        state.set_reasoning_provider_id(provider.id.clone().into());
        state.set_reasoning_model(pending_reasoning_model.clone().into());
    }
    if let Some(existing) = store.providers.iter_mut().find(|p| p.id == provider.id) {
        *existing = provider;
    } else {
        store.providers.push(provider);
    }
    state.set_pending_image_model("".into());
    state.set_pending_reasoning_model("".into());
    state.set_provider_editor_open(false);
    push_all(app, &store);
    save_local_store(app, &store);
}

fn poll_save_provider_models(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<Vec<String>, String>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(120), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("获取模型任务已中断，请重试。".to_string()))
                }
            }
        };

        let Some(result) = result else {
            poll_save_provider_models(app_weak, store, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        match result {
            Ok(models) => {
                let shared = models
                    .iter()
                    .cloned()
                    .map(SharedString::from)
                    .collect::<Vec<_>>();
                state.set_fetched_models(ModelRc::new(VecModel::from(shared)));
                state.set_provider_message("获取模型完成".into());
                save_provider_from_state(&app, &store, Some(models));
            }
            Err(reason) => {
                state.set_provider_message(format!("获取模型失败：{}", zh_error(&reason)).into());
            }
        }
    });
}

fn wire_reference_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(files) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_files()
            {
                let category =
                    resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
                let max_references = max_reference_images_for_category(&category);
                let mut store = store.borrow_mut();
                let references = references_for_category_mut(&mut store.references, &category);
                if references.len() >= max_references {
                    app.global::<AppState>()
                        .set_generation_status(reference_limit_message(max_references).into());
                    return;
                }
                for path in files {
                    if references.len() >= max_references {
                        break;
                    }
                    if let Ok(image) = load_image(&path) {
                        references.push(ReferenceData {
                            id: Uuid::new_v4().to_string(),
                            image,
                            source_path: path.display().to_string(),
                        });
                    }
                }
                if references.len() >= max_references {
                    app.global::<AppState>()
                        .set_generation_status(reference_limit_message(max_references).into());
                }
                push_references(&app, &store);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_paste_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let max_references = max_reference_images_for_category(&category);
            let Ok(mut clipboard) = arboard::Clipboard::new() else {
                return false;
            };
            let Ok(img) = clipboard.get_image() else {
                return false;
            };
            let mut store = store.borrow_mut();
            let references = references_for_category_mut(&mut store.references, &category);
            if references.len() >= max_references {
                state.set_generation_status(reference_limit_message(max_references).into());
                return true;
            }
            let image = image_from_clipboard(img);
            references.push(ReferenceData {
                id: Uuid::new_v4().to_string(),
                image,
                source_path: String::new(),
            });
            push_references(&app, &store);
            state.set_generation_status("已从剪贴板粘贴参考图".into());
            true
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference_from_drag(move |mime_type, data| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            add_reference_from_drag_data(&app, &store, mime_type.as_str(), data.as_str())
        });
    }

    state.on_start_thumbnail_drag_preview(move |data| {
        let Some(path) = drag_data_to_path(data.as_str()) else {
            return false;
        };
        drag_preview::start_thumbnail_drag_preview(path)
    });

    state.on_start_thumbnail_file_drag(move |data| {
        let Some(path) = drag_data_to_path(data.as_str()) else {
            return false;
        };
        drag_preview::start_thumbnail_file_drag(path)
    });

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_remove_reference(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let id = id.to_string();
                let category =
                    resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
                references_for_category_mut(&mut store.borrow_mut().references, &category)
                    .retain(|r| r.id != id);
                push_references(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_reference(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            let category =
                resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
            let store_ref = store.borrow();
            let Some(item) = references_for_category(&store_ref.references, &category)
                .iter()
                .find(|r| r.id == id)
                .cloned()
            else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_viewer_id(item.id.into());
            state.set_viewer_source("reference".into());
            state.set_viewer_image(item.image);
            state.set_viewer_title("参考图".into());
            state.set_viewer_prompt("".into());
            state.set_viewer_prompt_lines(1);
            state.set_viewer_time("".into());
            state.set_viewer_ratio("".into());
            state.set_viewer_quality("".into());
            state.set_viewer_model("".into());
            state.set_viewer_width(0);
            state.set_viewer_height(0);
            state.set_viewer_cutout_done(false);
            state.set_viewer_remove_black_done(false);
            state.set_viewer_upscale_done(false);
            state.set_viewer_open(true);
        });
    }
}

fn wire_generation_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_generate(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_generation(&app, store.clone(), None, true, None, None);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_stop_generation(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            stop_generation(&app, &store);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_optimize_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_current_prompt(&app, store.clone(), false);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_visual_optimize_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_current_prompt(&app, store.clone(), true);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_translate_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            translate_current_prompt(&app, store.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_conversation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.global::<AppState>().set_current_conversation_id(id);
            push_generations(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_regenerate(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let prompt = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id.to_string())
                .map(|g| g.prompt.clone());
            if let Some(conversation_id) = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id.to_string())
                .map(|g| g.conversation_id.clone())
            {
                app.global::<AppState>()
                    .set_current_conversation_id(conversation_id.into());
            }
            start_generation(&app, store.clone(), prompt, false, None, None);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_retry_generation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            retry_failed_generation(&app, store.clone(), id.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_quote_generation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            if let Some(item) = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id)
                .cloned()
            {
                let state = app.global::<AppState>();
                state.set_quote_title(item.title.into());
                state.set_quote_prompt(item.prompt.into());
                state.set_quote_ratio(item.ratio.into());
                state.set_quote_quality(item.quality.into());
                state.set_quote_width(item.width);
                state.set_quote_height(item.height);
            }
        });
    }
}

fn optimize_current_prompt(app: &AppWindow, store: Rc<RefCell<Store>>, visual_mode: bool) {
    let state = app.global::<AppState>();
    if state.get_optimizing_prompt() {
        return;
    }
    let raw_prompt = state.get_prompt().trim().to_string();
    if raw_prompt.is_empty() {
        state.set_generation_status("请输入需要优化的提示词".into());
        return;
    }
    let reasoning_id = state.get_reasoning_provider_id().to_string();
    let reasoning_model = state.get_reasoning_model().to_string();
    let store_ref = store.borrow();
    let Some(reasoning_provider) = store_ref
        .providers
        .iter()
        .find(|p| p.id == reasoning_id)
        .cloned()
    else {
        state.set_generation_status("请先选择推理模型".into());
        return;
    };
    if reasoning_model.trim().is_empty() {
        state.set_generation_status("请先选择推理模型".into());
        return;
    }
    let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
    let max_references = max_reference_images_for_category(&category);
    let references = references_for_category(&store_ref.references, &category)
        .iter()
        .take(max_references)
        .map(|r| r.image.clone())
        .collect::<Vec<_>>();
    drop(store_ref);
    if visual_mode && references.is_empty() {
        state.set_generation_status("请先上传参考图".into());
        return;
    }
    if !charge_credits(
        app,
        PROMPT_SERVICE_CREDIT_COST,
        "GPT-5.5 提示词优化",
        "优化提示词",
    ) {
        return;
    }

    state.set_generation_status("正在优化提示词...".into());
    state.set_optimizing_prompt(true);
    let ratio = resolve_ratio_for_category(
        &category,
        &state.get_ratio().to_string(),
        &raw_prompt,
        &state.get_quote_ratio().to_string(),
    );
    let quality = state.get_quality().to_string();
    let translate_prompt = state.get_translate_prompt();
    let output_language = if translate_prompt || state.get_language().as_str() == "en" {
        PromptLanguage::English
    } else {
        PromptLanguage::Chinese
    };
    let prompt_controls = PromptControls {
        category: category.clone(),
        creation: normalize_creation_mode_for_category(
            &category,
            &state.get_creation_mode().to_string(),
        ),
        style: state.get_style_mode().to_string(),
        view: state.get_view_mode().to_string(),
        weather: state.get_weather_mode().to_string(),
        time: state.get_time_mode().to_string(),
        light: state.get_light_mode().to_string(),
    };
    let controlled_prompt = prompt_with_controls(&raw_prompt, &prompt_controls, output_language);
    let controlled_prompt = if category == "action-sequence" {
        append_action_sequence_instruction(&controlled_prompt, output_language)
    } else {
        controlled_prompt
    };
    let prompt_for_optimization = append_parameter_priority_instruction(
        &controlled_prompt,
        &category,
        &ratio,
        &quality,
        output_language,
    );
    let quote = QuoteContext {
        title: state.get_quote_title().to_string(),
        prompt: state.get_quote_prompt().to_string(),
        ratio: state.get_quote_ratio().to_string(),
        quality: state.get_quote_quality().to_string(),
        width: state.get_quote_width(),
        height: state.get_quote_height(),
    };
    let reference_images = references
        .iter()
        .filter_map(|image| image_to_data_url(image).ok())
        .collect::<Vec<_>>();
    let (sender, receiver) = mpsc::channel::<std::result::Result<String, String>>();
    std::thread::spawn(move || {
        let result = optimize_prompt(
            &reasoning_provider,
            &reasoning_model,
            &prompt_for_optimization,
            &category,
            &ratio,
            &quality,
            &quote,
            &reference_images,
            translate_prompt,
            output_language,
            visual_mode,
        )
        .map_err(|err| zh_error(&err.to_string()));
        let _ = sender.send(result);
    });
    poll_prompt_optimize_result(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
}

fn poll_prompt_optimize_result(
    app_weak: Weak<AppWindow>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<String, String>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(120), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("提示词优化任务已中断，请重试".to_string()))
                }
            }
        };

        let Some(result) = result else {
            poll_prompt_optimize_result(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        match result {
            Ok(prompt) => {
                state.set_prompt(prompt.into());
                state.set_generation_status("提示词优化完成".into());
                state.set_optimizing_prompt(false);
            }
            Err(reason) => {
                state.set_generation_status(format!("提示词优化失败：{}", reason).into());
                state.set_optimizing_prompt(false);
                refund_credits(
                    &app,
                    PROMPT_SERVICE_CREDIT_COST,
                    "提示词优化积分退回",
                    "优化失败自动退回",
                );
            }
        }
    });
}

fn translate_current_prompt(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();
    if state.get_translating_prompt() {
        return;
    }
    let raw_prompt = state.get_prompt().trim().to_string();
    if raw_prompt.is_empty() {
        state.set_translate_prompt(false);
        return;
    }

    let reasoning_id = state.get_reasoning_provider_id().to_string();
    let reasoning_model = state.get_reasoning_model().to_string();
    let store_ref = store.borrow();
    let Some(reasoning_provider) = store_ref
        .providers
        .iter()
        .find(|p| p.id == reasoning_id)
        .cloned()
    else {
        state.set_generation_status("请先选择推理模型".into());
        state.set_translate_prompt(false);
        return;
    };
    if reasoning_model.trim().is_empty() {
        state.set_generation_status("请先选择推理模型".into());
        state.set_translate_prompt(false);
        return;
    }
    drop(store_ref);
    if !charge_credits(
        app,
        PROMPT_SERVICE_CREDIT_COST,
        "GPT-5.5 提示词翻译",
        "翻译提示词",
    ) {
        state.set_translate_prompt(false);
        return;
    }

    state.set_translating_prompt(true);
    state.set_generation_status("正在翻译提示词...".into());
    let (sender, receiver) = mpsc::channel::<std::result::Result<String, String>>();
    std::thread::spawn(move || {
        let result = translate_prompt_text(&reasoning_provider, &reasoning_model, &raw_prompt)
            .map_err(|err| zh_error(&err.to_string()));
        let _ = sender.send(result);
    });
    poll_prompt_translate_result(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
}

fn poll_prompt_translate_result(
    app_weak: Weak<AppWindow>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<String, String>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(120), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("提示词翻译任务已中断，请重试".to_string()))
                }
            }
        };

        let Some(result) = result else {
            poll_prompt_translate_result(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_translating_prompt(false);
        match result {
            Ok(prompt) => {
                if !prompt.trim().is_empty() {
                    state.set_prompt(prompt.into());
                }
                state.set_generation_status("提示词翻译完成".into());
            }
            Err(reason) => {
                state.set_generation_status(format!("提示词翻译失败：{}", reason).into());
                state.set_translate_prompt(false);
                refund_credits(
                    &app,
                    PROMPT_SERVICE_CREDIT_COST,
                    "提示词翻译积分退回",
                    "翻译失败自动退回",
                );
            }
        }
    });
}

fn wire_viewer_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_viewer(move |id, source| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open_viewer(&app, &store.borrow(), id.as_str(), source.as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_viewer(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_viewer_message("".into());
                state.set_viewer_open(false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_prev(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            move_viewer(&app, &store.borrow(), -1);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_next(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            move_viewer(&app, &store.borrow(), 1);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_download_asset(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            download_asset(&app, &store, id.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_viewer_copy_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            copy_viewer_image(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_download_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            download_viewer_image(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_cutout_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_viewer_image_processing(&app, store.clone(), ProcessImageMode::Cutout);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_remove_black(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_viewer_image_processing(&app, store.clone(), ProcessImageMode::RemoveBlack);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_open_upscale_dialog(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_viewer_upscale_done() {
                state.set_viewer_message(
                    processing_done_message(
                        &app,
                        ProcessImageMode::Upscale {
                            scale: 2,
                            target_long_edge: 2048,
                        },
                    )
                    .into(),
                );
                return;
            }
            state.set_upscale_scale(2);
            state.set_upscale_quality("2K".into());
            state.set_upscale_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_upscale_dialog(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_viewer_processing() {
                state.set_upscale_open(false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_start_upscale_image(move |scale, quality| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let scale = scale.clamp(2, 4) as u32;
            let target_long_edge = upscale_quality_long_edge(quality.as_str());
            start_viewer_image_processing(
                &app,
                store.clone(),
                ProcessImageMode::Upscale {
                    scale,
                    target_long_edge,
                },
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_regenerate(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let prompt = app.global::<AppState>().get_viewer_prompt().to_string();
            app.global::<AppState>().set_viewer_open(false);
            start_generation(&app, store.clone(), Some(prompt), false, None, None);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_viewer_edit(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_prompt(state.get_viewer_prompt());
            state.set_quote_title(state.get_viewer_title());
            state.set_quote_prompt(state.get_viewer_prompt());
            state.set_quote_ratio(state.get_viewer_ratio());
            state.set_quote_quality(state.get_viewer_quality());
            state.set_viewer_open(false);
            navigate_to(&app, "generation");
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_use_same(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let max_references = max_reference_images_for_category(&category);
            if references_for_category(&store.borrow().references, &category).len()
                >= max_references
            {
                state.set_viewer_message(reference_limit_message(max_references).into());
                return;
            }
            let prompt = state.get_viewer_prompt().to_string();
            let title = short_text(&prompt, 10);
            let conversation_id = Uuid::new_v4().to_string();
            let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
            conversations.insert(
                0,
                ConversationItem {
                    id: SharedString::from(conversation_id.clone()),
                    title: SharedString::from(title),
                    image: Image::default(),
                    loading: false,
                },
            );
            state.set_conversations(ModelRc::new(VecModel::from(conversations)));
            state.set_current_conversation_id(conversation_id.into());
            {
                let mut store_mut = store.borrow_mut();
                references_for_category_mut(&mut store_mut.references, &category).push(
                    ReferenceData {
                        id: Uuid::new_v4().to_string(),
                        image: state.get_viewer_image(),
                        source_path: String::new(),
                    },
                );
            }
            push_references(&app, &store.borrow());
            state.set_viewer_open(false);
            state.set_prompt(prompt.into());
            navigate_to(&app, "generation");
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_use_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let max_references = max_reference_images_for_category(&category);
            if references_for_category(&store.borrow().references, &category).len()
                >= max_references
            {
                state.set_viewer_message(reference_limit_message(max_references).into());
                return;
            }
            {
                let mut store_mut = store.borrow_mut();
                references_for_category_mut(&mut store_mut.references, &category).push(
                    ReferenceData {
                        id: Uuid::new_v4().to_string(),
                        image: state.get_viewer_image(),
                        source_path: String::new(),
                    },
                );
            }
            push_references(&app, &store.borrow());
            state.set_viewer_open(false);
            navigate_to(&app, "generation");
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_request_delete_asset(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_pending_delete_id(id);
                state.set_pending_delete_source(state.get_viewer_source());
                state.set_delete_confirm_open(true);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_confirm_delete(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let id = state.get_pending_delete_id().to_string();
            let source = state.get_pending_delete_source().to_string();
            {
                let mut store_mut = store.borrow_mut();
                match source.as_str() {
                    "asset" => store_mut.assets.retain(|a| a.id != id),
                    "inspiration" => store_mut.inspiration.retain(|a| a.id != id),
                    "reference" => {
                        let category = resolve_category(&state.get_asset_type().to_string(), "");
                        references_for_category_mut(&mut store_mut.references, &category)
                            .retain(|item| item.id != id);
                    }
                    _ => store_mut.generations.retain(|a| a.id != id),
                }
                save_local_store(&app, &store_mut);
            }
            state.set_pending_delete_id("".into());
            state.set_pending_delete_source("".into());
            state.set_delete_confirm_open(false);
            state.set_viewer_open(false);
            push_all(&app, &store.borrow());
        });
    }
}

fn add_reference_from_drag_data(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    let Some(path) = drag_data_to_path(data) else {
        return false;
    };
    add_reference_from_path(app, store, &path)
}

fn drag_data_to_path(data: &str) -> Option<PathBuf> {
    let raw = data
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let raw = if let Some(rest) = raw.strip_prefix("file:///") {
        rest
    } else if let Some(rest) = raw.strip_prefix("file://") {
        rest
    } else {
        raw
    };
    let decoded = percent_decode_path(raw);
    #[cfg(windows)]
    let decoded = decoded.trim_start_matches('/').replace('/', "\\");
    Some(PathBuf::from(decoded))
}

fn file_uri_for_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "failed" {
        return String::new();
    }
    #[cfg(windows)]
    {
        let normalized = path.replace('\\', "/");
        let encoded = percent_encode_uri_path(&normalized);
        if encoded.starts_with("//") {
            format!("file:{encoded}")
        } else {
            format!("file:///{encoded}")
        }
    }
    #[cfg(not(windows))]
    {
        let encoded = percent_encode_uri_path(path);
        if encoded.starts_with('/') {
            format!("file://{encoded}")
        } else {
            format!("file:///{encoded}")
        }
    }
}

fn percent_encode_uri_path(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                output.push(*byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn percent_decode_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn add_reference_from_path(app: &AppWindow, store: &Rc<RefCell<Store>>, path: &Path) -> bool {
    let state = app.global::<AppState>();
    if !path.exists() {
        state.set_generation_status("参考图文件不存在".into());
        return false;
    }
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    let max_references = max_reference_images_for_category(&category);
    let Ok(image) = load_image(path) else {
        state.set_generation_status("无法读取参考图".into());
        return false;
    };
    let mut store = store.borrow_mut();
    let references = references_for_category_mut(&mut store.references, &category);
    if references.len() >= max_references {
        state.set_generation_status(reference_limit_message(max_references).into());
        return true;
    }
    references.push(ReferenceData {
        id: Uuid::new_v4().to_string(),
        image,
        source_path: path.display().to_string(),
    });
    push_references(app, &store);
    state.set_generation_status("已添加参考图".into());
    true
}

fn wire_notification_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();
    let app_weak = app.as_weak();
    let store_for_single = store.clone();
    state.on_mark_notification_read(move |id| {
        if let Some(app) = app_weak.upgrade() {
            let mut store = store_for_single.borrow_mut();
            let id = id.to_string();
            if let Some(item) = store.notifications.iter_mut().find(|n| n.id == id) {
                item.read = true;
            }
            push_notifications(&app, &store);
            save_local_store(&app, &store);
        }
    });

    let app_weak = app.as_weak();
    let store_for_all = store.clone();
    state.on_mark_all_notifications_read(move || {
        if let Some(app) = app_weak.upgrade() {
            let mut store = store_for_all.borrow_mut();
            for item in &mut store.notifications {
                item.read = true;
            }
            push_notifications(&app, &store);
            save_local_store(&app, &store);
        }
    });

    let app_weak = app.as_weak();
    let store_for_clear = store.clone();
    state.on_clear_all_notifications(move || {
        if let Some(app) = app_weak.upgrade() {
            let mut store = store_for_clear.borrow_mut();
            store.notifications.clear();
            push_notifications(&app, &store);
            save_local_store(&app, &store);
        }
    });
}

fn current_workspace_category(app: &AppWindow) -> String {
    resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "")
}

fn category_is_generating(category: &str) -> bool {
    ACTIVE_GENERATIONS.with(|tasks| tasks.borrow().contains_key(category))
}

fn active_generation_matches(category: &str, task_id: &str) -> bool {
    ACTIVE_GENERATIONS.with(|tasks| {
        tasks
            .borrow()
            .get(category)
            .is_some_and(|task| task.task_id == task_id)
    })
}

fn insert_active_generation(task: ActiveGeneration) {
    ACTIVE_GENERATIONS.with(|tasks| {
        tasks.borrow_mut().insert(task.category.clone(), task);
    });
}

fn remove_active_generation(category: &str, task_id: &str) -> Option<ActiveGeneration> {
    ACTIVE_GENERATIONS.with(|tasks| {
        let mut tasks = tasks.borrow_mut();
        if tasks
            .get(category)
            .is_some_and(|task| task.task_id == task_id)
        {
            tasks.remove(category)
        } else {
            None
        }
    })
}

fn set_generation_status_for_category(app: &AppWindow, category: &str, status: &str) {
    GENERATION_STATUS_BY_CATEGORY.with(|statuses| {
        statuses
            .borrow_mut()
            .insert(category.to_string(), status.to_string());
    });
    if current_workspace_category(app) == category {
        app.global::<AppState>()
            .set_generation_status(status.to_string().into());
    }
}

fn update_active_generation_progress(
    app: &AppWindow,
    category: &str,
    task_id: &str,
    progress: i32,
    eta: i32,
) {
    ACTIVE_GENERATIONS.with(|tasks| {
        if let Some(task) = tasks.borrow_mut().get_mut(category) {
            if task.task_id == task_id {
                task.progress = progress;
                task.eta = eta;
            }
        }
    });
    if current_workspace_category(app) == category {
        let state = app.global::<AppState>();
        state.set_generation_progress(progress);
        state.set_generation_eta(eta);
    }
}

fn mark_active_generation_image_completed(
    app: &AppWindow,
    category: &str,
    task_id: &str,
    success: bool,
) -> Option<ActiveGeneration> {
    let active = ACTIVE_GENERATIONS.with(|tasks| {
        let mut tasks = tasks.borrow_mut();
        let task = tasks.get_mut(category)?;
        if task.task_id != task_id {
            return None;
        }
        task.completed_count = (task.completed_count + 1).min(task.total_count.max(1));
        task.loading_count = (task.total_count - task.completed_count).max(0);
        if success {
            task.success_count += 1;
        } else {
            task.failed_count += 1;
        }
        let total = task.total_count.max(1);
        task.progress = (8 + task.completed_count * 88 / total).clamp(1, 96);
        task.eta = if task.loading_count > 0 {
            IMAGE_GENERATION_WAIT_SECS as i32
        } else {
            0
        };
        Some(task.clone())
    });
    sync_generation_state_for_current_category(app);
    active
}

fn sync_generation_state_for_current_category(app: &AppWindow) {
    let state = app.global::<AppState>();
    let category = current_workspace_category(app);
    let active = ACTIVE_GENERATIONS.with(|tasks| tasks.borrow().get(&category).cloned());
    if let Some(task) = active {
        state.set_generating(true);
        state.set_generation_loading_count(task.loading_count);
        state.set_generation_task_id(task.task_id.into());
        state.set_generation_active_category(category.clone().into());
        state.set_generation_active_prompt(task.prompt.into());
        state.set_generation_active_credit_cost(task.credit_cost);
        state.set_generation_progress(task.progress);
        state.set_generation_eta(task.eta);
        let status = GENERATION_STATUS_BY_CATEGORY.with(|statuses| {
            statuses
                .borrow()
                .get(&category)
                .cloned()
                .unwrap_or_else(|| "正在生成...".to_string())
        });
        state.set_generation_status(status.into());
    } else {
        state.set_generating(false);
        state.set_generation_loading_count(0);
        state.set_generation_task_id("".into());
        state.set_generation_active_category("".into());
        state.set_generation_active_prompt("".into());
        state.set_generation_active_credit_cost(0);
        state.set_generation_progress(0);
        state.set_generation_eta(0);
        let status = GENERATION_STATUS_BY_CATEGORY.with(|statuses| {
            statuses
                .borrow()
                .get(&category)
                .cloned()
                .unwrap_or_default()
        });
        state.set_generation_status(status.into());
    }
}

fn finish_conversation_placeholder(state: &AppState, conversation_id: &str, image: Option<Image>) {
    let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
    if let Some(row) = conversations
        .iter_mut()
        .find(|c| c.loading && c.id.as_str() == conversation_id)
    {
        if let Some(image) = image {
            row.image = image;
        }
        row.loading = false;
    }
    state.set_conversations(ModelRc::new(VecModel::from(conversations)));
}

fn start_generation(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    override_prompt: Option<String>,
    create_conversation: bool,
    retry_failed_id: Option<String>,
    forced_count: Option<i32>,
) {
    let state = app.global::<AppState>();
    let input_prompt = state.get_prompt().trim().to_string();
    let raw_prompt = if !input_prompt.is_empty() {
        input_prompt
    } else {
        override_prompt.unwrap_or_default().trim().to_string()
    };
    if raw_prompt.trim().is_empty() {
        state.set_generation_status("请输入生成需求".into());
        return;
    }
    if !state.get_logged_in() {
        state.set_auth_open(true);
        return;
    }
    let provider_id = state.get_image_provider_id().to_string();
    let image_model = state.get_image_model().to_string();
    let store_ref = store.borrow();
    if store_ref.providers.is_empty()
        || provider_id.trim().is_empty()
        || image_model.trim().is_empty()
    {
        state.set_model_required_open(true);
        state.set_generation_status("请先添加模型".into());
        return;
    }
    let Some(image_provider) = store_ref
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
    else {
        state.set_model_required_open(true);
        state.set_generation_status("请先添加模型".into());
        return;
    };
    let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
    if category_is_generating(&category) {
        stop_generation(app, &store);
        return;
    }
    let max_references = max_reference_images_for_category(&category);
    let original_references = references_for_category(&store_ref.references, &category)
        .iter()
        .take(max_references)
        .cloned()
        .collect::<Vec<_>>();
    let references = original_references
        .iter()
        .map(|r| r.image.clone())
        .collect::<Vec<_>>();
    drop(store_ref);

    let ratio = resolve_ratio_for_category(
        &category,
        &state.get_ratio().to_string(),
        &raw_prompt,
        &state.get_quote_ratio().to_string(),
    );
    let quality = state.get_quality().to_string();
    let count = forced_count.unwrap_or_else(|| {
        if category == "action-sequence" {
            1
        } else {
            state.get_count().clamp(1, 4)
        }
    });
    let mode = state.get_mode().to_string();
    let credit_per_image = image_generation_credit_cost(&quality);
    let credit_cost = credit_per_image * count;
    if state.get_credit_balance() < credit_cost {
        state.set_generation_status("积分不足以支持本次生图，请前往充值".into());
        state.set_credit_insufficient_message("积分不足以支持本次生图，请前往充值".into());
        state.set_credit_insufficient_open(true);
        return;
    }
    if !charge_credits(
        app,
        credit_cost,
        "GPT Image 2 生图",
        &format!("{} x {} 张", quality, count),
    ) {
        return;
    }

    if let Some(retry_failed_id) = retry_failed_id.as_deref() {
        let mut store_mut = store.borrow_mut();
        store_mut
            .generations
            .retain(|item| item.id != retry_failed_id);
        save_local_store(app, &store_mut);
        push_all(app, &store_mut);
    }

    let conversation_id = if create_conversation {
        Uuid::new_v4().to_string()
    } else {
        state.get_current_conversation_id().to_string()
    };
    let task_id = Uuid::new_v4().to_string();
    insert_active_generation(ActiveGeneration {
        task_id: task_id.clone(),
        category: category.clone(),
        conversation_id: conversation_id.clone(),
        prompt: raw_prompt.clone(),
        credit_cost,
        credit_per_image,
        total_count: count,
        loading_count: count,
        completed_count: 0,
        success_count: 0,
        failed_count: 0,
        progress: 1,
        eta: IMAGE_GENERATION_WAIT_SECS as i32,
    });
    set_generation_status_for_category(app, &category, "正在生成...");
    sync_generation_state_for_current_category(app);
    navigate_to(app, "generation");

    let quote = QuoteContext {
        title: state.get_quote_title().to_string(),
        prompt: state.get_quote_prompt().to_string(),
        ratio: state.get_quote_ratio().to_string(),
        quality: state.get_quote_quality().to_string(),
        width: state.get_quote_width(),
        height: state.get_quote_height(),
    };
    let prompt_controls = PromptControls {
        category: category.clone(),
        creation: normalize_creation_mode_for_category(
            &category,
            &state.get_creation_mode().to_string(),
        ),
        style: state.get_style_mode().to_string(),
        view: state.get_view_mode().to_string(),
        weather: state.get_weather_mode().to_string(),
        time: state.get_time_mode().to_string(),
        light: state.get_light_mode().to_string(),
    };
    let prompt_language = if state.get_translate_prompt() || state.get_language().as_str() == "en" {
        PromptLanguage::English
    } else {
        PromptLanguage::Chinese
    };
    let generation_prompt = build_generation_prompt(
        &raw_prompt,
        &prompt_controls,
        &quote,
        &category,
        &ratio,
        &quality,
        prompt_language,
    );
    let reference_images = references
        .iter()
        .filter_map(|image| image_to_data_url(image).ok())
        .collect::<Vec<_>>();

    state.set_prompt("".into());
    state.set_quote_title("".into());
    state.set_quote_prompt("".into());
    state.set_quote_ratio("".into());
    state.set_quote_quality("".into());
    state.set_quote_width(0);
    state.set_quote_height(0);
    {
        let mut store_mut = store.borrow_mut();
        references_for_category_mut(&mut store_mut.references, &category).clear();
        push_references(app, &store_mut);
    }

    if create_conversation {
        let placeholder = ConversationItem {
            id: SharedString::from(conversation_id.clone()),
            title: SharedString::from(short_text(&raw_prompt, 10)),
            image: Image::default(),
            loading: true,
        };
        let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
        conversations.insert(0, placeholder);
        state.set_conversations(ModelRc::new(VecModel::from(conversations)));
        state.set_current_conversation_id(conversation_id.clone().into());
    }

    let (sender, receiver) = mpsc::channel::<GenerationOutcome>();
    let generation_prompt_for_thread = generation_prompt.clone();
    let ratio_for_thread = ratio.clone();
    let quality_for_thread = quality.clone();
    let image_model_for_thread = image_model.clone();
    std::thread::spawn(move || {
        let sent_prompt = generation_prompt_for_thread;
        for request_count in image_request_batches_for_count(count) {
            let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
            match request_image_batch(
                &image_provider,
                &image_model_for_thread,
                &sent_prompt,
                request_count,
                &ratio_for_thread,
                &quality_for_thread,
                &reference_images,
            ) {
                Ok(batch) => {
                    let Some(bytes) = batch.images.into_iter().next() else {
                        let _ = sender.send(GenerationOutcome::ImageFailure {
                            reason: "API did not return usable images".to_string(),
                            optimized: sent_prompt.clone(),
                            time,
                        });
                        continue;
                    };
                    if sender
                        .send(GenerationOutcome::ImageSuccess {
                            bytes,
                            optimized: sent_prompt.clone(),
                            time,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(err) => {
                    if sender
                        .send(GenerationOutcome::ImageFailure {
                            reason: zh_error(&err.to_string()),
                            optimized: sent_prompt.clone(),
                            time,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
        let _ = sender.send(GenerationOutcome::Finished {
            time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        });
    });

    let receiver = Rc::new(RefCell::new(Some(receiver)));
    poll_generation_stream(
        app.as_weak(),
        store,
        receiver,
        raw_prompt,
        category,
        mode,
        ratio,
        quality,
        image_model,
        conversation_id,
        create_conversation,
        original_references,
        quote,
        task_id,
        Instant::now(),
    );
}

fn retry_failed_generation(app: &AppWindow, store: Rc<RefCell<Store>>, id: String) {
    let item = {
        let store_ref = store.borrow();
        store_ref
            .generations
            .iter()
            .find(|item| item.id == id && item.source_path == "failed")
            .cloned()
    };
    let Some(item) = item else {
        app.global::<AppState>()
            .set_generation_status("未找到可重试的失败图片".into());
        return;
    };
    if item.prompt.trim().is_empty() {
        app.global::<AppState>()
            .set_generation_status("失败图片没有可重试的提示词".into());
        return;
    }
    let state = app.global::<AppState>();
    state.set_asset_type(item.category.clone().into());
    state.set_mode(item.kind.clone().into());
    state.set_ratio(item.ratio.clone().into());
    state.set_quality(item.quality.clone().into());
    state.set_count(1);
    state.set_prompt(item.prompt.clone().into());
    if !item.conversation_id.trim().is_empty() {
        state.set_current_conversation_id(item.conversation_id.clone().into());
    }
    start_generation(app, store, Some(item.prompt), false, Some(item.id), Some(1));
}

fn stop_generation(app: &AppWindow, store: &Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();
    let category = current_workspace_category(app);
    let task_id = ACTIVE_GENERATIONS.with(|tasks| {
        tasks
            .borrow()
            .get(&category)
            .map(|task| task.task_id.clone())
    });
    let Some(task_id) = task_id else {
        sync_generation_state_for_current_category(app);
        return;
    };
    let Some(task) = remove_active_generation(&category, &task_id) else {
        sync_generation_state_for_current_category(app);
        return;
    };
    set_generation_status_for_category(app, &category, "已停止生成");
    sync_generation_state_for_current_category(app);
    if !task.prompt.trim().is_empty() {
        state.set_prompt(task.prompt.clone().into());
    }
    finish_conversation_placeholder(&state, &task.conversation_id, None);
    push_references(app, &store.borrow());
    refund_credits(app, task.credit_cost, "生图积分退回", "用户停止生成");
}

fn add_stream_success_item(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    raw_prompt: &str,
    category: &str,
    mode: &str,
    quality: &str,
    image_model: &str,
    conversation_id: &str,
    optimized: &str,
    time: &str,
    bytes: &[u8],
) -> Result<Image> {
    let (bytes, image, width, height) = generated_image_from_bytes(bytes, quality)?;
    let source_path = save_generated_bytes(app, &bytes, raw_prompt)?;
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        title: short_text(raw_prompt, 18),
        category: category.to_string(),
        kind: mode.to_string(),
        time: time.to_string(),
        prompt: display_generation_prompt(optimized),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality.to_string(),
        model: image_model.to_string(),
        width,
        height,
        image,
        source_path,
        cutout_done: false,
        remove_black_done: false,
        upscale_done: false,
    };
    let conversation_image = item.image.clone();
    let mut store_mut = store.borrow_mut();
    store_mut.assets.insert(0, item.clone());
    store_mut.generations.insert(0, item);
    store_mut.notifications.insert(
        0,
        NotificationData {
            id: Uuid::new_v4().to_string(),
            title: format!("Generation succeeded: {}", short_text(raw_prompt, 24)),
            model: image_model.to_string(),
            time: time.to_string(),
            reason: String::new(),
            success: true,
            read: false,
        },
    );
    save_local_store(app, &store_mut);
    push_all(app, &store_mut);
    Ok(conversation_image)
}

fn add_stream_failure_item(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    raw_prompt: &str,
    category: &str,
    mode: &str,
    ratio: &str,
    quality: &str,
    image_model: &str,
    conversation_id: &str,
    reason: &str,
    time: &str,
) {
    let mut store_mut = store.borrow_mut();
    store_mut.generations.insert(
        0,
        AssetData {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            title: short_text(raw_prompt, 18),
            category: category.to_string(),
            kind: mode.to_string(),
            time: time.to_string(),
            prompt: raw_prompt.to_string(),
            ratio: ratio.to_string(),
            quality: quality.to_string(),
            model: image_model.to_string(),
            width: 0,
            height: 0,
            image: Image::default(),
            source_path: "failed".to_string(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
        },
    );
    store_mut.notifications.insert(
        0,
        NotificationData {
            id: Uuid::new_v4().to_string(),
            title: format!("Generation failed: {}", short_text(raw_prompt, 24)),
            model: image_model.to_string(),
            time: time.to_string(),
            reason: reason.to_string(),
            success: false,
            read: false,
        },
    );
    save_local_store(app, &store_mut);
    push_all(app, &store_mut);
}

fn restore_stream_inputs(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    raw_prompt: &str,
    category: &str,
    original_references: Vec<ReferenceData>,
    original_quote: QuoteContext,
) {
    let state = app.global::<AppState>();
    let mut store_mut = store.borrow_mut();
    if current_workspace_category(app) == category {
        state.set_prompt(raw_prompt.to_string().into());
        state.set_quote_title(original_quote.title.into());
        state.set_quote_prompt(original_quote.prompt.into());
        state.set_quote_ratio(original_quote.ratio.into());
        state.set_quote_quality(original_quote.quality.into());
        state.set_quote_width(original_quote.width);
        state.set_quote_height(original_quote.height);
    } else {
        set_prompt_draft_for_category(
            &mut store_mut.prompt_drafts,
            category,
            raw_prompt.to_string(),
        );
    }
    *references_for_category_mut(&mut store_mut.references, category) = original_references;
    save_local_store(app, &store_mut);
    push_all(app, &store_mut);
}

fn set_stream_final_status(app: &AppWindow, category: &str, success_count: i32, failed_count: i32) {
    if failed_count <= 0 {
        set_generation_status_for_category(app, category, "生成成功");
    } else if success_count > 0 {
        set_generation_status_for_category(app, category, "部分生成失败");
    } else {
        set_generation_status_for_category(app, category, "生成失败");
    }
}

fn poll_generation_stream(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<GenerationOutcome>>>>,
    raw_prompt: String,
    category: String,
    mode: String,
    ratio: String,
    quality: String,
    image_model: String,
    conversation_id: String,
    create_conversation: bool,
    original_references: Vec<ReferenceData>,
    original_quote: QuoteContext,
    task_id: String,
    started_at: Instant,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if let Some(app) = app_weak.upgrade() {
            if !active_generation_matches(&category, &task_id) {
                return;
            }
            let elapsed = started_at.elapsed().as_secs() as i32;
            let wait_secs = IMAGE_GENERATION_WAIT_SECS as i32;
            update_active_generation_progress(
                &app,
                &category,
                &task_id,
                (8 + elapsed * 88 / wait_secs).clamp(1, 96),
                (wait_secs - elapsed).clamp(1, wait_secs),
            );
        }

        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(GenerationOutcome::Failure {
                        reason: "生成任务已中断，请重新生成。".to_string(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    })
                }
            }
        };

        let Some(outcome) = outcome else {
            poll_generation_stream(
                app_weak,
                store,
                receiver,
                raw_prompt,
                category,
                mode,
                ratio,
                quality,
                image_model,
                conversation_id,
                create_conversation,
                original_references,
                original_quote,
                task_id,
                started_at,
            );
            return;
        };

        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;

        match outcome {
            GenerationOutcome::ImageSuccess {
                bytes,
                optimized,
                time,
            } => match add_stream_success_item(
                &app,
                &store,
                &raw_prompt,
                &category,
                &mode,
                &quality,
                &image_model,
                &conversation_id,
                &optimized,
                &time,
                &bytes,
            ) {
                Ok(conversation_image) => {
                    state.set_asset_category_filter("all".into());
                    if create_conversation {
                        finish_conversation_placeholder(
                            &state,
                            &conversation_id,
                            Some(conversation_image),
                        );
                    }
                    if let Some(active) =
                        mark_active_generation_image_completed(&app, &category, &task_id, true)
                    {
                        if active.loading_count > 0 {
                            set_generation_status_for_category(&app, &category, "正在生成...");
                        }
                    }
                }
                Err(error) => {
                    let reason = zh_error(&error.to_string());
                    let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
                    add_stream_failure_item(
                        &app,
                        &store,
                        &raw_prompt,
                        &category,
                        &mode,
                        &ratio,
                        &quality,
                        &image_model,
                        &conversation_id,
                        &reason,
                        &time,
                    );
                    if let Some(active) =
                        mark_active_generation_image_completed(&app, &category, &task_id, false)
                    {
                        refund_credits(
                            &app,
                            active.credit_per_image,
                            "生图积分退回",
                            "失败图片自动退回",
                        );
                    }
                }
            },
            GenerationOutcome::ImageFailure { reason, time, .. } => {
                add_stream_failure_item(
                    &app,
                    &store,
                    &raw_prompt,
                    &category,
                    &mode,
                    &ratio,
                    &quality,
                    &image_model,
                    &conversation_id,
                    &reason,
                    &time,
                );
                if let Some(active) =
                    mark_active_generation_image_completed(&app, &category, &task_id, false)
                {
                    refund_credits(
                        &app,
                        active.credit_per_image,
                        "生图积分退回",
                        "失败图片自动退回",
                    );
                    if active.loading_count > 0 {
                        set_generation_status_for_category(&app, &category, "正在生成...");
                    }
                }
            }
            GenerationOutcome::Finished { .. } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let Some(task) = remove_active_generation(&category, &task_id) else {
                    return;
                };
                if create_conversation && task.success_count == 0 {
                    finish_conversation_placeholder(&state, &conversation_id, None);
                }
                if task.failed_count > 0 && task.success_count == 0 {
                    restore_stream_inputs(
                        &app,
                        &store,
                        &raw_prompt,
                        &category,
                        original_references.clone(),
                        original_quote.clone(),
                    );
                }
                set_stream_final_status(&app, &category, task.success_count, task.failed_count);
                sync_generation_state_for_current_category(&app);
            }
            GenerationOutcome::Failure { reason, time } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let Some(task) = remove_active_generation(&category, &task_id) else {
                    return;
                };
                let remaining = (task.total_count - task.completed_count).max(1);
                for _ in 0..remaining {
                    add_stream_failure_item(
                        &app,
                        &store,
                        &raw_prompt,
                        &category,
                        &mode,
                        &ratio,
                        &quality,
                        &image_model,
                        &conversation_id,
                        &reason,
                        &time,
                    );
                    refund_credits(
                        &app,
                        task.credit_per_image,
                        "生图积分退回",
                        "失败图片自动退回",
                    );
                }
                if create_conversation && task.success_count == 0 {
                    finish_conversation_placeholder(&state, &conversation_id, None);
                }
                if task.success_count == 0 {
                    restore_stream_inputs(
                        &app,
                        &store,
                        &raw_prompt,
                        &category,
                        original_references.clone(),
                        original_quote.clone(),
                    );
                }
                set_stream_final_status(
                    &app,
                    &category,
                    task.success_count,
                    task.failed_count + remaining,
                );
                sync_generation_state_for_current_category(&app);
            }
            GenerationOutcome::Success { .. } => {}
        }

        if keep_polling {
            poll_generation_stream(
                app_weak,
                store,
                receiver,
                raw_prompt,
                category,
                mode,
                ratio,
                quality,
                image_model,
                conversation_id,
                create_conversation,
                original_references,
                original_quote,
                task_id,
                started_at,
            );
        }
    });
}

#[allow(dead_code)]
fn poll_generation_result(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<GenerationOutcome>>>>,
    raw_prompt: String,
    category: String,
    mode: String,
    ratio: String,
    quality: String,
    image_model: String,
    conversation_id: String,
    create_conversation: bool,
    original_references: Vec<ReferenceData>,
    original_quote: QuoteContext,
    task_id: String,
    started_at: Instant,
) {
    slint::Timer::single_shot(Duration::from_millis(120), move || {
        if let Some(app) = app_weak.upgrade() {
            if !active_generation_matches(&category, &task_id) {
                return;
            }
            let elapsed = started_at.elapsed().as_secs() as i32;
            let wait_secs = IMAGE_GENERATION_WAIT_SECS as i32;
            update_active_generation_progress(
                &app,
                &category,
                &task_id,
                (8 + elapsed * 88 / wait_secs).clamp(1, 96),
                (wait_secs - elapsed).clamp(1, wait_secs),
            );
        }
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => {
                    slot.take();
                    Some(outcome)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(GenerationOutcome::Failure {
                        reason: "生成任务已中断，请重新生成。".to_string(),
                        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    })
                }
            }
        };

        let Some(outcome) = outcome else {
            poll_generation_result(
                app_weak,
                store,
                receiver,
                raw_prompt,
                category,
                mode,
                ratio,
                quality,
                image_model,
                conversation_id,
                create_conversation,
                original_references,
                original_quote,
                task_id,
                started_at,
            );
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let Some(active_task) = remove_active_generation(&category, &task_id) else {
            return;
        };
        let credit_cost = active_task.credit_cost;
        match outcome {
            GenerationOutcome::Success {
                images,
                failed_count,
                failure_reason,
                optimized,
                time,
            } => {
                let mut failed_count = failed_count;
                let mut created = Vec::new();
                let display_prompt = display_generation_prompt(&optimized);
                for bytes in images {
                    let Ok((bytes, image, width, height)) =
                        generated_image_from_bytes(&bytes, &quality)
                    else {
                        failed_count += 1;
                        continue;
                    };
                    let actual_ratio = ratio_from_actual_dimensions(width, height);
                    let source_path =
                        save_generated_bytes(&app, &bytes, &raw_prompt).unwrap_or_default();
                    created.push(AssetData {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: conversation_id.clone(),
                        title: short_text(&raw_prompt, 18),
                        category: category.clone(),
                        kind: mode.clone(),
                        time: time.clone(),
                        prompt: display_prompt.clone(),
                        ratio: actual_ratio,
                        quality: quality.clone(),
                        model: image_model.clone(),
                        width,
                        height,
                        image,
                        source_path,
                        cutout_done: false,
                        remove_black_done: false,
                        upscale_done: false,
                    });
                }
                let mut failed_items = Vec::new();
                for _ in 0..failed_count {
                    failed_items.push(AssetData {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: conversation_id.clone(),
                        title: short_text(&raw_prompt, 18),
                        category: category.clone(),
                        kind: mode.clone(),
                        time: time.clone(),
                        prompt: raw_prompt.clone(),
                        ratio: ratio.clone(),
                        quality: quality.clone(),
                        model: image_model.clone(),
                        width: 0,
                        height: 0,
                        image: Image::default(),
                        source_path: "failed".to_string(),
                        cutout_done: false,
                        remove_black_done: false,
                        upscale_done: false,
                    });
                }
                if created.is_empty() && failed_items.is_empty() {
                    let reason = "接口返回的图片无法读取。".to_string();
                    let mut store_mut = store.borrow_mut();
                    store_mut.generations.insert(
                        0,
                        AssetData {
                            id: Uuid::new_v4().to_string(),
                            conversation_id: conversation_id.clone(),
                            title: short_text(&raw_prompt, 18),
                            category: category.clone(),
                            kind: mode.clone(),
                            time: time.clone(),
                            prompt: reason.clone(),
                            ratio: ratio.clone(),
                            quality: quality.clone(),
                            model: image_model.clone(),
                            width: 0,
                            height: 0,
                            image: Image::default(),
                            source_path: "failed".to_string(),
                            cutout_done: false,
                            remove_black_done: false,
                            upscale_done: false,
                        },
                    );
                    if create_conversation {
                        finish_conversation_placeholder(&state, &conversation_id, None);
                    }
                    store_mut.notifications.insert(
                        0,
                        NotificationData {
                            id: Uuid::new_v4().to_string(),
                            title: format!("Generation failed: {}", short_text(&raw_prompt, 24)),
                            model: image_model,
                            time,
                            reason: reason.clone(),
                            success: false,
                            read: false,
                        },
                    );
                    set_generation_status_for_category(&app, &category, "生成失败");
                    sync_generation_state_for_current_category(&app);
                    if current_workspace_category(&app) == category {
                        state.set_prompt(raw_prompt.clone().into());
                        state.set_quote_title(original_quote.title.clone().into());
                        state.set_quote_prompt(original_quote.prompt.clone().into());
                        state.set_quote_ratio(original_quote.ratio.clone().into());
                        state.set_quote_quality(original_quote.quality.clone().into());
                        state.set_quote_width(original_quote.width);
                        state.set_quote_height(original_quote.height);
                    } else {
                        set_prompt_draft_for_category(
                            &mut store_mut.prompt_drafts,
                            &category,
                            raw_prompt.clone(),
                        );
                    }
                    *references_for_category_mut(&mut store_mut.references, &category) =
                        original_references.clone();
                    save_local_store(&app, &store_mut);
                    push_all(&app, &store_mut);
                    refund_credits(&app, credit_cost, "生图积分退回", "生成失败自动退回");
                    return;
                }
                let failed_total = failed_items.len();
                let conversation_image = created.first().map(|item| item.image.clone());
                let has_success = conversation_image.is_some();
                let mut store_mut = store.borrow_mut();
                for item in created.into_iter().rev() {
                    store_mut.assets.insert(0, item.clone());
                    store_mut.generations.insert(0, item);
                }
                for item in failed_items.into_iter().rev() {
                    store_mut.generations.insert(0, item);
                }
                if create_conversation {
                    finish_conversation_placeholder(&state, &conversation_id, conversation_image);
                }
                let notification_success = failed_total == 0;
                let notification_title = if notification_success {
                    format!("Generation succeeded: {}", short_text(&raw_prompt, 24))
                } else if has_success {
                    format!(
                        "Generation partially failed: {}",
                        short_text(&raw_prompt, 24)
                    )
                } else {
                    format!("Generation failed: {}", short_text(&raw_prompt, 24))
                };
                let notification_reason = if failed_total == 0 {
                    String::new()
                } else {
                    format!("{} image(s) failed: {}", failed_total, failure_reason)
                };
                store_mut.notifications.insert(
                    0,
                    NotificationData {
                        id: Uuid::new_v4().to_string(),
                        title: notification_title,
                        model: image_model,
                        time,
                        reason: notification_reason,
                        success: notification_success,
                        read: false,
                    },
                );
                set_generation_status_for_category(&app, &category, "生成成功");
                if failed_total == 0 {
                    set_generation_status_for_category(&app, &category, "生成成功");
                } else if has_success {
                    set_generation_status_for_category(&app, &category, "部分生成失败");
                } else {
                    set_generation_status_for_category(&app, &category, "生成失败");
                }
                sync_generation_state_for_current_category(&app);
                state.set_asset_category_filter("all".into());
                if failed_total > 0 && !has_success {
                    if current_workspace_category(&app) == category {
                        state.set_prompt(raw_prompt.clone().into());
                        state.set_quote_title(original_quote.title.clone().into());
                        state.set_quote_prompt(original_quote.prompt.clone().into());
                        state.set_quote_ratio(original_quote.ratio.clone().into());
                        state.set_quote_quality(original_quote.quality.clone().into());
                        state.set_quote_width(original_quote.width);
                        state.set_quote_height(original_quote.height);
                    } else {
                        set_prompt_draft_for_category(
                            &mut store_mut.prompt_drafts,
                            &category,
                            raw_prompt.clone(),
                        );
                    }
                    *references_for_category_mut(&mut store_mut.references, &category) =
                        original_references.clone();
                }
                save_local_store(&app, &store_mut);
                push_all(&app, &store_mut);
                if failed_total > 0 {
                    let refund = image_generation_credit_cost(&quality) * failed_total as i32;
                    refund_credits(&app, refund, "生图积分退回", "失败图片自动退回");
                }
            }
            GenerationOutcome::Failure { reason, time } => {
                let mut store_mut = store.borrow_mut();
                store_mut.generations.insert(
                    0,
                    AssetData {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: conversation_id.clone(),
                        title: short_text(&raw_prompt, 18),
                        category: category.clone(),
                        kind: mode.clone(),
                        time: time.clone(),
                        prompt: raw_prompt.clone(),
                        ratio: ratio.clone(),
                        quality: quality.clone(),
                        model: image_model.clone(),
                        width: 0,
                        height: 0,
                        image: Image::default(),
                        source_path: "failed".to_string(),
                        cutout_done: false,
                        remove_black_done: false,
                        upscale_done: false,
                    },
                );
                if create_conversation {
                    finish_conversation_placeholder(&state, &conversation_id, None);
                }
                store_mut.notifications.insert(
                    0,
                    NotificationData {
                        id: Uuid::new_v4().to_string(),
                        title: format!("Generation failed: {}", short_text(&raw_prompt, 24)),
                        model: image_model,
                        time,
                        reason: reason.clone(),
                        success: false,
                        read: false,
                    },
                );
                set_generation_status_for_category(&app, &category, "生成失败");
                sync_generation_state_for_current_category(&app);
                if current_workspace_category(&app) == category {
                    state.set_prompt(raw_prompt.clone().into());
                    state.set_quote_title(original_quote.title.clone().into());
                    state.set_quote_prompt(original_quote.prompt.clone().into());
                    state.set_quote_ratio(original_quote.ratio.clone().into());
                    state.set_quote_quality(original_quote.quality.clone().into());
                    state.set_quote_width(original_quote.width);
                    state.set_quote_height(original_quote.height);
                } else {
                    set_prompt_draft_for_category(
                        &mut store_mut.prompt_drafts,
                        &category,
                        raw_prompt.clone(),
                    );
                }
                *references_for_category_mut(&mut store_mut.references, &category) =
                    original_references;
                save_local_store(&app, &store_mut);
                push_all(&app, &store_mut);
                refund_credits(&app, credit_cost, "生图积分退回", "生成失败自动退回");
            }
            GenerationOutcome::ImageSuccess { .. }
            | GenerationOutcome::ImageFailure { .. }
            | GenerationOutcome::Finished { .. } => {}
        }
    });
}

fn optimize_prompt(
    provider: &ProviderData,
    model: &str,
    prompt: &str,
    category: &str,
    ratio: &str,
    quality: &str,
    quote: &QuoteContext,
    reference_images: &[String],
    translate_prompt: bool,
    output_language: PromptLanguage,
    visual_mode: bool,
) -> Result<String> {
    if provider.endpoint.trim().is_empty()
        || provider.api_key.trim().is_empty()
        || model.trim().is_empty()
    {
        return Ok(prompt.to_string());
    }
    let endpoint = normalize_chat_endpoint(&provider.endpoint);
    let user = optimization_user_prompt(
        prompt,
        category,
        ratio,
        quality,
        quote,
        reference_images.len(),
        translate_prompt,
        output_language,
        visual_mode,
    );
    let system = optimization_system_prompt(output_language, visual_mode);
    let user_message = if reference_images.is_empty() {
        json!({ "role": "user", "content": user })
    } else {
        let mut content = vec![json!({ "type": "text", "text": user })];
        for image in reference_images {
            content.push(json!({ "type": "image_url", "image_url": { "url": image } }));
        }
        json!({ "role": "user", "content": content })
    };
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            user_message
        ],
        "temperature": 0.4,
        "max_tokens": 800
    });
    let value = request_json("POST", &endpoint, &provider.api_key, Some(body))?;
    Ok(extract_text(&value).unwrap_or_else(|| prompt.to_string()))
}
fn translate_prompt_text(provider: &ProviderData, model: &str, prompt: &str) -> Result<String> {
    if provider.endpoint.trim().is_empty()
        || provider.api_key.trim().is_empty()
        || model.trim().is_empty()
    {
        return Ok(prompt.to_string());
    }
    let endpoint = normalize_chat_endpoint(&provider.endpoint);
    let body = json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are the translation assistant for ArtForgeStudio. Translate user text into natural English only. Do not optimize, expand, summarize, or add new details. Return only the translated English text."
            },
            {
                "role": "user",
                "content": format!("Translate this text into English. Return only the translation. Text: {}", prompt)
            }
        ],
        "temperature": 0.1,
        "max_tokens": 600
    });
    let value = request_json("POST", &endpoint, &provider.api_key, Some(body))?;
    let translated = extract_text(&value).unwrap_or_else(|| prompt.to_string());
    let translated = translated.trim();
    if translated.is_empty() {
        Ok(prompt.to_string())
    } else {
        Ok(translated.to_string())
    }
}

#[allow(dead_code, unreachable_code)]
fn generate_image_bytes_legacy(
    provider: &ProviderData,
    model: &str,
    prompt: &str,
    count: i32,
    ratio: &str,
    quality: &str,
    reference_images: &[String],
) -> Result<Vec<Vec<u8>>> {
    return Ok(generate_image_bytes_single_request(
        provider,
        model,
        prompt,
        count,
        ratio,
        quality,
        reference_images,
    )?
    .images);
}

#[allow(dead_code, unreachable_code)]
fn generate_image_bytes(
    provider: &ProviderData,
    model: &str,
    prompt: &str,
    count: i32,
    ratio: &str,
    quality: &str,
    reference_images: &[String],
) -> Result<Vec<Vec<u8>>> {
    return Ok(generate_image_bytes_single_request(
        provider,
        model,
        prompt,
        count,
        ratio,
        quality,
        reference_images,
    )?
    .images);
}

fn generate_image_bytes_single_request(
    provider: &ProviderData,
    model: &str,
    prompt: &str,
    count: i32,
    ratio: &str,
    quality: &str,
    reference_images: &[String],
) -> Result<ImageBatchResult> {
    let target_count = count.clamp(1, 4) as usize;
    let mut images = Vec::new();
    let mut failed_count = 0usize;
    let mut last_error: Option<String> = None;
    for request_count in image_request_batches_for_count(count) {
        match request_image_batch(
            provider,
            model,
            prompt,
            request_count,
            ratio,
            quality,
            reference_images,
        ) {
            Ok(batch) => {
                let remaining = target_count.saturating_sub(images.len());
                let requested = request_count.max(1) as usize;
                let before = images.len();
                images.extend(batch.images.into_iter().take(remaining.min(requested)));
                let accepted = images.len().saturating_sub(before);
                if accepted < requested {
                    failed_count += requested - accepted;
                    last_error.get_or_insert_with(|| {
                        "API did not return enough usable images".to_string()
                    });
                }
                if images.len() >= target_count {
                    break;
                }
            }
            Err(err) => {
                failed_count += request_count.max(1) as usize;
                last_error = Some(err.to_string());
            }
        }
    }
    Ok(ImageBatchResult {
        images,
        failed_count,
        last_error,
    })
}

fn image_request_batches_for_count(count: i32) -> Vec<i32> {
    let target_count = count.clamp(1, 4);
    vec![1; target_count as usize]
}

#[allow(dead_code, unreachable_code)]
fn generate_image_bytes_once(
    provider: &ProviderData,
    model: &str,
    prompt: &str,
    count: i32,
    ratio: &str,
    quality: &str,
    reference_images: &[String],
) -> Result<Vec<Vec<u8>>> {
    return Ok(generate_image_bytes_single_request(
        provider,
        model,
        prompt,
        count,
        ratio,
        quality,
        reference_images,
    )?
    .images);
}

fn request_image_batch(
    provider: &ProviderData,
    model: &str,
    prompt: &str,
    count: i32,
    ratio: &str,
    quality: &str,
    reference_images: &[String],
) -> Result<ImageRequestResult> {
    if provider.endpoint.trim().is_empty()
        || provider.api_key.trim().is_empty()
        || model.trim().is_empty()
    {
        return Err(anyhow!("Missing API endpoint, API key, or model name"));
    }
    let endpoint = normalize_image_endpoint(&provider.endpoint);
    let body = build_image_request_body(
        model,
        prompt,
        count,
        ratio,
        quality,
        reference_images,
        &endpoint,
    );
    match request_api("POST", &endpoint, &provider.api_key, Some(body))? {
        ApiResponse::Bytes(bytes) => Ok(ImageRequestResult {
            images: vec![bytes],
        }),
        ApiResponse::Json(value) => {
            collect_images_from_value(&value, &endpoint, &provider.api_key, count as usize)
        }
    }
}

fn build_image_request_body(
    model: &str,
    prompt: &str,
    count: i32,
    ratio: &str,
    quality: &str,
    reference_images: &[String],
    _endpoint: &str,
) -> Value {
    let quality = normalized_quality(quality);
    let (width, height) = pixel_dimensions_for(ratio, quality);
    let pixel_size = format!("{width}x{height}");
    let size = image_request_size_for_model(&pixel_size);
    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "n": count,
        "size": size,
        "pixel_size": pixel_size,
        "requested_size": pixel_size,
        "output_size": pixel_size,
        "width": width,
        "height": height,
        "aspect_ratio": ratio,
        "ratio": ratio,
        "quality": quality,
        "image_size": quality,
        "resolution": quality,
        "target_resolution": quality,
        "output_resolution": quality
    });
    if is_gpt_image_2_model(model) {
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "response_format".to_string(),
                Value::String("url".to_string()),
            );
        }
    }
    if !reference_images.is_empty() {
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "image".to_string(),
                Value::String(reference_images[0].clone()),
            );
            obj.insert(
                "images".to_string(),
                Value::Array(
                    reference_images
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            obj.insert(
                "reference_images".to_string(),
                Value::Array(
                    reference_images
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
    }
    body
}

fn image_request_size_for_model(pixel_size: &str) -> String {
    pixel_size.to_string()
}

fn is_gpt_image_2_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("gpt-image-2")
}

fn generate_images(
    provider: &ProviderData,
    model: &str,
    prompt: &str,
    count: i32,
    ratio: &str,
    quality: &str,
    reference_images: &[String],
) -> Result<Vec<Image>> {
    generate_image_bytes_single_request(
        provider,
        model,
        prompt,
        count,
        ratio,
        quality,
        reference_images,
    )?
    .images
    .into_iter()
    .map(|bytes| image_from_bytes(&bytes))
    .collect()
}

fn test_provider_connection(provider: &ProviderData) -> Result<()> {
    if provider.endpoint.trim().is_empty() || provider.api_key.trim().is_empty() {
        return Err(anyhow!("Missing API endpoint or API key"));
    }
    match list_models(&provider.endpoint, &provider.api_key) {
        Ok(_) => Ok(()),
        Err(list_err) => {
            let model = if provider.selected_model.trim().is_empty() {
                provider.models.first().cloned()
            } else {
                Some(provider.selected_model.trim().to_string())
            };
            let Some(model) = model else {
                return Err(list_err);
            };
            let body = json!({
                "model": model,
                "messages": [{ "role": "user", "content": "ping" }],
                "temperature": 0.1,
                "max_tokens": 8
            });
            request_json(
                "POST",
                &normalize_chat_endpoint(&provider.endpoint),
                &provider.api_key,
                Some(body),
            )
            .map(|_| ())
        }
    }
}

fn list_models(endpoint: &str, api_key: &str) -> Result<Vec<String>> {
    if endpoint.trim().is_empty() || api_key.trim().is_empty() {
        return Err(anyhow!("Missing API endpoint or API key"));
    }
    let value = request_json("GET", &normalize_models_endpoint(endpoint), api_key, None)?;
    let source = value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .or_else(|| value.get("models").and_then(Value::as_array))
        .ok_or_else(|| anyhow!("API did not return a model list"))?;
    let mut models = Vec::new();
    for item in source {
        let model = item
            .as_str()
            .or_else(|| item.get("id").and_then(Value::as_str))
            .or_else(|| item.get("name").and_then(Value::as_str))
            .or_else(|| item.get("model").and_then(Value::as_str))
            .unwrap_or("")
            .trim();
        if !model.is_empty() && !models.iter().any(|m| m == model) {
            models.push(model.to_string());
        }
    }
    Ok(models)
}

fn request_json(method: &str, endpoint: &str, api_key: &str, body: Option<Value>) -> Result<Value> {
    match request_api(method, endpoint, api_key, body)? {
        ApiResponse::Json(value) => Ok(value),
        ApiResponse::Bytes(_) => Err(anyhow!("API returned image bytes instead of JSON")),
    }
}

fn request_api(
    method: &str,
    endpoint: &str,
    api_key: &str,
    body: Option<Value>,
) -> Result<ApiResponse> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(IMAGE_REQUEST_TIMEOUT_SECS))
        .build()?;
    let mut request = match method {
        "GET" => client.get(endpoint),
        _ => client.post(endpoint),
    }
    .header("Accept", "application/json, image/*")
    .bearer_auth(api_key);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .context("请求超时，请检查网络环境或服务商接口状态后重试")?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = response.bytes()?.to_vec();
    if content_type.starts_with("image/") || looks_like_image_bytes(&bytes) {
        if bytes.is_empty() {
            return Err(anyhow!("API returned an empty image"));
        }
        return Ok(ApiResponse::Bytes(bytes));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .or_else(|| v.get("message").and_then(Value::as_str))
                    .or_else(|| v.get("error").and_then(Value::as_str))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        return Err(anyhow!(message));
    }
    if text.trim().is_empty() {
        Ok(ApiResponse::Json(json!({})))
    } else if let Ok(value) = serde_json::from_str(&text) {
        Ok(ApiResponse::Json(value))
    } else {
        Ok(ApiResponse::Json(json!({ "text": text })))
    }
}

fn extract_text(value: &Value) -> Option<String> {
    for pointer in [
        "/output_text",
        "/content",
        "/text",
        "/choices/0/message/content",
        "/choices/0/text",
    ] {
        if let Some(text) = value.pointer(pointer).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn extract_image(value: &Value) -> Option<String> {
    extract_images(value).into_iter().next()
}

fn extract_images(value: &Value) -> Vec<String> {
    let mut images = Vec::new();
    collect_image_strings(value, "", &mut images);
    images.dedup();
    images
}

fn extract_image_candidates(value: &Value, endpoint: &str) -> Vec<String> {
    let mut images = extract_images(value);
    collect_file_image_candidates(value, endpoint, &mut images);
    images.dedup();
    images
}

fn collect_file_image_candidates(value: &Value, endpoint: &str, images: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_file_image_candidates(item, endpoint, images);
            }
        }
        Value::Object(map) => {
            let maybe_id = map
                .get("file_id")
                .and_then(Value::as_str)
                .or_else(|| map.get("fileId").and_then(Value::as_str))
                .or_else(|| {
                    let object = map.get("object").and_then(Value::as_str).unwrap_or("");
                    if object.to_ascii_lowercase().contains("file") {
                        map.get("id").and_then(Value::as_str)
                    } else {
                        None
                    }
                });
            if let Some(file_id) = maybe_id {
                if let Some(url) = file_content_url(endpoint, file_id) {
                    images.push(url);
                }
            }
            for child in map.values() {
                collect_file_image_candidates(child, endpoint, images);
            }
        }
        _ => {}
    }
}

fn file_content_url(endpoint: &str, file_id: &str) -> Option<String> {
    let file_id = file_id.trim();
    if file_id.is_empty() {
        return None;
    }
    let (scheme, rest) = endpoint.split_once("://")?;
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() {
        return None;
    }
    let path = rest
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or("")
        .trim_end_matches('/');
    let base_path = path
        .find("/images/")
        .map(|index| &path[..index])
        .or_else(|| path.find("/chat/").map(|index| &path[..index]))
        .or_else(|| path.find("/responses").map(|index| &path[..index]))
        .unwrap_or(path);
    let prefix = if base_path.is_empty() {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}/{}", base_path.trim_matches('/'))
    };
    Some(format!("{prefix}/files/{file_id}/content"))
}

fn collect_images_from_value(
    value: &Value,
    endpoint: &str,
    api_key: &str,
    limit: usize,
) -> Result<ImageRequestResult> {
    if is_pending_task_status(value) {
        if let Some(poll_url) = extract_poll_url(value, endpoint) {
            return poll_image_result(&poll_url, endpoint, api_key, limit);
        }
    }
    let raw_images = extract_image_candidates(value, endpoint);
    if !raw_images.is_empty() {
        let mut images = Vec::new();
        let mut last_error: Option<String> = None;
        for raw in raw_images {
            if images.len() >= limit {
                break;
            }
            match image_bytes_from_response(&raw, endpoint, api_key) {
                Ok(bytes) => images.push(bytes),
                Err(err) => last_error = Some(err.to_string()),
            }
        }
        if !images.is_empty() {
            return Ok(ImageRequestResult { images });
        }
        if let Some(poll_url) = extract_poll_url(value, endpoint) {
            return poll_image_result(&poll_url, endpoint, api_key, limit);
        }
        return Err(anyhow!(
            "{}",
            last_error.unwrap_or_else(|| "API did not return usable images".to_string())
        ));
    }
    if let Some(poll_url) = extract_poll_url(value, endpoint) {
        return poll_image_result(&poll_url, endpoint, api_key, limit);
    }
    Err(anyhow!("API did not return images"))
}

fn collect_image_strings(value: &Value, key: &str, images: &mut Vec<String>) {
    match value {
        Value::String(raw) => {
            for image in normalize_image_strings(key, raw) {
                images.push(image);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_image_strings(item, key, images);
            }
        }
        Value::Object(map) => {
            for (child_key, child_value) in map {
                collect_image_strings(child_value, child_key, images);
            }
        }
        _ => {}
    }
}

fn normalize_image_strings(key: &str, raw: &str) -> Vec<String> {
    let text = raw.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let key = key.to_ascii_lowercase();
    if key.contains("poll") || key.contains("status") || key.contains("task") {
        return Vec::new();
    }
    if text.starts_with("data:image") {
        return vec![text.to_string()];
    }
    let image_key = is_image_result_key(&key);
    if image_key && looks_like_base64_image(text) {
        return vec![format!("data:image/png;base64,{text}")];
    }
    let mut images = Vec::new();
    for part in text.split(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | '"' | '\'' | '[' | ']' | '{' | '}')
    }) {
        let candidate = part.trim_matches(|ch: char| matches!(ch, '<' | '>' | '(' | ')' | ';'));
        if candidate.is_empty() {
            continue;
        }
        if is_url_like(candidate) && (image_key || looks_like_image_url(candidate)) {
            images.push(candidate.to_string());
        }
    }
    images
}

fn is_image_result_key(key: &str) -> bool {
    [
        "image",
        "images",
        "image_url",
        "image_urls",
        "imageurl",
        "imageurls",
        "url",
        "urls",
        "b64_json",
        "base64",
        "base64_json",
        "data",
        "file",
        "path",
        "output",
        "outputs",
        "output_url",
        "output_urls",
        "outputurl",
        "outputurls",
        "result",
        "results",
        "result_url",
        "result_urls",
        "resulturl",
        "resulturls",
        "download_url",
        "download_urls",
        "downloadurl",
        "downloadurls",
        "file_url",
        "fileurl",
        "public_url",
        "publicurl",
        "signed_url",
        "signedurl",
    ]
    .iter()
    .any(|name| key == *name || key.ends_with(name))
}

fn is_url_like(text: &str) -> bool {
    text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with('/')
        || text.starts_with("./")
        || text.starts_with("../")
}

fn looks_like_image_url(text: &str) -> bool {
    let lower = text
        .split(['?', '#'])
        .next()
        .unwrap_or(text)
        .to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
        || lower.ends_with(".bmp")
}

fn looks_like_base64_image(text: &str) -> bool {
    if text.len() < 80 || text.contains(' ') || text.contains('\n') {
        return false;
    }
    text.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'))
}

fn extract_poll_url(value: &Value, endpoint: &str) -> Option<String> {
    for pointer in [
        "/poll_url",
        "/status_url",
        "/result_url",
        "/url",
        "/data/poll_url",
        "/data/status_url",
        "/data/result_url",
        "/data/url",
    ] {
        if let Some(url) = value.pointer(pointer).and_then(Value::as_str) {
            let url = absolutize_url(endpoint, url);
            if !url.is_empty() {
                return Some(url);
            }
        }
    }
    let task_id = value
        .get("task_id")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/data/task_id").and_then(Value::as_str))
        .or_else(|| {
            if is_pending_task_status(value) {
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| value.pointer("/data/id").and_then(Value::as_str))
            } else {
                None
            }
        })?;
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.is_empty() || task_id.trim().is_empty() {
        None
    } else {
        Some(format!("{endpoint}/{}", task_id.trim()))
    }
}

fn poll_image_result(
    poll_url: &str,
    endpoint: &str,
    api_key: &str,
    limit: usize,
) -> Result<ImageRequestResult> {
    for _ in 0..IMAGE_POLL_ATTEMPTS {
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        match request_api("GET", poll_url, api_key, None)? {
            ApiResponse::Bytes(bytes) => {
                return Ok(ImageRequestResult {
                    images: vec![bytes],
                })
            }
            ApiResponse::Json(value) => {
                let raw_images = extract_image_candidates(&value, endpoint);
                if !raw_images.is_empty() {
                    let mut images = Vec::new();
                    let mut last_error: Option<String> = None;
                    for raw in raw_images {
                        if images.len() >= limit {
                            break;
                        }
                        match image_bytes_from_response(&raw, endpoint, api_key) {
                            Ok(bytes) => images.push(bytes),
                            Err(err) => last_error = Some(err.to_string()),
                        }
                    }
                    if !images.is_empty() {
                        return Ok(ImageRequestResult { images });
                    }
                    if let Some(err) = last_error {
                        return Err(anyhow!(err));
                    }
                }
                if is_failed_task_status(&value) {
                    return Err(anyhow!("后端任务生成失败"));
                }
            }
        }
    }
    Err(anyhow!("后端图片生成完成较慢，客户端轮询超时"))
}

fn is_failed_task_status(value: &Value) -> bool {
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/data/status").and_then(Value::as_str))
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        status.as_str(),
        "failed" | "error" | "cancelled" | "canceled" | "timeout"
    )
}

fn is_pending_task_status(value: &Value) -> bool {
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/data/status").and_then(Value::as_str))
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        status.as_str(),
        "pending" | "queued" | "running" | "processing" | "in_progress" | "submitted"
    )
}

fn image_from_response(raw: &str) -> Result<Image> {
    image_from_bytes(&image_bytes_from_response(raw, "", "")?)
}

fn image_bytes_from_response(raw: &str, endpoint: &str, api_key: &str) -> Result<Vec<u8>> {
    image_bytes_from_response_with_depth(raw, endpoint, api_key, 0)
}

fn image_bytes_from_response_with_depth(
    raw: &str,
    endpoint: &str,
    api_key: &str,
    depth: usize,
) -> Result<Vec<u8>> {
    if raw.starts_with("data:image") {
        let (_, data) = raw
            .split_once(',')
            .ok_or_else(|| anyhow!("Invalid image data format"))?;
        return Ok(base64::engine::general_purpose::STANDARD.decode(data)?);
    }
    if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with('/') {
        return download_image_bytes_with_depth(&absolutize_url(endpoint, raw), api_key, depth);
    }
    Ok(base64::engine::general_purpose::STANDARD.decode(raw)?)
}

fn download_image_bytes(url: &str, api_key: &str) -> Result<Vec<u8>> {
    download_image_bytes_with_depth(url, api_key, 0)
}

fn download_image_bytes_with_depth(url: &str, api_key: &str, depth: usize) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(IMAGE_REQUEST_TIMEOUT_SECS))
        .build()?;
    if api_key.trim().is_empty() {
        return download_image_bytes_with_auth_depth(&client, url, None, depth);
    }
    match download_image_bytes_with_auth_depth(&client, url, Some(api_key), depth) {
        Ok(bytes) => Ok(bytes),
        Err(first_err) => {
            download_image_bytes_with_auth_depth(&client, url, None, depth).map_err(|retry_err| {
                anyhow!(
                    "Image download failed with auth: {}; retry without auth failed: {}",
                    first_err,
                    retry_err
                )
            })
        }
    }
}

fn download_image_bytes_with_auth(
    client: &reqwest::blocking::Client,
    url: &str,
    api_key: Option<&str>,
) -> Result<Vec<u8>> {
    download_image_bytes_with_auth_depth(client, url, api_key, 0)
}

fn download_image_bytes_with_auth_depth(
    client: &reqwest::blocking::Client,
    url: &str,
    api_key: Option<&str>,
    depth: usize,
) -> Result<Vec<u8>> {
    let mut request = client.get(url).header(
        "Accept",
        "image/*, application/octet-stream, application/json",
    );
    if let Some(api_key) = api_key {
        request = request.bearer_auth(api_key);
    }
    let response = request.send()?;
    if !response.status().is_success() {
        return Err(anyhow!("Image download failed: {}", response.status()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = response.bytes()?.to_vec();
    if bytes.is_empty() {
        return Err(anyhow!("Downloaded image is empty"));
    }
    if content_type.starts_with("image/") || looks_like_image_bytes(&bytes) {
        return Ok(bytes);
    }
    if depth < 3 {
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            let raw_images = extract_image_candidates(&value, url);
            for raw in raw_images {
                match image_bytes_from_response_with_depth(
                    &raw,
                    url,
                    api_key.unwrap_or(""),
                    depth + 1,
                ) {
                    Ok(bytes) => return Ok(bytes),
                    Err(_) => {}
                }
            }
            return Err(anyhow!("Image download returned JSON without usable image"));
        }
    }
    Err(anyhow!(
        "Image download did not return image data{}",
        if content_type.is_empty() {
            String::new()
        } else {
            format!(" ({content_type})")
        }
    ))
}

fn looks_like_image_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xff, 0xd8, 0xff])
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || bytes.starts_with(b"BM")
}

fn absolutize_url(base: &str, raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with("data:image") {
        return raw.to_string();
    }
    if raw.is_empty() {
        return String::new();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return raw.to_string();
    };
    let host = rest.split('/').next().unwrap_or("");
    if raw.starts_with('/') {
        return format!("{scheme}://{host}{raw}");
    }
    let base_dir = base.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(base);
    format!("{}/{}", base_dir.trim_end_matches('/'), raw)
}

fn image_from_bytes(bytes: &[u8]) -> Result<Image> {
    Ok(image_from_bytes_with_dimensions(bytes)?.0)
}

fn image_from_bytes_with_dimensions(bytes: &[u8]) -> Result<(Image, i32, i32)> {
    let img = image::load_from_memory(bytes)?.to_rgba8();
    let (w, h) = img.dimensions();
    let buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Ok((Image::from_rgba8(buffer), w as i32, h as i32))
}

fn generated_image_from_bytes(bytes: &[u8], quality: &str) -> Result<(Vec<u8>, Image, i32, i32)> {
    let mut img = image::load_from_memory(bytes)?.to_rgba8();
    let (mut width, mut height) = img.dimensions();
    let max_edge = max_edge_for_quality(quality) as u32;
    let mut output_bytes = bytes.to_vec();
    if width.max(height) > max_edge {
        let (target_width, target_height) = fit_dimensions_to_max_edge(width, height, max_edge);
        img = image::imageops::resize(
            &img,
            target_width,
            target_height,
            image::imageops::FilterType::Lanczos3,
        );
        width = target_width;
        height = target_height;
        output_bytes = encode_png_rgba(&img, width, height)?;
    }
    let image = slint_image_from_rgba(&img, width, height);
    Ok((output_bytes, image, width as i32, height as i32))
}

fn encode_png_rgba(rgba: &image::RgbaImage, width: u32, height: u32) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut bytes));
    image::ImageEncoder::write_image(
        encoder,
        rgba.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}

fn slint_image_from_rgba(rgba: &image::RgbaImage, width: u32, height: u32) -> Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        width,
        height,
    );
    Image::from_rgba8(buffer)
}

fn max_edge_for_quality(quality: &str) -> i32 {
    match normalized_quality(quality) {
        "4K" => 4096,
        "2K" => 2048,
        _ => 1024,
    }
}

fn fit_dimensions_to_max_edge(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_edge.max(1), max_edge.max(1));
    }
    if width >= height {
        let target_height =
            ((height as f64 * max_edge as f64 / width as f64).round() as u32).clamp(1, max_edge);
        (max_edge, target_height)
    } else {
        let target_width =
            ((width as f64 * max_edge as f64 / height as f64).round() as u32).clamp(1, max_edge);
        (target_width, max_edge)
    }
}

fn image_from_clipboard(img: arboard::ImageData<'_>) -> Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        img.bytes.as_ref(),
        img.width as u32,
        img.height as u32,
    );
    Image::from_rgba8(buffer)
}

fn image_to_data_url(image: &Image) -> Result<String> {
    let buffer = image
        .to_rgba8()
        .ok_or_else(|| anyhow!("参考图数据不可读取"))?;
    let mut bytes = Vec::new();
    {
        let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut bytes));
        image::ImageEncoder::write_image(
            encoder,
            buffer.as_bytes(),
            buffer.width(),
            buffer.height(),
            image::ExtendedColorType::Rgba8,
        )?;
    }
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn load_image(path: &Path) -> Result<Image> {
    Image::load_from_path(path).map_err(|_| anyhow!("无法读取图片"))
}

fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn init_version_state(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_current_version(env!("CARGO_PKG_VERSION").into());
    refresh_update_state(app);
}

fn refresh_update_state(app: &AppWindow) -> bool {
    let state = app.global::<AppState>();
    let current = env!("CARGO_PKG_VERSION");
    let latest = read_update_manifest_version().unwrap_or_else(|| current.to_string());
    let available = compare_versions(&latest, current).is_gt();
    state.set_latest_version(latest.clone().into());
    state.set_update_available(available);
    if available {
        state.set_update_message(format!("发现新版本 {latest}").into());
    } else if state.get_update_message().is_empty() {
        state.set_update_message("当前已经是最新版本".into());
    }
    available
}

fn read_update_manifest_version() -> Option<String> {
    for base in resource_base_dirs() {
        for path in [
            base.join("update-manifest.json"),
            base.join("data").join("update-manifest.json"),
        ] {
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<UpdateManifest>(&text) else {
                continue;
            };
            let version = manifest.version.trim();
            if !version.is_empty() {
                return Some(version.to_string());
            }
        }
    }
    None
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left_parts = version_parts(left);
    let right_parts = version_parts(right);
    let len = left_parts.len().max(right_parts.len());
    for index in 0..len {
        let left_value = *left_parts.get(index).unwrap_or(&0);
        let right_value = *right_parts.get(index).unwrap_or(&0);
        match left_value.cmp(&right_value) {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    std::cmp::Ordering::Equal
}

fn version_parts(version: &str) -> Vec<i32> {
    version
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<i32>().unwrap_or(0))
        .collect()
}

fn advance_update_progress(app_weak: Weak<AppWindow>) {
    slint::Timer::single_shot(Duration::from_millis(180), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if !state.get_update_progress_open() || state.get_update_ready() {
            return;
        }
        let next = (state.get_update_progress() + 8).min(100);
        state.set_update_progress(next);
        if next >= 100 {
            state.set_update_ready(true);
            return;
        }
        advance_update_progress(app.as_weak());
    });
}

fn relaunch_current_exe() -> Result<()> {
    let exe = std::env::current_exe().context("无法获取当前客户端路径")?;
    Command::new(exe).spawn().context("无法重启客户端")?;
    Ok(())
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn macos_resources_dir() -> Option<PathBuf> {
    let exe_dir = app_dir();
    let contents_dir = exe_dir.parent()?;
    if exe_dir.file_name().and_then(|value| value.to_str()) == Some("MacOS")
        && contents_dir.file_name().and_then(|value| value.to_str()) == Some("Contents")
    {
        Some(contents_dir.join("Resources"))
    } else {
        None
    }
}

fn resource_base_dirs() -> Vec<PathBuf> {
    let exe_dir = app_dir();
    let mut bases = Vec::new();
    push_unique_path(&mut bases, exe_dir.clone());
    if let Some(resources_dir) = macos_resources_dir() {
        push_unique_path(&mut bases, resources_dir);
    }
    if let Some(parent) = exe_dir.parent() {
        push_unique_path(&mut bases, parent.to_path_buf());
    }
    if let Ok(current_dir) = std::env::current_dir() {
        push_unique_path(&mut bases, current_dir.clone());
        if let Some(parent) = current_dir.parent() {
            push_unique_path(&mut bases, parent.join("local-preview").join("static"));
        }
    }
    #[cfg(windows)]
    {
        push_unique_path(
            &mut bases,
            PathBuf::from(r"C:\Users\deyx1\Documents\ArtForgeStudio"),
        );
    }
    bases
}

fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("ArtForgeStudio")
                .join("data");
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(resources_dir) = macos_resources_dir() {
            return resources_dir.join("data");
        }
    }

    macos_resources_dir().unwrap_or_else(app_dir).join("data")
}

fn init_portable_dirs(app: &AppWindow) -> Result<()> {
    let data_dir = app_data_dir();
    let input_dir = data_dir.join("input");
    let output_dir = data_dir.join("out");
    let prompt_dir = data_dir.join("prompt");

    fs::create_dir_all(&input_dir)?;
    fs::create_dir_all(&output_dir)?;
    fs::create_dir_all(&prompt_dir)?;

    let state = app.global::<AppState>();
    state.set_input_dir(input_dir.display().to_string().into());
    state.set_output_dir(output_dir.display().to_string().into());
    state.set_prompt_dir(prompt_dir.display().to_string().into());
    Ok(())
}

fn output_dir_path(app: &AppWindow) -> PathBuf {
    let value = app.global::<AppState>().get_output_dir().to_string();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return app_data_dir().join("out");
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        path
    } else {
        app_dir().join(path)
    }
}

fn save_generated_bytes(app: &AppWindow, bytes: &[u8], prompt: &str) -> Result<String> {
    let dir = output_dir_path(app);
    fs::create_dir_all(&dir)?;
    let stem = sanitize_filename(&short_text(prompt, 18));
    let ext = image_extension(bytes);
    let path = unique_path(dir.join(format!(
        "{}-{}.{}",
        Local::now().format("%Y%m%d%H%M%S%3f"),
        stem,
        ext
    )));
    fs::write(&path, bytes)?;
    Ok(path.display().to_string())
}

fn image_extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "webp"
    } else {
        "png"
    }
}

fn sanitize_filename(value: &str) -> String {
    let text = value
        .chars()
        .map(|ch| {
            if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control()
            {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    let trimmed = text.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "image".to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
    for index in 1..1000 {
        let name = if ext.is_empty() {
            format!("{stem}-{index}")
        } else {
            format!("{stem}-{index}.{ext}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

fn viewer_item<'a>(store: &'a Store, id: &str, source: &str) -> Option<&'a AssetData> {
    match source {
        "asset" => store.assets.iter().find(|item| item.id == id),
        "inspiration" => store.inspiration.iter().find(|item| item.id == id),
        _ => store.generations.iter().find(|item| item.id == id),
    }
}

fn copy_viewer_image(app: &AppWindow) {
    let state = app.global::<AppState>();
    let image = state.get_viewer_image();
    let Some(buffer) = image.to_rgba8() else {
        state.set_viewer_message("图片数据不可复制".into());
        return;
    };
    let data = arboard::ImageData {
        width: buffer.width() as usize,
        height: buffer.height() as usize,
        bytes: Cow::Owned(buffer.as_bytes().to_vec()),
    };
    match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_image(data)) {
        Ok(_) => state.set_viewer_message("已复制图片".into()),
        Err(error) => state.set_viewer_message(format!("复制失败：{error}").into()),
    }
}

fn reveal_path_in_file_manager(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("图片文件不存在"));
    }
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("explorer");
        if target.is_file() {
            command.arg("/select,").arg(&target);
        } else {
            command.arg(&target);
        }
        command
            .spawn()
            .with_context(|| format!("无法打开文件夹：{}", target.display()))?;
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        if target.is_file() {
            command.arg("-R").arg(&target);
        } else {
            command.arg(&target);
        }
        command
            .spawn()
            .with_context(|| format!("无法打开文件夹：{}", target.display()))?;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let folder = if target.is_file() {
            target.parent().unwrap_or(&target)
        } else {
            target.as_path()
        };
        Command::new("xdg-open")
            .arg(folder)
            .spawn()
            .with_context(|| format!("无法打开文件夹：{}", folder.display()))?;
    }

    Ok(())
}

fn download_viewer_image(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let id = state.get_viewer_id().to_string();
    let source = state.get_viewer_source().to_string();
    let item = viewer_item(store, &id, &source);
    let Some(item) = item else {
        state.set_viewer_message("没有可打开位置的原始文件".into());
        return;
    };
    let source_path = item.source_path.trim();
    if source_path.is_empty() || source_path == "failed" || source_path == "asset" {
        state.set_viewer_message("没有可打开位置的原始文件".into());
        return;
    }
    let source = PathBuf::from(source_path);
    match reveal_path_in_file_manager(&source) {
        Ok(_) => state.set_viewer_message("已打开图片所在文件夹".into()),
        Err(error) => state.set_viewer_message(format!("打开文件夹失败：{error}").into()),
    }
}

fn download_asset(app: &AppWindow, store: &Rc<RefCell<Store>>, id: String) {
    let item = {
        let store_ref = store.borrow();
        store_ref
            .generations
            .iter()
            .chain(store_ref.assets.iter())
            .chain(store_ref.inspiration.iter())
            .find(|item| item.id == id)
            .cloned()
    };
    let state = app.global::<AppState>();
    let Some(item) = item else {
        state.set_generation_status("未找到图片".into());
        return;
    };
    let source_path = item.source_path.trim();
    if source_path.is_empty() || source_path == "failed" || source_path == "asset" {
        state.set_generation_status("没有可打开位置的原始文件".into());
        return;
    }
    let source = PathBuf::from(source_path);
    match reveal_path_in_file_manager(&source) {
        Ok(_) => state.set_generation_status("已打开图片所在文件夹".into()),
        Err(error) => state.set_generation_status(format!("打开文件夹失败：{error}").into()),
    }
}

#[derive(Clone, Copy)]
enum ProcessImageMode {
    Cutout,
    RemoveBlack,
    Upscale { scale: u32, target_long_edge: u32 },
}

fn start_viewer_image_processing(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    mode: ProcessImageMode,
) {
    let state = app.global::<AppState>();
    if state.get_viewer_processing() {
        return;
    }
    let already_done = match mode {
        ProcessImageMode::Cutout => state.get_viewer_cutout_done(),
        ProcessImageMode::RemoveBlack => state.get_viewer_remove_black_done(),
        ProcessImageMode::Upscale { .. } => state.get_viewer_upscale_done(),
    };
    if already_done {
        state.set_viewer_message(processing_done_message(app, mode).into());
        return;
    }
    state.set_viewer_processing(true);
    state.set_viewer_processing_progress(0);
    state.set_viewer_processing_label(processing_label(app, mode).into());
    let duration_ms = viewer_processing_duration_ms(mode);
    for (delay_percent, progress) in [
        (10u64, 12),
        (24, 28),
        (40, 46),
        (60, 64),
        (78, 82),
        (94, 94),
    ] {
        let delay = duration_ms.saturating_mul(delay_percent) / 100;
        schedule_viewer_processing_progress(app.as_weak(), delay, progress);
    }
    let app_weak = app.as_weak();
    slint::Timer::single_shot(Duration::from_millis(duration_ms), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if process_viewer_image(&app, &store, mode) {
            let state = app.global::<AppState>();
            state.set_viewer_processing_progress(100);
            let app_weak = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(180), move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let state = app.global::<AppState>();
                state.set_viewer_processing(false);
                state.set_upscale_open(false);
                state.set_viewer_open(false);
                navigate_to(&app, "generation");
            });
        } else {
            let state = app.global::<AppState>();
            state.set_viewer_processing(false);
        }
    });
}

fn viewer_processing_duration_ms(mode: ProcessImageMode) -> u64 {
    match mode {
        ProcessImageMode::Cutout | ProcessImageMode::RemoveBlack => 3000,
        ProcessImageMode::Upscale { .. } => 560,
    }
}

fn schedule_viewer_processing_progress(app: Weak<AppWindow>, delay_ms: u64, progress: i32) {
    slint::Timer::single_shot(Duration::from_millis(delay_ms), move || {
        let Some(app) = app.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if state.get_viewer_processing() && state.get_viewer_processing_progress() < progress {
            state.set_viewer_processing_progress(progress);
        }
    });
}

fn processing_label(app: &AppWindow, mode: ProcessImageMode) -> &'static str {
    let en = app.global::<AppState>().get_language().as_str() == "en";
    match mode {
        ProcessImageMode::Cutout => {
            if en {
                "Cutting out"
            } else {
                "正在抠图"
            }
        }
        ProcessImageMode::RemoveBlack => {
            if en {
                "Removing black"
            } else {
                "正在去黑"
            }
        }
        ProcessImageMode::Upscale { .. } => {
            if en {
                "Upscaling"
            } else {
                "正在放大"
            }
        }
    }
}

fn processing_done_message(app: &AppWindow, mode: ProcessImageMode) -> &'static str {
    let en = app.global::<AppState>().get_language().as_str() == "en";
    match mode {
        ProcessImageMode::Cutout => {
            if en {
                "This image has already been cut out."
            } else {
                "当前图片已抠图"
            }
        }
        ProcessImageMode::RemoveBlack => {
            if en {
                "Black has already been removed from this image."
            } else {
                "当前图片已去黑"
            }
        }
        ProcessImageMode::Upscale { .. } => {
            if en {
                "This image has already been upscaled."
            } else {
                "当前图片已清晰放大"
            }
        }
    }
}

fn upscale_quality_long_edge(quality: &str) -> u32 {
    if quality.eq_ignore_ascii_case("4K") {
        4096
    } else {
        2048
    }
}

fn process_viewer_image(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mode: ProcessImageMode,
) -> bool {
    let state = app.global::<AppState>();
    let Some(buffer) = state.get_viewer_image().to_rgba8() else {
        state.set_viewer_message("图片数据不可处理".into());
        return false;
    };
    let mut width = buffer.width();
    let mut height = buffer.height();
    if width == 0 || height == 0 {
        state.set_viewer_message("图片数据不可处理".into());
        return false;
    }
    let mut rgba = buffer.as_bytes().to_vec();
    match mode {
        ProcessImageMode::Cutout => cutout_edge_background(&mut rgba, width, height),
        ProcessImageMode::RemoveBlack => remove_black_pixels(&mut rgba),
        ProcessImageMode::Upscale {
            scale,
            target_long_edge,
        } => {
            let Some(source) = image::RgbaImage::from_raw(width, height, rgba) else {
                state.set_viewer_message("图片数据不可处理".into());
                return false;
            };
            let (target_width, target_height) =
                upscale_dimensions(width, height, scale, target_long_edge);
            let resized = image::imageops::resize(
                &source,
                target_width,
                target_height,
                image::imageops::FilterType::Lanczos3,
            );
            width = target_width;
            height = target_height;
            rgba = resized.into_raw();
        }
    }
    if let Err(error) = save_processed_viewer_image(app, store, rgba, width, height, mode) {
        state.set_viewer_message(format!("处理失败：{error}").into());
        return false;
    }
    true
}

fn upscale_dimensions(width: u32, height: u32, scale: u32, target_long_edge: u32) -> (u32, u32) {
    let scale = scale.clamp(2, 4) as u64;
    let scaled_width = (width as u64).saturating_mul(scale);
    let scaled_height = (height as u64).saturating_mul(scale);
    let scaled_long = scaled_width.max(scaled_height).max(1);
    let original_long = width.max(height) as u64;
    let target_long = (target_long_edge as u64).max(original_long).min(8192);
    if scaled_long <= target_long {
        return (
            scaled_width.min(8192).max(1) as u32,
            scaled_height.min(8192).max(1) as u32,
        );
    }
    let target_width = (scaled_width.saturating_mul(target_long) / scaled_long).max(1);
    let target_height = (scaled_height.saturating_mul(target_long) / scaled_long).max(1);
    (target_width as u32, target_height as u32)
}

fn cutout_edge_background(rgba: &mut [u8], width: u32, height: u32) {
    let corners = [
        (0, 0),
        (width.saturating_sub(1), 0),
        (0, height.saturating_sub(1)),
        (width.saturating_sub(1), height.saturating_sub(1)),
    ];
    let mut bg = [0u32; 3];
    for &(x, y) in &corners {
        let idx = pixel_index(width, x, y);
        bg[0] += rgba[idx] as u32;
        bg[1] += rgba[idx + 1] as u32;
        bg[2] += rgba[idx + 2] as u32;
    }
    let bg = [
        (bg[0] / corners.len() as u32) as u8,
        (bg[1] / corners.len() as u32) as u8,
        (bg[2] / corners.len() as u32) as u8,
    ];
    let mut visited = vec![false; (width as usize).saturating_mul(height as usize)];
    let mut queue = Vec::new();
    for x in 0..width {
        enqueue_background_pixel(rgba, width, height, x, 0, bg, &mut visited, &mut queue);
        enqueue_background_pixel(
            rgba,
            width,
            height,
            x,
            height.saturating_sub(1),
            bg,
            &mut visited,
            &mut queue,
        );
    }
    for y in 0..height {
        enqueue_background_pixel(rgba, width, height, 0, y, bg, &mut visited, &mut queue);
        enqueue_background_pixel(
            rgba,
            width,
            height,
            width.saturating_sub(1),
            y,
            bg,
            &mut visited,
            &mut queue,
        );
    }

    let mut cursor = 0usize;
    while cursor < queue.len() {
        let (x, y) = queue[cursor];
        cursor += 1;
        if x > 0 {
            enqueue_background_pixel(rgba, width, height, x - 1, y, bg, &mut visited, &mut queue);
        }
        if x + 1 < width {
            enqueue_background_pixel(rgba, width, height, x + 1, y, bg, &mut visited, &mut queue);
        }
        if y > 0 {
            enqueue_background_pixel(rgba, width, height, x, y - 1, bg, &mut visited, &mut queue);
        }
        if y + 1 < height {
            enqueue_background_pixel(rgba, width, height, x, y + 1, bg, &mut visited, &mut queue);
        }
    }

    for y in 0..height {
        for x in 0..width {
            let flat = (y * width + x) as usize;
            if visited[flat] {
                rgba[pixel_index(width, x, y) + 3] = 0;
            }
        }
    }
}

fn enqueue_background_pixel(
    rgba: &[u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    bg: [u8; 3],
    visited: &mut [bool],
    queue: &mut Vec<(u32, u32)>,
) {
    if x >= width || y >= height {
        return;
    }
    let flat = (y * width + x) as usize;
    if visited[flat] {
        return;
    }
    let idx = pixel_index(width, x, y);
    if color_distance_sq([rgba[idx], rgba[idx + 1], rgba[idx + 2]], bg) > 55 * 55 {
        return;
    }
    visited[flat] = true;
    queue.push((x, y));
}

fn remove_black_pixels(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let luma =
            (54u32 * pixel[0] as u32 + 183u32 * pixel[1] as u32 + 19u32 * pixel[2] as u32) / 256;
        if luma <= 34 {
            pixel[3] = 0;
        } else if luma < 84 {
            let scale = (luma - 34) as f32 / 50.0;
            pixel[3] = (pixel[3] as f32 * scale) as u8;
        }
    }
}

fn pixel_index(width: u32, x: u32, y: u32) -> usize {
    ((y * width + x) * 4) as usize
}

fn color_distance_sq(a: [u8; 3], b: [u8; 3]) -> i32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    dr * dr + dg * dg + db * db
}

fn save_processed_viewer_image(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    mode: ProcessImageMode,
) -> Result<()> {
    let state = app.global::<AppState>();
    let (suffix, title_suffix) = match mode {
        ProcessImageMode::Cutout => ("cutout", "抠图"),
        ProcessImageMode::RemoveBlack => ("remove-black", "去黑"),
        ProcessImageMode::Upscale { scale, .. } => {
            if scale >= 4 {
                ("upscale-4x", "清晰放大4X")
            } else if scale == 3 {
                ("upscale-3x", "清晰放大3X")
            } else {
                ("upscale-2x", "清晰放大2X")
            }
        }
    };
    let image_buffer = image::RgbaImage::from_raw(width, height, rgba.clone())
        .ok_or_else(|| anyhow!("invalid image buffer"))?;
    let bytes = encode_png_rgba(&image_buffer, width, height)?;
    let dir = output_dir_path(app);
    fs::create_dir_all(&dir)?;
    let stem = sanitize_filename(&format!("{}-{}", state.get_viewer_title(), title_suffix));
    let path = unique_path(dir.join(format!(
        "{}-{}-{}.png",
        Local::now().format("%Y%m%d%H%M%S%3f"),
        stem,
        suffix
    )));
    fs::write(&path, bytes)?;

    let buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, width, height);
    let image = Image::from_rgba8(buffer);
    let source = state.get_viewer_source().to_string();
    let id = state.get_viewer_id().to_string();
    let original = {
        let store_ref = store.borrow();
        viewer_item(&store_ref, &id, &source).cloned()
    };
    let category = original
        .as_ref()
        .map(|item| item.category.clone())
        .unwrap_or_else(|| resolve_category(&state.get_asset_type().to_string(), ""));
    let kind = original
        .as_ref()
        .map(|item| item.kind.clone())
        .unwrap_or_else(|| state.get_mode().to_string());
    let prompt = original
        .as_ref()
        .map(|item| item.prompt.clone())
        .unwrap_or_else(|| state.get_viewer_prompt().to_string());
    let quality = match mode {
        ProcessImageMode::Upscale {
            target_long_edge, ..
        } => {
            if target_long_edge >= 4096 {
                "4K".to_string()
            } else {
                "2K".to_string()
            }
        }
        _ => original
            .as_ref()
            .map(|item| item.quality.clone())
            .unwrap_or_else(|| state.get_viewer_quality().to_string()),
    };
    let base_cutout_done = original
        .as_ref()
        .map(|item| item.cutout_done)
        .unwrap_or_else(|| state.get_viewer_cutout_done());
    let base_remove_black_done = original
        .as_ref()
        .map(|item| item.remove_black_done)
        .unwrap_or_else(|| state.get_viewer_remove_black_done());
    let base_upscale_done = original
        .as_ref()
        .map(|item| item.upscale_done)
        .unwrap_or_else(|| state.get_viewer_upscale_done());
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: original
            .as_ref()
            .map(|item| item.conversation_id.clone())
            .unwrap_or_default(),
        title: format!(
            "{} {}",
            original
                .as_ref()
                .map(|item| item.title.clone())
                .unwrap_or_else(|| state.get_viewer_title().to_string()),
            title_suffix
        ),
        category,
        kind,
        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        prompt,
        ratio: ratio_from_actual_dimensions(width as i32, height as i32),
        quality,
        model: match mode {
            ProcessImageMode::Upscale { .. } => "本地清晰放大".to_string(),
            _ => "本地处理".to_string(),
        },
        width: width as i32,
        height: height as i32,
        image,
        source_path: path.display().to_string(),
        cutout_done: base_cutout_done || matches!(mode, ProcessImageMode::Cutout),
        remove_black_done: base_remove_black_done || matches!(mode, ProcessImageMode::RemoveBlack),
        upscale_done: base_upscale_done || matches!(mode, ProcessImageMode::Upscale { .. }),
    };
    {
        let mut store_mut = store.borrow_mut();
        store_mut.assets.insert(0, item.clone());
        store_mut.generations.insert(0, item.clone());
        save_local_store(app, &store_mut);
        push_all(app, &store_mut);
    }
    Ok(())
}

fn user_profile_path() -> PathBuf {
    app_data_dir().join("user-profile.json")
}

fn set_invited_users_state(app: &AppWindow, users: &[InvitedUserData]) {
    let state = app.global::<AppState>();
    state.set_invited_users(ModelRc::new(VecModel::from(
        users.iter().map(to_invited_user_view).collect::<Vec<_>>(),
    )));
    state.set_invite_history_points(users.iter().map(|user| user.rebate_points).sum());
}

fn load_user_profile(app: &AppWindow) {
    let Ok(text) = fs::read_to_string(user_profile_path()) else {
        return;
    };
    let Ok(profile) = serde_json::from_str::<UserProfileData>(&text) else {
        return;
    };
    let state = app.global::<AppState>();
    state.set_logged_in(profile.logged_in);
    state.set_phone_mask(profile.phone_mask.into());
    state.set_nickname(profile.nickname.into());
    state.set_credit_balance(profile.credit_balance);
    state.set_credit_records(ModelRc::new(VecModel::from(
        profile
            .credit_records
            .iter()
            .map(to_credit_record_view)
            .collect::<Vec<_>>(),
    )));
    state.set_invite_code(profile.invite_code.into());
    state.set_invite_link(profile.invite_link.into());
    set_invited_users_state(app, &profile.invited_users);
    state.set_last_daily_credit_date(profile.last_daily_credit_date.into());
    if !profile.language.trim().is_empty() {
        state.set_language(profile.language.into());
    }
    if !profile.theme_id.trim().is_empty() {
        state.set_theme_id(profile.theme_id.clone().into());
        apply_theme(app, &profile.theme_id);
    }
    let card_style = if profile.card_style == "square" {
        "square"
    } else {
        "rounded"
    };
    state.set_card_style(card_style.into());
    if !profile.asset_type.trim().is_empty() {
        let category = resolve_category(&profile.asset_type, "");
        if category == "action-sequence" {
            state.set_creation_mode("anim-idle".into());
            state.set_count(1);
            state.set_ratio("1:1".into());
            state.set_ratio_more_open(false);
        }
        state.set_asset_type(category.into());
    }
    if profile.logged_in {
        ensure_credit_account(app);
        grant_daily_free_credits(app);
        state.set_page("generation".into());
    }
}

fn save_user_profile(app: &AppWindow) {
    let state = app.global::<AppState>();
    let phone_mask = state.get_phone_mask().to_string();
    let nickname = state.get_nickname().to_string();
    let profile = UserProfileData {
        logged_in: state.get_logged_in(),
        phone_mask,
        nickname,
        credit_balance: state.get_credit_balance(),
        last_daily_credit_date: state.get_last_daily_credit_date().to_string(),
        theme_id: state.get_theme_id().to_string(),
        card_style: if state.get_card_style() == "square" {
            "square".to_string()
        } else {
            "rounded".to_string()
        },
        language: state.get_language().to_string(),
        asset_type: resolve_category(&state.get_asset_type().to_string(), ""),
        invite_code: state.get_invite_code().to_string(),
        invite_link: state.get_invite_link().to_string(),
        invited_users: state
            .get_invited_users()
            .iter()
            .map(|item| InvitedUserData {
                email: item.email.to_string(),
                username: item.username.to_string(),
                rebate_points: item.rebate_points,
                register_time: item.register_time.to_string(),
            })
            .collect(),
        credit_records: state
            .get_credit_records()
            .iter()
            .map(|item| CreditRecordData {
                title: item.title.to_string(),
                amount: item.amount.to_string(),
                time: item.time.to_string(),
                note: item.note.to_string(),
                positive: item.positive,
            })
            .collect(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&profile) {
        let path = user_profile_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, text);
    }
}

fn local_store_path() -> PathBuf {
    app_data_dir().join("local-store.json")
}

fn load_local_store(app: &AppWindow, store: &Rc<RefCell<Store>>) {
    let Ok(text) = fs::read_to_string(local_store_path()) else {
        recover_output_assets(app, store);
        save_local_store(app, &store.borrow());
        return;
    };
    let Ok(data) = serde_json::from_str::<LocalStoreData>(&text) else {
        recover_output_assets(app, store);
        save_local_store(app, &store.borrow());
        return;
    };
    {
        let mut store_mut = store.borrow_mut();
        store_mut.providers = data.providers;
        store_mut.assets = data
            .assets
            .into_iter()
            .filter_map(asset_from_stored)
            .collect();
        store_mut.generations = data
            .generations
            .into_iter()
            .filter_map(asset_from_stored)
            .collect();
        store_mut.notifications = data.notifications;
        store_mut.prompt_drafts = data.prompt_drafts;
    }
    let state = app.global::<AppState>();
    state.set_image_provider_id(data.image_provider_id.into());
    state.set_image_model(data.image_model.into());
    state.set_reasoning_provider_id(data.reasoning_provider_id.into());
    state.set_reasoning_model(data.reasoning_model.into());
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    state.set_asset_type(category.clone().into());
    state.set_prompt(prompt_draft_for_category(&store.borrow().prompt_drafts, &category).into());
}

fn prompt_draft_for_category(drafts: &PromptDrafts, category: &str) -> String {
    match category {
        "scene" => drafts.scene.clone(),
        "ui" => drafts.ui.clone(),
        "effect" => drafts.effect.clone(),
        "action-sequence" => drafts.action_sequence.clone(),
        _ => drafts.character.clone(),
    }
}

fn set_prompt_draft_for_category(drafts: &mut PromptDrafts, category: &str, prompt: String) {
    match category {
        "scene" => drafts.scene = prompt,
        "ui" => drafts.ui = prompt,
        "effect" => drafts.effect = prompt,
        "action-sequence" => drafts.action_sequence = prompt,
        _ => drafts.character = prompt,
    }
}

fn store_current_prompt_draft(app: &AppWindow, store: &Rc<RefCell<Store>>, category: &str) {
    let prompt = app.global::<AppState>().get_prompt().to_string();
    set_prompt_draft_for_category(&mut store.borrow_mut().prompt_drafts, category, prompt);
}

fn references_for_category<'a>(
    references: &'a ReferenceGroups,
    category: &str,
) -> &'a Vec<ReferenceData> {
    match category {
        "scene" => &references.scene,
        "ui" => &references.ui,
        "effect" => &references.effect,
        "action-sequence" => &references.action_sequence,
        _ => &references.character,
    }
}

fn references_for_category_mut<'a>(
    references: &'a mut ReferenceGroups,
    category: &str,
) -> &'a mut Vec<ReferenceData> {
    match category {
        "scene" => &mut references.scene,
        "ui" => &mut references.ui,
        "effect" => &mut references.effect,
        "action-sequence" => &mut references.action_sequence,
        _ => &mut references.character,
    }
}

fn recover_output_assets(app: &AppWindow, store: &Rc<RefCell<Store>>) {
    let dir = output_dir_path(app);
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let mut paths = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .map(|ext| {
                    matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "png" | "jpg" | "jpeg" | "webp"
                    )
                })
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    paths.reverse();

    let mut recovered = Vec::new();
    for path in paths {
        let Ok(image) = load_image(&path) else {
            continue;
        };
        let (width, height) = image::image_dimensions(&path)
            .map(|(w, h)| (w as i32, h as i32))
            .unwrap_or((0, 0));
        let title = recovered_asset_title(&path);
        let id = Uuid::new_v4().to_string();
        recovered.push(AssetData {
            id,
            conversation_id: Uuid::new_v4().to_string(),
            title: title.clone(),
            category: "other".to_string(),
            kind: "game".to_string(),
            time: "本地恢复".to_string(),
            prompt: title,
            ratio: ratio_from_actual_dimensions(width, height),
            quality: quality_from_actual_dimensions(width, height),
            model: "本地文件".to_string(),
            width,
            height,
            image,
            source_path: path.display().to_string(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
        });
    }
    if recovered.is_empty() {
        return;
    }
    let mut store_mut = store.borrow_mut();
    store_mut.assets = recovered.clone();
    store_mut.generations = recovered;
}

fn recovered_asset_title(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("本地图片");
    let title = stem
        .split_once('-')
        .map(|(_, rest)| rest)
        .unwrap_or(stem)
        .replace('_', " ");
    if title.trim().is_empty() {
        "本地图片".to_string()
    } else {
        title
    }
}

fn save_local_store(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let data = LocalStoreData {
        providers: store.providers.clone(),
        generations: store.generations.iter().map(stored_asset_from).collect(),
        assets: store.assets.iter().map(stored_asset_from).collect(),
        notifications: store.notifications.clone(),
        image_provider_id: state.get_image_provider_id().to_string(),
        image_model: state.get_image_model().to_string(),
        reasoning_provider_id: state.get_reasoning_provider_id().to_string(),
        reasoning_model: state.get_reasoning_model().to_string(),
        prompt_drafts: store.prompt_drafts.clone(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&data) {
        let path = local_store_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, text);
    }
}

fn stored_asset_from(asset: &AssetData) -> StoredAssetData {
    StoredAssetData {
        id: asset.id.clone(),
        conversation_id: asset.conversation_id.clone(),
        title: asset.title.clone(),
        category: asset.category.clone(),
        kind: asset.kind.clone(),
        time: asset.time.clone(),
        prompt: asset.prompt.clone(),
        ratio: asset.ratio.clone(),
        quality: asset.quality.clone(),
        model: asset.model.clone(),
        width: asset.width,
        height: asset.height,
        source_path: asset.source_path.clone(),
        cutout_done: asset.cutout_done,
        remove_black_done: asset.remove_black_done,
        upscale_done: asset.upscale_done,
    }
}

fn asset_from_stored(asset: StoredAssetData) -> Option<AssetData> {
    let image = if asset.source_path == "failed" || asset.source_path.trim().is_empty() {
        Image::default()
    } else {
        load_image(&PathBuf::from(&asset.source_path)).ok()?
    };
    Some(AssetData {
        id: asset.id,
        conversation_id: asset.conversation_id,
        title: asset.title,
        category: asset.category,
        kind: asset.kind,
        time: asset.time,
        prompt: asset.prompt,
        ratio: asset.ratio,
        quality: asset.quality,
        model: asset.model,
        width: asset.width,
        height: asset.height,
        image,
        source_path: asset.source_path,
        cutout_done: asset.cutout_done,
        remove_black_done: asset.remove_black_done,
        upscale_done: asset.upscale_done,
    })
}

fn ensure_credit_account(app: &AppWindow) {
    let state = app.global::<AppState>();
    if state.get_credit_balance() > 0 || state.get_credit_records().row_count() > 0 {
        return;
    }
    state.set_credit_balance(1000);
    let record = CreditRecordData {
        title: "新用户赠送积分".to_string(),
        amount: "+1000".to_string(),
        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        note: "登录后自动发放".to_string(),
        positive: true,
    };
    state.set_credit_records(ModelRc::new(VecModel::from(vec![to_credit_record_view(
        &record,
    )])));
}

fn add_credit_record(app: &AppWindow, title: &str, amount: i32, note: &str, positive: bool) {
    let state = app.global::<AppState>();
    let mut records = state.get_credit_records().iter().collect::<Vec<_>>();
    records.insert(
        0,
        CreditRecord {
            title: title.into(),
            amount: if positive {
                format!("+{}", amount).into()
            } else {
                format!("-{}", amount).into()
            },
            time: Local::now().format("%Y-%m-%d %H:%M").to_string().into(),
            note: note.into(),
            positive,
        },
    );
    state.set_credit_records(ModelRc::new(VecModel::from(records)));
}

fn grant_daily_free_credits(app: &AppWindow) {
    let state = app.global::<AppState>();
    let today = Local::now().format("%Y-%m-%d").to_string();
    if state.get_last_daily_credit_date().as_str() == today {
        return;
    }
    state.set_last_daily_credit_date(today.into());
    state.set_credit_balance(state.get_credit_balance() + DAILY_FREE_CREDITS);
    add_credit_record(
        app,
        "每日免费积分",
        DAILY_FREE_CREDITS,
        "每日登录赠送，可生成 5 张 1K 图",
        true,
    );
    save_user_profile(app);
}

fn charge_credits(app: &AppWindow, amount: i32, title: &str, note: &str) -> bool {
    if amount <= 0 {
        return true;
    }
    let state = app.global::<AppState>();
    if state.get_credit_balance() < amount {
        state.set_generation_status("积分不足，请前往充值".into());
        state.set_credit_insufficient_message("积分不足，请前往充值".into());
        state.set_credit_insufficient_open(true);
        return false;
    }
    state.set_credit_balance(state.get_credit_balance() - amount);
    add_credit_record(app, title, amount, note, false);
    save_user_profile(app);
    true
}

fn refund_credits(app: &AppWindow, amount: i32, title: &str, note: &str) {
    if amount <= 0 {
        return;
    }
    let state = app.global::<AppState>();
    state.set_credit_balance(state.get_credit_balance() + amount);
    add_credit_record(app, title, amount, note, true);
    save_user_profile(app);
}

fn recharge_credits(app: &AppWindow, amount: i32) {
    let state = app.global::<AppState>();
    if !state.get_logged_in() {
        state.set_auth_open(true);
        return;
    }
    state.set_credit_balance(state.get_credit_balance() + amount);
    add_credit_record(app, "充值积分", amount, "支付宝扫码付款", true);
    save_user_profile(app);
}
fn load_showcase_images(app: &AppWindow) {
    let state = app.global::<AppState>();
    if let Some(path) = asset_path("showcase/character.png") {
        if let Ok(image) = load_image(&path) {
            state.set_showcase_character(image);
        }
    }
    if let Some(path) = asset_path("showcase/scene.png") {
        if let Ok(image) = load_image(&path) {
            state.set_showcase_scene(image);
        }
    }
    if let Some(path) = asset_path("showcase/ui.png") {
        if let Ok(image) = load_image(&path) {
            state.set_showcase_ui(image);
        }
    }
    if let Some(path) = asset_path("showcase/vfx.png") {
        if let Ok(image) = load_image(&path) {
            state.set_showcase_vfx(image);
        }
    }
}

fn asset_path(relative: &str) -> Option<PathBuf> {
    resource_base_dirs()
        .into_iter()
        .map(|base| base.join("assets").join(relative))
        .find(|path| path.exists())
}

fn seed_inspiration(app: &AppWindow, store: &Rc<RefCell<Store>>) -> Result<()> {
    let dirs = inspiration_dirs();
    let mut items = Vec::new();
    let mut seen_files = BTreeSet::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let mut paths = fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_string()
        });
        for path in paths {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                continue;
            }
            let file_key = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_ascii_lowercase())
                .unwrap_or_else(|| path.display().to_string().to_ascii_lowercase());
            if !seen_files.insert(file_key) {
                continue;
            }
            if let Ok(image) = load_image(&path) {
                let index = items.len() + 1;
                let (title, category, kind) = inspiration_meta(index);
                let (width, height) = image::image_dimensions(&path)
                    .map(|(w, h)| (w as i32, h as i32))
                    .unwrap_or((1254, 1254));
                let ratio = ratio_from_actual_dimensions(width, height);
                let quality = quality_from_actual_dimensions(width, height);
                items.push(AssetData {
                    id: format!("inspiration-{index}"),
                    conversation_id: String::new(),
                    title: title.to_string(),
                    category: category.to_string(),
                    kind: kind.to_string(),
                    time: "官方示例".to_string(),
                    prompt: inspiration_prompt(index, title, &ratio),
                    ratio,
                    quality,
                    model: "官方示例".to_string(),
                    width,
                    height,
                    image,
                    source_path: path.display().to_string(),
                    cutout_done: false,
                    remove_black_done: false,
                    upscale_done: false,
                });
            }
        }
    }
    store.borrow_mut().inspiration = items;
    push_all(app, &store.borrow());
    Ok(())
}

fn inspiration_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for base in resource_base_dirs() {
        push_unique_path(&mut dirs, base.join("assets").join("sucai"));
    }
    dirs
}

fn inspiration_meta(index: usize) -> (&'static str, &'static str, &'static str) {
    [
        ("东方巨像", "scene", "game"),
        ("游戏 UI 套件", "ui", "game"),
        ("奇幻角色图标", "character", "game"),
        ("村庄场景地图", "scene", "game"),
        ("角色设计图", "character", "game"),
        ("迷你角色图标", "character", "game"),
        ("沙漠场景", "scene", "game"),
        ("战略游戏画面", "scene", "film"),
        ("Q 版图标 UI", "ui", "game"),
        ("RPG 贴图集", "ui", "game"),
        ("技能特效", "effect", "game"),
        ("奇幻角色集", "character", "game"),
        ("怪物图标", "character", "game"),
        ("装备栏 UI", "ui", "game"),
        ("像素 BOSS 战", "scene", "film"),
        ("魔法森林", "scene", "game"),
        ("城市场景", "scene", "film"),
        ("丧尸角色", "character", "game"),
        ("像素魔女", "character", "game"),
        ("重甲骑士", "character", "game"),
        ("日式 RPG UI", "ui", "game"),
        ("复古游戏 UI", "ui", "game"),
        ("特效设计", "effect", "game"),
        ("游戏 CG 角色", "character", "game"),
    ]
    .get(index.saturating_sub(1))
    .copied()
    .unwrap_or(("官方示例", "other", "game"))
}

fn inspiration_prompt(index: usize, title: &str, ratio: &str) -> String {
    match index {
        1 => "航拍，俯视，巨物恐惧症，背景呈现庞大的东方玄幻风格建筑。超大型力士像伸手向人类挥舞，游戏 CG 风格，全景镜头，全身动态动作，景深效果，倾斜失衡构图，巨型比例，C4D建模，Blender制作，虚幻引擎，Octane渲染，全局光照，光线追踪反射，屏幕空间环境光遮蔽，着色器，快速近似抗锯齿，电脑生成图像，实时光线追踪，视觉特效，4K画质，最佳品质，超精细，超写实，幽暗奇幻，暗黑风格，粗粝质感，微妙色调。".to_string(),
        2 => "生成一组游戏风格UI素材，包含上百个不同样式的按钮、面板、进度条，采用日式 RPG风格，不应受到阴影或光线的影响。大师，杰作。背景是白色的".to_string(),
        3 | 6 | 10 | 12 | 13 => "一套包含各种奇幻rpg迷你角色图标的贴图集，包括埃及妖精、兜帽法师、骑士和怪物等，采用可爱的q版风格，线条粗犷利落，色彩鲜艳，平面2d游戏画风，符合手游美学，细节丰富，纯深色灰背景，风格类似《王国保卫战》".to_string(),
        4 => "小村庄游戏场景地图，带顶视图的RPG Maker 风格地图，Chrono Trigger 风格，画面风格是 90 年代复古像素风，风格参考《塞尔达传说》复古像素游戏，轮廓用粗黑像素线条勾勒，色彩块面分明，色调以高饱和的复古游戏色绿和黄为主，红、蓝等为辅助，明亮的，包含：有两栋房子，几个村子，玉米地，喷泉，农场，养鸡场，聊天的村民，".to_string(),
        5 => "人设设计超详细图，背景白色，展示了每件作品的复杂设计过程，图纸包括对角色各部分大量尺寸和解释性文本注释，英文文字的设计说明,不同角度的零散缩略图增加了场景的深度，每个细节都有展示，极具想象力，丰富联想，水彩融合水墨，极具设计感服装，人设，超细节，吕布化身为巨大半透明由狂暴能量构成的深红色武魂真身，手持长枪呈战斗姿态，深红色煞气如火焰般燃烧升腾暗红色能量闪电在周身噼啪作响。暗黑美学，国风玄幻，CG艺术,特写,极具动态和攻击性，最高画质，压迫感强,超高细节。长卷构图，艺术设计。".to_string(),
        7 => "卡通风格的插图，一个游戏场景，一个沙漠场景，地面水平线在画面自上而下的十分之一处，画面中间是广阔的浅黄色的沙地，沙地占画面的十分之九，小小的绿色的仙人掌与小红色的花朵在左边，远处的废墟由红褐色的岩石和白色的岩石柱子还有一棵棕榈树与棕色的树干组成，色调不要太明亮，和平和安静的气氛，2D游戏资产".to_string(),
        8 => "生成即时战略游戏的游戏画面。".to_string(),
        9 => "2D游戏，游戏图标ui设计，Q版卡通游戏UI，等距视角，手绘治愈Q 版萌系、柔和暖色调，模拟经营游戏 UI、温馨田园风，日式治愈风格，手绘萌系质感风格，生成15个萌系游戏图标，15个一组在同一个画面中，生成2D 卡通 / 动漫游戏风格的图标，充满装饰性元素，可爱童话风格，生成不同组合的萌系厨房卫生间卧室空间需要有：（卧室、卫生间，厨房，书房，电竞房，健身房，客厅）；高级感，大师杰作，视觉上简洁明快，“平滑的卡通渲染质感”，色彩明亮清新，刻画细节，过渡柔和，矢量插画，细腻的写实光照，柔和的光影完美呈现，光影过渡自然，勾勒清晰的线稿，整体氛围轻松愉悦，层次丰富，塑造扁平，“低饱和对比色”，纯灰色背景，不要出现汉字".to_string(),
        11 => "游戏特效，没有人物，没有主角，灰色背景，有层次的，俯视角，平面，多个不一样的设计，素材，排列整齐，火焰".to_string(),
        14 => "游戏传奇界面的一组游戏风格UI装备栏合集，采用热血传奇游戏风格，火龙，精美 的细节，边缘有图案和装饰元素，不应受到阴影或光线的影响。大师杰作，黑色背景".to_string(),
        15 => "16-bit像素勇者大战复古游戏BOSS，怀旧像素颗粒，有限色板抖动，[RPG/横版动作]场景选择".to_string(),
        16 => "美漫卡通风格，游戏场景图，没有人，魔法森林，魔法祭台，粗线条，扁平画风，没有人".to_string(),
        17 => "设计一个2D的45度角游戏场景，漫画平涂简单风格，Q版。场景主题：城市战场边缘 核心关键词：铁丝网、沙袋、战壕、废弃建筑 地形：废弃城市的郊区地带，周围有废墟和破旧汽车 基地：现代化兵工厂，门口摆放弹药或军用设施".to_string(),
        18 => "生成q版2d游戏角色，丧尸，漫画平涂风格。两类丧尸怪形象，一类瘦丧尸，敏捷型，一栋速度快，血量少。另外一类肉型，移动慢，血量高。可以配合一些现代的武器或者配饰。".to_string(),
        19 => "游戏角色设计，像素风，纯白色背景，四视图，正视图，侧视图，背视图，可爱，小魔女，拿着一个小的法杖，带着魔法帽，可爱，高饱和度的配色，光影艺术，单色背景，美丽，近距离".to_string(),
        20 => "生成一个像素风格的重甲骑士三视图，并且要在下方展示武器特写".to_string(),
        22 => "生成一组游戏风格UI素材，包含上百个不同样式的按钮、面板、进度条，采用复古美食荒野牛仔风格，不应受到阴影或光线的影响。大师，杰作。".to_string(),
        23 => "游戏特效，没有人物，没有主角，灰色背景，有层次的，俯视角，平面，多个不一样的设计，素材，排列整齐，暗黑，爆炸后的地面痕迹，没有火焰，没有烟雾".to_string(),
        24 => "游戏CG风格，隐约的淡彩褪色，泛朦，对焦模糊，特写，剑风传奇格斯，高颜值，复古，迷离，低饱和，反射，质感，泛光模糊晕染，高噪点，胶片颗粒质感，极具艺术感，震撼人心，色彩丰富，暗部叠加，特写镜头，超高清。落雪飞溅，前景落雪虚化，动态模糊，背景动态虚化，阳光灿烂，蓝天白云，光影交错，特写镜头，突出速度感和视觉冲击力，强透视，原比例。".to_string(),
        _ => {
            format!("{title}，{ratio} 构图，官方灵感示例，可用于做同款或作为参考图继续创作。")
        }
    }
}

fn open_provider_editor(app: &AppWindow, store: &Store, id: &str) {
    let state = app.global::<AppState>();
    state.set_edit_provider_id(id.into());
    state.set_pending_image_model("".into());
    state.set_pending_reasoning_model("".into());
    if let Some(provider) = store.providers.iter().find(|p| p.id == id) {
        state.set_provider_name(provider.name.clone().into());
        state.set_provider_remark(provider.remark.clone().into());
        state.set_provider_website(provider.website.clone().into());
        state.set_provider_endpoint(provider.endpoint.clone().into());
        state.set_provider_api_key(provider.api_key.clone().into());
        state.set_provider_model_name(provider.selected_model.clone().into());
        state.set_fetched_models(ModelRc::new(VecModel::from(
            provider
                .models
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
        state.set_provider_used_models(ModelRc::new(VecModel::from(
            normalized_used_models(provider.used_models.clone(), &provider.models)
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
        if state.get_image_provider_id().as_str() == id {
            state.set_pending_image_model(state.get_image_model());
        }
        if state.get_reasoning_provider_id().as_str() == id {
            state.set_pending_reasoning_model(state.get_reasoning_model());
        }
    } else {
        state.set_provider_name("".into());
        state.set_provider_remark("".into());
        state.set_provider_website("".into());
        state.set_provider_endpoint("".into());
        state.set_provider_api_key("".into());
        state.set_provider_model_name("".into());
        state.set_fetched_models(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
        state.set_provider_used_models(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
    }
    push_provider_model_options(app);
    state.set_provider_message("".into());
    state.set_provider_editor_open(true);
}

fn open_viewer(app: &AppWindow, store: &Store, id: &str, source: &str) {
    let item = match source {
        "asset" => store.assets.iter().find(|a| a.id == id),
        "inspiration" => store.inspiration.iter().find(|a| a.id == id),
        _ => store.generations.iter().find(|a| a.id == id),
    };
    let Some(item) = item else {
        return;
    };
    let state = app.global::<AppState>();
    state.set_viewer_message("".into());
    state.set_viewer_id(item.id.clone().into());
    state.set_viewer_source(source.into());
    state.set_viewer_image(item.image.clone());
    state.set_viewer_title(item.title.clone().into());
    state.set_viewer_prompt(item.prompt.clone().into());
    state.set_viewer_prompt_lines(estimated_prompt_lines(&item.prompt));
    state.set_viewer_time(item.time.clone().into());
    state.set_viewer_ratio(item.ratio.clone().into());
    state.set_viewer_quality(item.quality.clone().into());
    state.set_viewer_model(item.model.clone().into());
    state.set_viewer_cutout_done(item.cutout_done);
    state.set_viewer_remove_black_done(item.remove_black_done);
    state.set_viewer_upscale_done(item.upscale_done);
    let (width, height) = if item.width > 32 && item.height > 32 {
        (item.width, item.height)
    } else {
        pixel_dimensions_for(&item.ratio, &item.quality)
    };
    state.set_viewer_width(width);
    state.set_viewer_height(height);
    state.set_viewer_open(true);
}

fn estimated_prompt_lines(prompt: &str) -> i32 {
    let estimated_chars_per_line = 28;
    let lines = prompt
        .lines()
        .map(|line| {
            let chars = line.chars().count();
            ((chars + estimated_chars_per_line - 1) / estimated_chars_per_line).max(1)
        })
        .sum::<usize>()
        .max(1);
    lines.min(1000) as i32
}

fn move_viewer(app: &AppWindow, store: &Store, direction: i32) {
    let state = app.global::<AppState>();
    let source = state.get_viewer_source().to_string();
    if source == "reference" {
        return;
    }
    let current_id = state.get_viewer_id().to_string();
    let ids = viewer_ids(app, store, &source);
    let Some(index) = ids.iter().position(|id| id == &current_id) else {
        return;
    };
    if direction < 0 && index == 0 {
        state.set_viewer_message(
            if state.get_language().as_str() == "en" {
                "This is the first image."
            } else {
                "当前已是第一张，"
            }
            .into(),
        );
        return;
    }
    if direction > 0 && index + 1 >= ids.len() {
        state.set_viewer_message(
            if state.get_language().as_str() == "en" {
                "This is the last image."
            } else {
                "当前已是最后一张，"
            }
            .into(),
        );
        return;
    }
    let next_index = if direction < 0 { index - 1 } else { index + 1 };
    if let Some(next_id) = ids.get(next_index) {
        open_viewer(app, store, next_id, &source);
    }
}

fn viewer_ids(app: &AppWindow, store: &Store, source: &str) -> Vec<String> {
    let state = app.global::<AppState>();
    let current_id = state.get_viewer_id().to_string();
    let visible_ids = match source {
        "asset" => state
            .get_assets()
            .iter()
            .map(|item| item.id.to_string())
            .collect::<Vec<_>>(),
        "inspiration" => state
            .get_inspiration()
            .iter()
            .map(|item| item.id.to_string())
            .collect::<Vec<_>>(),
        _ => state
            .get_generations()
            .iter()
            .map(|item| item.id.to_string())
            .collect::<Vec<_>>(),
    };
    if visible_ids.iter().any(|id| id == &current_id) {
        return visible_ids;
    }
    match source {
        "asset" => store.assets.iter().map(|item| item.id.clone()).collect(),
        "inspiration" => store
            .inspiration
            .iter()
            .map(|item| item.id.clone())
            .collect(),
        _ => store
            .generations
            .iter()
            .map(|item| item.id.clone())
            .collect(),
    }
}

fn navigate_to(app: &AppWindow, page: &str) {
    let state = app.global::<AppState>();
    if page != "welcome" && !state.get_logged_in() {
        state.set_auth_open(true);
        return;
    }
    state.set_page(page.into());
}

fn navigate_to_with_store(app: &AppWindow, store: &Store, page: &str) {
    navigate_to(app, page);
    if page == "assets" && app.global::<AppState>().get_logged_in() {
        app.global::<AppState>()
            .set_asset_category_filter("all".into());
        push_assets(app, store);
    }
    if page == "generation" && app.global::<AppState>().get_logged_in() {
        push_generations(app, store);
    }
}

fn push_all(app: &AppWindow, store: &Store) {
    push_providers(app, store);
    push_conversations(app, store);
    push_assets(app, store);
    push_generations(app, store);
    push_inspiration(app, store);
    push_notifications(app, store);
    push_references(app, store);
}

fn push_providers(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    state.set_provider_used_filter_active(store.providers.iter().any(|provider| {
        !normalized_used_models(provider.used_models.clone(), &provider.models).is_empty()
    }));
    state.set_providers(ModelRc::new(VecModel::from(
        store
            .providers
            .iter()
            .map(to_provider_view)
            .collect::<Vec<_>>(),
    )));
}

fn push_conversations(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let mut seen = BTreeSet::new();
    let mut conversations = Vec::new();
    for item in store
        .generations
        .iter()
        .filter(|item| item.source_path != "failed" && !item.conversation_id.trim().is_empty())
    {
        if !seen.insert(item.conversation_id.clone()) {
            continue;
        }
        conversations.push(ConversationItem {
            id: item.conversation_id.clone().into(),
            title: short_text(&item.title, 10).into(),
            image: item.image.clone(),
            loading: false,
        });
    }
    if state
        .get_current_conversation_id()
        .as_str()
        .trim()
        .is_empty()
    {
        if let Some(first) = conversations.first() {
            state.set_current_conversation_id(first.id.clone());
        }
    }
    state.set_conversations(ModelRc::new(VecModel::from(conversations)));
}

fn push_assets(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    state.set_asset_character_count(count_assets(store, "character"));
    state.set_asset_scene_count(count_assets(store, "scene"));
    state.set_asset_ui_count(count_assets(store, "ui"));
    state.set_asset_effect_count(count_assets(store, "effect"));
    state.set_asset_other_count(count_assets(store, "other"));
    state.set_asset_all_count(store.assets.len() as i32);
    let kind = "all".to_string();
    let category = state.get_asset_category_filter().to_string();
    let filtered = store
        .assets
        .iter()
        .filter(|item| include_gallery_item(item, &kind, &category))
        .collect::<Vec<_>>();
    state.set_assets(ModelRc::new(VecModel::from(
        filtered
            .iter()
            .map(|item| to_asset_view(item))
            .collect::<Vec<_>>(),
    )));
    state.set_asset_groups(ModelRc::new(VecModel::from(group_asset_views(
        &filtered,
        state.get_language().as_str(),
    ))));
    let cols = split_asset_row_columns(filtered);
    state.set_asset_col_0(ModelRc::new(VecModel::from(cols[0].clone())));
    state.set_asset_col_1(ModelRc::new(VecModel::from(cols[1].clone())));
    state.set_asset_col_2(ModelRc::new(VecModel::from(cols[2].clone())));
    state.set_asset_col_3(ModelRc::new(VecModel::from(cols[3].clone())));
    state.set_asset_col_4(ModelRc::new(VecModel::from(cols[4].clone())));
}

fn push_generations(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    let current_items = store
        .generations
        .iter()
        .filter(|item| item.category == category)
        .collect::<Vec<_>>();
    state.set_generations(ModelRc::new(VecModel::from(
        current_items
            .iter()
            .map(|item| to_asset_view(item))
            .collect::<Vec<_>>(),
    )));
    state.set_generation_groups(ModelRc::new(VecModel::from(group_asset_views(
        &current_items,
        state.get_language().as_str(),
    ))));
}

fn push_inspiration(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let kind = "all".to_string();
    let category = state.get_inspiration_category_filter().to_string();
    let filtered = store
        .inspiration
        .iter()
        .filter(|item| include_gallery_item(item, &kind, &category))
        .collect::<Vec<_>>();
    state.set_inspiration(ModelRc::new(VecModel::from(
        filtered
            .iter()
            .map(|item| to_asset_view(item))
            .collect::<Vec<_>>(),
    )));
    let cols = split_gallery_columns(filtered);
    state.set_inspiration_col_0(ModelRc::new(VecModel::from(cols[0].clone())));
    state.set_inspiration_col_1(ModelRc::new(VecModel::from(cols[1].clone())));
    state.set_inspiration_col_2(ModelRc::new(VecModel::from(cols[2].clone())));
    state.set_inspiration_col_3(ModelRc::new(VecModel::from(cols[3].clone())));
    state.set_inspiration_col_4(ModelRc::new(VecModel::from(cols[4].clone())));
}

fn include_gallery_item(item: &AssetData, kind: &str, category: &str) -> bool {
    if kind != "all" && item.kind != kind {
        return false;
    }
    if category == "all" {
        return true;
    }
    item.category == category
}

fn group_asset_views(items: &[&AssetData], language: &str) -> Vec<AssetGroup> {
    let mut groups: Vec<(String, Vec<AssetItem>)> = Vec::new();
    for asset in items {
        let title = time_group_label(&asset.time, language);
        if groups.last().map(|(last_title, _)| last_title.as_str()) != Some(title.as_str()) {
            groups.push((title.clone(), Vec::new()));
        }
        if let Some((_, group_items)) = groups.last_mut() {
            group_items.push(to_asset_view(asset));
        }
    }
    groups
        .into_iter()
        .map(|(title, items)| AssetGroup {
            title: title.into(),
            items: ModelRc::new(VecModel::from(items)),
        })
        .collect()
}

fn time_group_label(time: &str, language: &str) -> String {
    let date_text = time.split_whitespace().next().unwrap_or("").trim();
    let today = Local::now().date_naive();
    let english = language == "en";
    if let Ok(date) = NaiveDate::parse_from_str(date_text, "%Y-%m-%d") {
        if date == today {
            return if english { "Today" } else { "今天" }.to_string();
        }
        if date == today - ChronoDuration::days(1) {
            return if english { "Yesterday" } else { "昨天" }.to_string();
        }
        if date.year() == today.year() {
            return if english {
                format!("{}/{}", date.month(), date.day())
            } else {
                format!("{}月{}日", date.month(), date.day())
            };
        }
        return if english {
            format!("{}/{}/{}", date.year(), date.month(), date.day())
        } else {
            format!("{}年{}月{}日", date.year(), date.month(), date.day())
        };
    }
    if time.trim().is_empty() {
        return if english {
            "Unknown date"
        } else {
            "未知日期"
        }
        .to_string();
    }
    time.trim().to_string()
}

fn split_gallery_columns(items: Vec<&AssetData>) -> [Vec<AssetItem>; 5] {
    let mut cols: [Vec<AssetItem>; 5] =
        [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut heights = [0_i64; 5];
    for item in items {
        let index = heights
            .iter()
            .enumerate()
            .min_by_key(|(_, height)| **height)
            .map(|(index, _)| index)
            .unwrap_or(0);
        heights[index] += gallery_height_score(item);
        cols[index].push(to_asset_view(item));
    }
    cols
}

fn split_asset_row_columns(items: Vec<&AssetData>) -> [Vec<AssetItem>; 5] {
    let mut cols: [Vec<AssetItem>; 5] =
        [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for row in items.chunks(5) {
        let row_items = row
            .iter()
            .map(|item| to_asset_view(item))
            .collect::<Vec<_>>();
        for (index, item) in row_items.into_iter().enumerate() {
            cols[index].push(item);
        }
    }
    cols
}

fn gallery_height_score(item: &AssetData) -> i64 {
    if item.width <= 0 || item.height <= 0 {
        return 248;
    }
    ((item.height as i64) * 220 / (item.width as i64)).max(128)
}

fn count_assets(store: &Store, category: &str) -> i32 {
    store
        .assets
        .iter()
        .filter(|item| item.kind == "game" && item.category == category)
        .count() as i32
}

fn push_references(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    let max_references = max_reference_images_for_category(&category);
    state.set_references(ModelRc::new(VecModel::from(
        references_for_category(&store.references, &category)
            .iter()
            .take(max_references)
            .map(|item| ReferenceItem {
                id: item.id.clone().into(),
                image: item.image.clone(),
                source_path: item.source_path.clone().into(),
            })
            .collect::<Vec<_>>(),
    )));
}

fn push_notifications(app: &AppWindow, store: &Store) {
    let has_unread = store.notifications.iter().any(|n| !n.read);
    let state = app.global::<AppState>();
    state.set_has_unread(has_unread);
    state.set_notifications(ModelRc::new(VecModel::from(
        store
            .notifications
            .iter()
            .map(|n| NotificationItem {
                id: n.id.clone().into(),
                title: n.title.clone().into(),
                model: n.model.clone().into(),
                time: n.time.clone().into(),
                reason: n.reason.clone().into(),
                success: n.success,
                read: n.read,
            })
            .collect::<Vec<_>>(),
    )));
}

fn to_provider_view(provider: &ProviderData) -> ModelProvider {
    ModelProvider {
        id: provider.id.clone().into(),
        name: provider.name.clone().into(),
        endpoint: provider.endpoint.clone().into(),
        remark: provider.remark.clone().into(),
        website: provider.website.clone().into(),
        api_key: provider.api_key.clone().into(),
        models: ModelRc::new(VecModel::from(
            provider
                .models
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )),
        used_models: ModelRc::new(VecModel::from(
            normalized_used_models(provider.used_models.clone(), &provider.models)
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )),
        selected_model: provider.selected_model.clone().into(),
    }
}

fn to_asset_view(asset: &AssetData) -> AssetItem {
    AssetItem {
        id: asset.id.clone().into(),
        title: asset.title.clone().into(),
        category: asset.category.clone().into(),
        kind: asset.kind.clone().into(),
        time: asset.time.clone().into(),
        prompt: asset.prompt.clone().into(),
        ratio: asset.ratio.clone().into(),
        quality: asset.quality.clone().into(),
        model: asset.model.clone().into(),
        width: asset.width,
        height: asset.height,
        image: asset.image.clone(),
        source_path: asset.source_path.clone().into(),
        drag_uri: file_uri_for_path(&asset.source_path).into(),
        cutout_done: asset.cutout_done,
        remove_black_done: asset.remove_black_done,
        upscale_done: asset.upscale_done,
    }
}

fn to_credit_record_view(record: &CreditRecordData) -> CreditRecord {
    CreditRecord {
        title: record.title.clone().into(),
        amount: record.amount.clone().into(),
        time: record.time.clone().into(),
        note: record.note.clone().into(),
        positive: record.positive,
    }
}

fn to_invited_user_view(user: &InvitedUserData) -> InvitedUser {
    InvitedUser {
        email: user.email.clone().into(),
        username: user.username.clone().into(),
        rebate_points: user.rebate_points,
        register_time: user.register_time.clone().into(),
    }
}

fn start_countdown(app_weak: Weak<AppWindow>) {
    let timer = Rc::new(slint::Timer::default());
    let timer_for_tick = timer.clone();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(1),
        move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let value = state.get_auth_countdown();
                if value <= 0 {
                    timer_for_tick.stop();
                } else {
                    state.set_auth_countdown(value - 1);
                }
            } else {
                timer_for_tick.stop();
            }
        },
    );
}

fn apply_theme(app: &AppWindow, theme: &str) {
    match theme {
        "sprite" => set_theme_palette(
            app,
            (236, 251, 244),
            (255, 255, 255),
            (224, 248, 238),
            (194, 235, 217),
            (7, 19, 15),
            (80, 98, 91),
            (141, 160, 150),
            (0, 217, 130),
            (6, 185, 111),
            (0, 200, 120),
            (245, 165, 36),
            (239, 105, 105),
        ),
        "light" => set_theme_palette(
            app,
            (250, 250, 252),
            (255, 255, 255),
            (244, 244, 248),
            (228, 228, 236),
            (31, 32, 48),
            (74, 76, 96),
            (138, 140, 160),
            (79, 70, 229),
            (67, 56, 202),
            (34, 197, 94),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "ocean" | "blue" => set_theme_palette(
            app,
            (6, 11, 20),
            (12, 16, 28),
            (24, 36, 60),
            (24, 34, 58),
            (228, 236, 248),
            (184, 196, 216),
            (120, 144, 168),
            (14, 165, 233),
            (2, 132, 199),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "warm" => set_theme_palette(
            app,
            (12, 8, 6),
            (20, 14, 10),
            (36, 26, 16),
            (42, 30, 22),
            (244, 236, 220),
            (216, 196, 168),
            (156, 124, 88),
            (245, 158, 11),
            (217, 119, 6),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "forest" => set_theme_palette(
            app,
            (6, 14, 10),
            (12, 22, 16),
            (24, 36, 26),
            (24, 42, 30),
            (228, 244, 236),
            (184, 216, 196),
            (120, 156, 132),
            (34, 197, 94),
            (22, 163, 74),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "rose" => set_theme_palette(
            app,
            (12, 6, 8),
            (20, 10, 14),
            (36, 18, 24),
            (42, 24, 32),
            (244, 220, 228),
            (216, 180, 192),
            (156, 104, 120),
            (244, 63, 94),
            (225, 29, 72),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "cyber" => set_theme_palette(
            app,
            (10, 4, 16),
            (20, 8, 28),
            (42, 20, 56),
            (44, 18, 68),
            (244, 220, 248),
            (216, 180, 220),
            (160, 112, 172),
            (217, 70, 239),
            (168, 85, 247),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "oled" => set_theme_palette(
            app,
            (0, 0, 0),
            (8, 8, 8),
            (24, 24, 24),
            (26, 26, 26),
            (240, 240, 240),
            (184, 184, 184),
            (112, 112, 112),
            (16, 185, 129),
            (5, 150, 105),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "cream" => set_theme_palette(
            app,
            (242, 235, 224),
            (247, 240, 230),
            (235, 227, 213),
            (216, 207, 194),
            (58, 48, 37),
            (102, 90, 78),
            (158, 146, 134),
            (201, 107, 115),
            (163, 78, 88),
            (34, 197, 94),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "system" => set_theme_palette(
            app,
            (236, 251, 244),
            (255, 255, 255),
            (224, 248, 238),
            (194, 235, 217),
            (7, 19, 15),
            (80, 98, 91),
            (141, 160, 150),
            (0, 217, 130),
            (6, 185, 111),
            (0, 200, 120),
            (245, 165, 36),
            (239, 105, 105),
        ),
        "user" => set_theme_palette(
            app,
            (250, 250, 252),
            (255, 255, 255),
            (244, 244, 248),
            (228, 228, 236),
            (31, 32, 48),
            (74, 76, 96),
            (138, 140, 160),
            (91, 95, 199),
            (67, 56, 202),
            (34, 197, 94),
            (245, 158, 11),
            (239, 68, 68),
        ),
        _ => set_theme_palette(
            app,
            (6, 6, 14),
            (12, 12, 28),
            (20, 20, 42),
            (24, 24, 56),
            (228, 228, 244),
            (184, 184, 204),
            (120, 120, 160),
            (79, 70, 229),
            (67, 56, 202),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
    }
}

fn set_theme_palette(
    app: &AppWindow,
    bg: (u8, u8, u8),
    panel: (u8, u8, u8),
    panel_soft: (u8, u8, u8),
    border: (u8, u8, u8),
    text: (u8, u8, u8),
    muted: (u8, u8, u8),
    weak: (u8, u8, u8),
    accent: (u8, u8, u8),
    accent_dark: (u8, u8, u8),
    success: (u8, u8, u8),
    warning: (u8, u8, u8),
    danger: (u8, u8, u8),
) {
    let p = app.global::<AppTheme>();
    p.set_bg(rgb(bg));
    p.set_panel(rgb(panel));
    p.set_panel_soft(rgb(panel_soft));
    p.set_border(rgb(border));
    p.set_text(rgb(text));
    p.set_muted(rgb(muted));
    p.set_weak(rgb(weak));
    p.set_accent(rgb(accent));
    p.set_accent_dark(rgb(accent_dark));
    p.set_success(rgb(success));
    p.set_warning(rgb(warning));
    p.set_danger(rgb(danger));
}

fn rgb((r, g, b): (u8, u8, u8)) -> slint::Color {
    slint::Color::from_rgb_u8(r, g, b)
}

fn normalize_image_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/images/generations") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/images/generations")
    }
}

fn normalize_models_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/models") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/models")
    }
}

fn normalize_chat_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn supported_ratios() -> &'static [(&'static str, i32, i32)] {
    &[
        ("1:1", 1, 1),
        ("3:2", 3, 2),
        ("2:3", 2, 3),
        ("4:3", 4, 3),
        ("3:4", 3, 4),
        ("5:4", 5, 4),
        ("4:5", 4, 5),
        ("16:9", 16, 9),
        ("9:16", 9, 16),
        ("2:1", 2, 1),
        ("1:2", 1, 2),
        ("21:9", 21, 9),
        ("9:21", 9, 21),
    ]
}

fn supported_ratios_for_category(category: &str) -> &'static [(&'static str, i32, i32)] {
    if category == "action-sequence" {
        &ACTION_SEQUENCE_RATIOS
    } else {
        supported_ratios()
    }
}

fn action_sequence_ratio_allowed(ratio: &str) -> bool {
    ACTION_SEQUENCE_RATIOS
        .iter()
        .any(|(label, _, _)| *label == ratio)
}

fn max_reference_images_for_category(category: &str) -> usize {
    if category == "action-sequence" {
        1
    } else {
        MAX_REFERENCE_IMAGES
    }
}

fn reference_limit_message(max_references: usize) -> &'static str {
    if max_references == 1 {
        "动作序列只能上传 1 张参考图"
    } else {
        "最多上传 8 张参考图"
    }
}

fn normalize_creation_mode_for_category(category: &str, creation: &str) -> String {
    if category != "action-sequence" {
        return creation.to_string();
    }
    match creation {
        "anim-idle" | "anim-run" | "anim-walk" | "anim-attack" | "anim-death" => {
            creation.to_string()
        }
        _ => "anim-idle".to_string(),
    }
}

fn normalized_quality(quality: &str) -> &'static str {
    match quality.trim().to_ascii_uppercase().as_str() {
        "4K" => "4K",
        "2K" => "2K",
        _ => "1K",
    }
}

fn size_for(ratio: &str, quality: &str) -> String {
    let (width, height) = pixel_dimensions_for(ratio, quality);
    format!("{width}x{height}")
}

fn pixel_dimensions_for(ratio: &str, quality: &str) -> (i32, i32) {
    let max_edge = match normalized_quality(quality) {
        "4K" => 4096,
        "2K" => 2048,
        _ => 1024,
    };
    let (w, h) = ratio_dimensions(ratio);
    if w <= 0 || h <= 0 {
        return (max_edge, max_edge);
    }
    if w >= h {
        (
            max_edge,
            round_dimension(max_edge as f64 * h as f64 / w as f64),
        )
    } else {
        (
            round_dimension(max_edge as f64 * w as f64 / h as f64),
            max_edge,
        )
    }
}

fn round_dimension(value: f64) -> i32 {
    (((value.max(64.0) / 8.0).round() as i32) * 8).max(64)
}

fn image_generation_credit_cost(quality: &str) -> i32 {
    match quality.trim().to_ascii_uppercase().as_str() {
        "4K" => 300,
        "2K" => 120,
        _ => 50,
    }
}

fn ratio_dimensions(ratio: &str) -> (i32, i32) {
    supported_ratios()
        .iter()
        .find(|(label, _, _)| *label == ratio)
        .map(|(_, w, h)| (*w, *h))
        .unwrap_or((1, 1))
}

fn ratio_from_actual_dimensions(width: i32, height: i32) -> String {
    if width <= 0 || height <= 0 {
        return "1:1".to_string();
    }
    let actual = width as f64 / height as f64;
    supported_ratios()
        .iter()
        .min_by(|left, right| {
            let left_ratio = left.1 as f64 / left.2 as f64;
            let right_ratio = right.1 as f64 / right.2 as f64;
            (actual - left_ratio)
                .abs()
                .partial_cmp(&(actual - right_ratio).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(label, _, _)| (*label).to_string())
        .unwrap_or_else(|| "1:1".to_string())
}

fn quality_from_actual_dimensions(width: i32, height: i32) -> String {
    let longest = width.max(height);
    if longest > 2048 {
        "4K".to_string()
    } else if longest > 1024 {
        "2K".to_string()
    } else {
        "1K".to_string()
    }
}

fn refresh_advanced_prompt_preview(app: &AppWindow) {
    let state = app.global::<AppState>();
    let language = if state.get_language().as_str() == "en" {
        PromptLanguage::English
    } else {
        PromptLanguage::Chinese
    };
    let controls = PromptControls {
        category: resolve_category(&state.get_asset_type().to_string(), ""),
        creation: state.get_creation_mode().to_string(),
        style: state.get_style_mode().to_string(),
        view: state.get_view_mode().to_string(),
        weather: state.get_weather_mode().to_string(),
        time: state.get_time_mode().to_string(),
        light: state.get_light_mode().to_string(),
    };
    let text = advanced_prompt_preview_text(&controls, language);
    state.set_advanced_prompt_preview(text.into());
}

fn resolve_category(selected: &str, _prompt: &str) -> String {
    match selected {
        "character" | "scene" | "ui" | "effect" => selected.to_string(),
        _ => "character".to_string(),
    }
}

fn resolve_ratio_for_category(
    category: &str,
    selected: &str,
    prompt: &str,
    quoted: &str,
) -> String {
    let ratios = supported_ratios_for_category(category);
    if selected != "smart" {
        return ratios
            .iter()
            .find(|(label, _, _)| *label == selected)
            .map(|(label, _, _)| (*label).to_string())
            .unwrap_or_else(|| "1:1".to_string());
    }
    let text = prompt.to_lowercase();
    for (ratio, _, _) in ratios {
        if text.contains(*ratio) {
            return (*ratio).to_string();
        }
    }
    if ratios.iter().any(|(ratio, _, _)| *ratio == quoted) {
        return quoted.to_string();
    }
    "1:1".to_string()
}

fn optimization_system_prompt(language: PromptLanguage, visual_mode: bool) -> &'static str {
    if visual_mode {
        return match language {
            PromptLanguage::Chinese => {
                "你是 ArtForgeStudio 的图文提示词优化助手。请结合用户文字和上传参考图，分析参考图的主体、构图、比例、色彩、材质、光影、风格、关键细节与可复用视觉元素，再改写成适合图像生成模型的高质量中文提示词。只输出最终提示词，不要解释过程，不要输出英文或中英混排。"
            }
            PromptLanguage::English => {
                "You are the image-and-text prompt optimization assistant for ArtForgeStudio. Analyze the uploaded reference image(s) for subject, composition, aspect, palette, material, lighting, style, details, and reusable visual elements, then combine that visual analysis with the user's text into a high-quality image-generation prompt. Output in English only. Return only the final prompt."
            }
        };
    }
    match language {
        PromptLanguage::Chinese => {
            "你是 ArtForgeStudio 的提示词优化助手。请把用户需求优化为适合图像生成模型的高质量提示词。必须只使用中文输出，不要夹杂英文单词、英文短语或双语解释。只输出最终提示词，不要展示思考过程。"
        }
        PromptLanguage::English => {
            "You are the prompt optimization assistant for ArtForgeStudio. Rewrite the user request into a high-quality prompt for an image generation model. Output in English only. Do not include Chinese words, bilingual notes, explanations, or reasoning. Return only the final prompt."
        }
    }
}

fn optimization_user_prompt(
    prompt: &str,
    category: &str,
    ratio: &str,
    quality: &str,
    quote: &QuoteContext,
    reference_count: usize,
    translate_prompt: bool,
    language: PromptLanguage,
    visual_mode: bool,
) -> String {
    if visual_mode {
        return match language {
            PromptLanguage::Chinese => {
                format!(
                    "输出语言：中文。用户文字需求：{prompt}\n分类：{category}\n比例：{ratio}\n清晰度：{quality}\n上传参考图数量：{reference_count}\n引用图片标题：{}\n引用图片提示词：{}\n引用图片比例：{}\n引用图片清晰度：{}\n引用图片尺寸：{} x {}\n请先理解参考图的主体、轮廓、构图、镜头、色彩、材质、光影、风格、细节和可复用视觉元素，再结合用户文字需求输出最终图像生成提示词。只输出提示词，不要解释。",
                    quote.title,
                    quote.prompt,
                    quote.ratio,
                    quote.quality,
                    quote.width,
                    quote.height
                )
            }
            PromptLanguage::English => {
                let translation_note = if translate_prompt {
                    "Translate the user's request into natural English before optimizing it."
                } else {
                    "Keep the request in English and optimize it."
                };
                format!(
                    "Output language: English only.\n{translation_note}\nUser text request: {prompt}\nCategory: {category}\nAspect ratio: {ratio}\nResolution: {quality}\nUploaded reference image count: {reference_count}\nQuoted image title: {}\nQuoted image prompt: {}\nQuoted image ratio: {}\nQuoted image resolution: {}\nQuoted image size: {} x {}\nAnalyze the reference image(s) for subject, silhouette, composition, camera, palette, material, lighting, style, details, and reusable visual elements. Combine that visual analysis with the user text and return only the final image-generation prompt.",
                    quote.title,
                    quote.prompt,
                    quote.ratio,
                    quote.quality,
                    quote.width,
                    quote.height
                )
            }
        };
    }
    match language {
        PromptLanguage::Chinese => {
            let reference_note = if reference_count == 0 {
                "无".to_string()
            } else {
                format!("用户上传了 {reference_count} 张参考图。请理解参考图的主体、构图、色彩、风格和关键元素，并把这些视觉信息写进最终提示词作为参考。")
            };
            format!(
                "输出语言：中文。禁止输出英文或中英混排。\n用户需求：{prompt}\n分类：{category}\n比例：{ratio}\n清晰度：{quality}\n上传参考图：{reference_note}\n引用图片标题：{}\n引用图片提示词：{}\n引用图片比例：{}\n引用图片清晰度：{}\n引用图片尺寸：{} x {}\n如果引用图片信息不为空，请把用户需求理解为对引用图片的修改；如果有上传参考图，请把参考图作为视觉参考。只输出优化后的图片生成提示词。",
                quote.title,
                quote.prompt,
                quote.ratio,
                quote.quality,
                quote.width,
                quote.height
            )
        }
        PromptLanguage::English => {
            let reference_note = if reference_count == 0 {
                "None".to_string()
            } else {
                format!("The user uploaded {reference_count} reference image(s). Understand the subject, composition, colors, style, and key visual elements, then include those visual details in the final prompt as reference guidance.")
            };
            let translation_note = if translate_prompt {
                "Translate the user's request into natural English before optimizing it."
            } else {
                "Keep the request in English and optimize it."
            };
            format!(
                "Output language: English only. Do not output Chinese or bilingual text.\n{translation_note}\nUser request: {prompt}\nCategory: {category}\nAspect ratio: {ratio}\nResolution: {quality}\nUploaded reference images: {reference_note}\nQuoted image title: {}\nQuoted image prompt: {}\nQuoted image ratio: {}\nQuoted image resolution: {}\nQuoted image size: {} x {}\nIf quoted image information is present, treat the user request as an edit to that quoted image. If reference images are provided, use them as visual references. Return only the optimized image-generation prompt.",
                quote.title,
                quote.prompt,
                quote.ratio,
                quote.quality,
                quote.width,
                quote.height
            )
        }
    }
}

fn control_label(kind: &str, value: &str, language: PromptLanguage) -> &'static str {
    if language == PromptLanguage::Chinese {
        return match (kind, value) {
            ("creation", "character-standee") => "角色立绘",
            ("creation", "character-turnaround") => "角色三视图设定",
            ("creation", "character-8dir") => "角色 8 方向动作",
            ("creation", "character-spritesheet") => "角色 SpriteSheet 序列帧",
            ("creation", "character-spine-parts") => "Spine 角色拆件",
            ("creation", "character-portrait") => "NPC 头像",
            ("creation", "character-poster") => "角色宣传海报",
            ("creation", "scene-concept") => "场景概念设计",
            ("creation", "tileset") => "游戏地图块素材",
            ("creation", "map-ref") => "关卡地图参考",
            ("creation", "poster") => "宣传主视觉海报",
            ("creation", "loading") => "游戏加载页插画",
            ("creation", "minimap") => "俯视小地图",
            ("creation", "building-kit") => "模块化建筑套件",
            ("creation", "ui-hud") => "HUD 战斗界面",
            ("creation", "ui-main-screen") => "游戏主界面",
            ("creation", "ui-backpack") => "背包物品界面",
            ("creation", "ui-shop") => "商城购买界面",
            ("creation", "ui-icon") => "UI 图标",
            ("creation", "ui-loading") => "Loading 载入界面",
            ("creation", "ui-popup") => "弹窗模态界面",
            ("creation", "fx-skill") => "技能特效",
            ("creation", "fx-buff") => "Buff 状态特效",
            ("creation", "fx-explosion") => "爆炸冲击特效",
            ("creation", "fx-scene") => "场景环境特效",
            ("creation", "fx-ui") => "UI 反馈特效",
            ("creation", "fx-weapon-trail") => "武器拖尾轨迹",
            ("creation", "anim-run") => "跑步循环动画",
            ("creation", "anim-walk") => "走路动作",
            ("creation", "anim-attack") => "攻击动作",
            ("creation", "anim-hit") => "受击动作",
            ("creation", "anim-idle") => "待机循环动画",
            ("creation", "anim-jump") => "跳跃动作",
            ("creation", "anim-death") => "死亡动作",
            ("creation", "anim-skill") => "技能动作",
            ("creation", _) => "自由创作",
            ("style", "warm") => "温暖治愈风格",
            ("style", "cold") => "冷系压迫风格",
            ("style", "vivid") => "高饱和鲜艳色彩",
            ("style", "soft") => "低饱和柔和色彩",
            ("style", "dark") => "黑暗奇幻风格",
            ("style", "cyber") => "赛博朋克霓虹风格",
            ("style", "fantasy") => "日式幻想风格",
            ("style", "ghibli") => "绘本动画风格",
            ("style", _) => "自由风格",
            ("view", "top-down") => "俯视视角",
            ("view", "2.5d") => "2.5D 斜视角",
            ("view", "isometric") => "等距视角",
            ("view", "side-view") => "侧视视角",
            ("view", "third-person") => "第三人称视角",
            ("view", "first-person") => "第一人称视角",
            ("view", "orthographic") => "正交视角",
            ("view", _) => "自由视角",
            ("weather", "sunny") => "晴天",
            ("weather", "cloudy") => "阴天",
            ("weather", "rainy") => "雨天",
            ("weather", "storm") => "暴风雨天气",
            ("weather", "snow") => "雪天",
            ("weather", "fog") => "雾天",
            ("weather", "dust") => "沙尘氛围",
            ("weather", _) => "自然天气",
            ("time", "morning") => "清晨",
            ("time", "noon") => "正午日光",
            ("time", "dusk") => "黄昏金色时刻",
            ("time", "blue-hour") => "蓝调时刻",
            ("time", "night") => "深夜",
            ("time", _) => "自然时间",
            ("light", "soft") => "柔和自然光",
            ("light", "cinematic") => "电影感光照",
            ("light", "glow") => "梦幻发光",
            ("light", "contrast") => "高对比光照",
            ("light", "volumetric") => "体积光束",
            ("light", "neon") => "霓虹光照",
            ("light", _) => "自然光照",
            _ => "",
        };
    }

    match (kind, value) {
        ("creation", "character-standee") => "character full-body standing illustration",
        ("creation", "character-turnaround") => "character three-view turnaround sheet",
        ("creation", "character-8dir") => "character 8-direction action set",
        ("creation", "character-spritesheet") => "character SpriteSheet animation frames",
        ("creation", "character-spine-parts") => "character Spine separated parts",
        ("creation", "character-portrait") => "NPC character portrait",
        ("creation", "character-poster") => "character promotional poster",
        ("creation", "scene-concept") => "scene concept art",
        ("creation", "tileset") => "TileSet game tiles",
        ("creation", "map-ref") => "level design map reference",
        ("creation", "poster") => "key visual promotional artwork",
        ("creation", "loading") => "game loading screen artwork",
        ("creation", "minimap") => "mini map top-down game map",
        ("creation", "building-kit") => "modular building kit",
        ("creation", "ui-hud") => "game HUD battle interface",
        ("creation", "ui-main-screen") => "game main screen entry interface",
        ("creation", "ui-backpack") => "backpack item interface",
        ("creation", "ui-shop") => "shop purchase interface",
        ("creation", "ui-icon") => "UI icon",
        ("creation", "ui-loading") => "loading screen UI",
        ("creation", "ui-popup") => "popup modal interface",
        ("creation", "fx-skill") => "skill visual effect",
        ("creation", "fx-buff") => "buff status visual effect",
        ("creation", "fx-explosion") => "explosion impact visual effect",
        ("creation", "fx-scene") => "scene environmental visual effect",
        ("creation", "fx-ui") => "UI feedback visual effect",
        ("creation", "fx-weapon-trail") => "weapon trail visual effect",
        ("creation", "anim-run") => "run cycle animation",
        ("creation", "anim-walk") => "walk animation",
        ("creation", "anim-attack") => "attack animation",
        ("creation", "anim-hit") => "hit reaction animation",
        ("creation", "anim-idle") => "idle loop animation",
        ("creation", "anim-jump") => "jump animation",
        ("creation", "anim-death") => "death animation",
        ("creation", "anim-skill") => "skill action animation",
        ("creation", _) => "free creation",
        ("style", "warm") => "warm healing style",
        ("style", "cold") => "cold oppressive style",
        ("style", "vivid") => "high saturation vivid color",
        ("style", "soft") => "low saturation soft color",
        ("style", "dark") => "dark fantasy style",
        ("style", "cyber") => "cyberpunk neon style",
        ("style", "fantasy") => "Japanese fantasy style",
        ("style", "ghibli") => "storybook animation style",
        ("style", _) => "free style",
        ("view", "top-down") => "top-down camera view",
        ("view", "2.5d") => "2.5D angled view",
        ("view", "isometric") => "isometric view",
        ("view", "side-view") => "side view",
        ("view", "third-person") => "third person view",
        ("view", "first-person") => "first person view",
        ("view", "orthographic") => "orthographic view",
        ("view", _) => "free camera view",
        ("weather", "sunny") => "sunny weather",
        ("weather", "cloudy") => "cloudy weather",
        ("weather", "rainy") => "rainy weather",
        ("weather", "storm") => "storm weather",
        ("weather", "snow") => "snowy weather",
        ("weather", "fog") => "foggy weather",
        ("weather", "dust") => "dust storm atmosphere",
        ("weather", _) => "natural weather",
        ("time", "morning") => "morning time",
        ("time", "noon") => "noon daylight",
        ("time", "dusk") => "dusk golden hour",
        ("time", "blue-hour") => "blue hour",
        ("time", "night") => "deep night",
        ("time", _) => "natural time of day",
        ("light", "soft") => "soft natural lighting",
        ("light", "cinematic") => "cinematic lighting",
        ("light", "glow") => "dreamy glowing light",
        ("light", "contrast") => "high contrast lighting",
        ("light", "volumetric") => "volumetric light beams",
        ("light", "neon") => "neon lighting",
        ("light", _) => "natural lighting",
        _ => "",
    }
}

fn visible_prompt_control_entries<'a>(
    controls: &'a PromptControls,
) -> Vec<(&'static str, &'a str)> {
    if controls.category == "action-sequence" {
        return vec![("creation", controls.creation.as_str())];
    }
    let mut entries = vec![
        ("creation", controls.creation.as_str()),
        ("style", controls.style.as_str()),
    ];
    if controls.category == "scene" || controls.category == "character" {
        entries.push(("view", controls.view.as_str()));
    }
    if controls.category == "scene" {
        entries.push(("weather", controls.weather.as_str()));
        entries.push(("time", controls.time.as_str()));
    }
    entries.push(("light", controls.light.as_str()));
    entries
}

fn prompt_controls_text(controls: &PromptControls, language: PromptLanguage) -> String {
    visible_prompt_control_entries(controls)
        .iter()
        .map(|(kind, value)| control_label(kind, value, language))
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn advanced_prompt_preview_text(controls: &PromptControls, language: PromptLanguage) -> String {
    visible_prompt_control_entries(controls)
        .iter()
        .map(|(kind, value)| {
            let name = match (*kind, language) {
                ("creation", PromptLanguage::Chinese) => "创作方式",
                ("creation", PromptLanguage::English) => "Creation",
                ("style", PromptLanguage::Chinese) => "风格",
                ("style", PromptLanguage::English) => "Style",
                ("view", PromptLanguage::Chinese) => "镜头/视角",
                ("view", PromptLanguage::English) => "Camera/view",
                ("weather", PromptLanguage::Chinese) => "天气",
                ("weather", PromptLanguage::English) => "Weather",
                ("time", PromptLanguage::Chinese) => "时间",
                ("time", PromptLanguage::English) => "Time of day",
                ("light", PromptLanguage::Chinese) => "光照",
                ("light", PromptLanguage::English) => "Lighting",
                _ => "",
            };
            format!("{name}: {}", control_label(kind, value, language))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn prompt_with_controls(
    prompt: &str,
    controls: &PromptControls,
    language: PromptLanguage,
) -> String {
    let controls_text = prompt_controls_text(controls, language);
    if controls_text.is_empty() {
        prompt.to_string()
    } else if language == PromptLanguage::Chinese {
        format!("{prompt}\n\n生成控制：{controls_text}")
    } else {
        format!("{prompt}\n\nGeneration controls: {controls_text}")
    }
}

fn append_action_sequence_instruction(prompt: &str, language: PromptLanguage) -> String {
    if language == PromptLanguage::Chinese {
        format!(
            "{prompt}\n\n动作序列规则：如果上传了参考图，请以参考图中的角色或主体为基础，生成所选动作类型对应的动作资源，保持角色外观、服装、配色和识别特征一致。动作类型只从待机、跑步、走路、攻击、死亡中选择，不要生成无关动作。"
        )
    } else {
        format!(
            "{prompt}\n\nAction sequence rule: If a reference image is uploaded, use the character or subject in the reference image as the basis for the selected action asset. Keep the character appearance, outfit, colors, and identifying traits consistent. The action type must be one of idle, run, walk, attack, or death; do not generate unrelated actions."
        )
    }
}

fn build_generation_prompt(
    prompt: &str,
    controls: &PromptControls,
    quote: &QuoteContext,
    category: &str,
    ratio: &str,
    quality: &str,
    language: PromptLanguage,
) -> String {
    let mut final_prompt = prompt_with_controls(prompt, controls, language);
    if !quote.title.trim().is_empty()
        || !quote.prompt.trim().is_empty()
        || !quote.ratio.trim().is_empty()
        || !quote.quality.trim().is_empty()
        || quote.width > 0
        || quote.height > 0
    {
        if language == PromptLanguage::Chinese {
            final_prompt.push_str(&format!(
                "\n\n参考图片信息：标题：{}；提示词：{}；宽高比：{}；清晰度：{}；尺寸：{} x {}。请把用户需求理解为对参考图片的修改或延续。",
                quote.title,
                quote.prompt,
                quote.ratio,
                quote.quality,
                quote.width,
                quote.height
            ));
        } else {
            final_prompt.push_str(&format!(
                "\n\nReference image information: title: {}; prompt: {}; aspect ratio: {}; resolution: {}; size: {} x {}. Treat the user request as an edit or continuation of the reference image.",
                quote.title,
                quote.prompt,
                quote.ratio,
                quote.quality,
                quote.width,
                quote.height
            ));
        }
    }
    if category == "action-sequence" {
        final_prompt = append_action_sequence_instruction(&final_prompt, language);
    }
    append_parameter_priority_instruction(&final_prompt, category, ratio, quality, language)
}

fn append_parameter_priority_instruction(
    prompt: &str,
    category: &str,
    ratio: &str,
    quality: &str,
    language: PromptLanguage,
) -> String {
    if language == PromptLanguage::Chinese {
        format!(
            "{prompt}\n\n参数优先规则：左侧工作台分类和下方已选择的卡片为最终参数，并覆盖用户提示词中冲突的描述。最终分类：{category}。最终宽高比：{ratio}。最终清晰度：{quality}。应用会按所选张数调用生图模型。除非用户明确要求拼图、网格、分屏或多画面构图，否则不要在一张画布里生成多张图。"
        )
    } else {
        format!(
            "{prompt}\n\nParameter priority rule: the left workspace category and selected cards below are final and override any conflicting words in the user's prompt. Final category: {category}. Final aspect ratio: {ratio}. Final quality: {quality}. The application requests the selected image count from the image model. Do not create grids, collages, contact sheets, split panels, or multiple images inside one canvas unless the user explicitly asks for that composition."
        )
    }
}

fn display_generation_prompt(prompt: &str) -> String {
    let normalized = prompt.replace("\r\n", "\n");
    let hidden_prefixes = [
        "生成控制：",
        "参数优先规则：",
        "动作序列规则：",
        "Generation controls:",
        "Parameter priority rule:",
        "Action sequence rule:",
    ];
    normalized
        .split("\n\n")
        .filter(|part| {
            let trimmed = part.trim_start();
            !hidden_prefixes
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string()
}

fn short_text(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn mask_phone(phone: &str) -> String {
    if phone.len() >= 11 {
        format!("{}****{}", &phone[..3], &phone[7..])
    } else {
        phone.to_string()
    }
}

fn load_system_fonts() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut names = BTreeSet::new();
    for face in db.faces() {
        for (family, _) in &face.families {
            let name = family.trim();
            if !name.is_empty() {
                names.insert(name.to_string());
            }
        }
    }
    for fallback in [
        "Microsoft YaHei UI",
        "Microsoft YaHei",
        "SimSun",
        "SimHei",
        "DengXian",
        "Segoe UI",
        "Arial",
    ] {
        names.insert(fallback.to_string());
    }
    names.into_iter().collect()
}

fn push_provider_model_options(app: &AppWindow) {
    let state = app.global::<AppState>();
    let used = state
        .get_provider_used_models()
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>();
    let models = state
        .get_fetched_models()
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>();
    let used = normalized_used_models(used, &models)
        .into_iter()
        .collect::<BTreeSet<_>>();
    state.set_provider_model_options(ModelRc::new(VecModel::from(
        models
            .into_iter()
            .map(|model| ProviderModelOption {
                used: used.contains(&model),
                name: model.into(),
            })
            .collect::<Vec<_>>(),
    )));
}

fn normalized_used_models(used: Vec<String>, models: &[String]) -> Vec<String> {
    let available = models.iter().cloned().collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    used.into_iter()
        .filter(|model| available.contains(model) && seen.insert(model.clone()))
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelRole {
    Image,
    Reasoning,
    None,
}

fn apply_model_role(role: &str, model: &str) -> ModelRole {
    if model.trim().is_empty() {
        return ModelRole::None;
    }
    match role {
        "image" => ModelRole::Image,
        "reasoning" => ModelRole::Reasoning,
        _ => ModelRole::None,
    }
}

fn model_belongs_to_provider(provider: &ProviderData, model: &str) -> bool {
    !model.trim().is_empty() && provider.models.iter().any(|item| item == model)
}

fn next_pending_models(
    role: &str,
    model: &str,
    current_image: &str,
    current_reasoning: &str,
) -> (String, String) {
    let mut image = current_image.to_string();
    let mut reasoning = current_reasoning.to_string();
    match apply_model_role(role, model) {
        ModelRole::Image => {
            image = model.to_string();
            if reasoning == image {
                reasoning.clear();
            }
        }
        ModelRole::Reasoning => {
            reasoning = model.to_string();
            if image == reasoning {
                image.clear();
            }
        }
        ModelRole::None => {}
    }
    (image, reasoning)
}

fn zh_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("504")
        || lower.contains("gateway time-out")
        || lower.contains("gateway timeout")
    {
        return "生成请求超时，后端可能仍在生成。".to_string();
    }
    if lower.contains("timeout") || raw.contains("超时") {
        "请求超时，请检查网络环境或服务商接口状态后重试。".to_string()
    } else if lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("network")
    {
        "网络连接失败，请检查网络环境、代理或服务商接口地址。".to_string()
    } else if lower.contains("unauthorized")
        || lower.contains("invalid api key")
        || lower.contains("401")
    {
        "API Key 无效或权限不足，请检查模型配置。".to_string()
    } else if lower.contains("forbidden") || lower.contains("permission") || lower.contains("403") {
        "当前账号没有调用该模型的权限，请检查 API Key 权限或更换模型。".to_string()
    } else if lower.contains("not found") || lower.contains("404") {
        "模型或接口地址不存在，请检查模型名称和 API 请求地址。".to_string()
    } else if lower.contains("rate") || lower.contains("429") {
        "请求过于频繁或额度不足，请稍后重试。".to_string()
    } else if lower.contains("quota") || lower.contains("balance") || lower.contains("billing") {
        "账号额度不足或计费状态异常，请检查服务商账户。".to_string()
    } else if lower.contains("size") || lower.contains("resolution") {
        "当前模型不支持所选尺寸，请更换比例或分辨率。".to_string()
    } else if lower.contains("model")
        && (lower.contains("unsupported") || lower.contains("not support"))
    {
        "当前模型不支持这类请求，请确认已选择生图模型或更换模型。".to_string()
    } else if lower.contains("json") || lower.contains("parse") || lower.contains("deserialize") {
        "接口返回内容格式异常，请检查 API 请求地址和模型类型是否正确。".to_string()
    } else if lower.contains("no prompt")
        || lower.contains("returned no prompt")
        || lower.contains("empty")
    {
        "推理模型没有返回可用提示词，请确认选择的是支持文本输出的推理模型，并检查 API 返回内容。"
            .to_string()
    } else if lower.contains("image") {
        "图片生成失败，请检查 API 配置、模型能力或稍后重试。".to_string()
    } else if raw.trim().is_empty() {
        "接口返回错误，请检查 API 配置、模型能力或稍后重试。".to_string()
    } else {
        "接口返回错误，请检查 API 配置、模型能力或稍后重试。".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_role_does_not_become_reasoning_role() {
        assert_eq!(apply_model_role("image", "gpt-image-2"), ModelRole::Image);
        assert_ne!(
            apply_model_role("image", "gpt-image-2"),
            ModelRole::Reasoning
        );
    }

    #[test]
    fn reasoning_role_does_not_become_image_role() {
        assert_eq!(
            apply_model_role("reasoning", "deepseek-chat"),
            ModelRole::Reasoning
        );
        assert_ne!(
            apply_model_role("reasoning", "deepseek-chat"),
            ModelRole::Image
        );
    }

    #[test]
    fn empty_model_has_no_role() {
        assert_eq!(apply_model_role("image", ""), ModelRole::None);
    }

    #[test]
    fn model_role_selection_is_exclusive() {
        let (image, reasoning) = next_pending_models("image", "gpt-image-2", "", "gpt-image-2");
        assert_eq!(image, "gpt-image-2");
        assert_eq!(reasoning, "");

        let (image, reasoning) =
            next_pending_models("reasoning", "deepseek-chat", "deepseek-chat", "");
        assert_eq!(image, "");
        assert_eq!(reasoning, "deepseek-chat");
    }

    #[test]
    fn pending_task_url_is_poll_url() {
        let value = json!({
            "status": "processing",
            "url": "/v1/image-tasks/task-123"
        });

        assert!(is_pending_task_status(&value));
        assert_eq!(
            extract_poll_url(&value, "https://api.example.com/v1/images/generations").as_deref(),
            Some("https://api.example.com/v1/image-tasks/task-123")
        );
    }

    #[test]
    fn camel_case_image_url_keys_are_image_results() {
        assert!(is_image_result_key("imageurl"));
        assert!(is_image_result_key("outputurl"));
        assert!(is_image_result_key("downloadurl"));
        assert!(is_image_result_key("publicurl"));
    }

    #[test]
    fn requested_images_are_sent_one_by_one() {
        assert_eq!(image_request_batches_for_count(1), vec![1]);
        assert_eq!(image_request_batches_for_count(2), vec![1, 1]);
        assert_eq!(image_request_batches_for_count(4), vec![1, 1, 1, 1]);
        assert_eq!(image_request_batches_for_count(8), vec![1, 1, 1, 1]);
    }

    #[test]
    fn gpt_image_2_request_carries_selected_quality() {
        let body = build_image_request_body(
            "gpt-image-2",
            "test prompt",
            1,
            "16:9",
            "4K",
            &[],
            "https://api.getapi.pro/v1/images/generations",
        );

        assert_eq!(body["resolution"], "4K");
        assert_eq!(body["quality"], "4K");
        assert_eq!(body["image_size"], "4K");
        assert_eq!(body["size"], "4096x2304");
        assert_eq!(body["pixel_size"], "4096x2304");
        assert_eq!(body["width"], 4096);
        assert_eq!(body["height"], 2304);
    }

    #[test]
    fn quality_pixel_size_uses_longest_edge_limits() {
        assert_eq!(pixel_dimensions_for("9:16", "1K"), (576, 1024));
        assert_eq!(pixel_dimensions_for("16:9", "1K"), (1024, 576));
        assert_eq!(pixel_dimensions_for("9:16", "2K"), (1152, 2048));
        assert_eq!(pixel_dimensions_for("16:9", "4K"), (4096, 2304));

        assert_eq!(quality_from_actual_dimensions(1023, 1537), "2K");
        assert_eq!(quality_from_actual_dimensions(1024, 1024), "1K");
        assert_eq!(quality_from_actual_dimensions(2048, 1152), "2K");
        assert_eq!(quality_from_actual_dimensions(4096, 2304), "4K");
    }

    #[test]
    fn generated_images_are_clamped_to_selected_quality() {
        let source = image::RgbaImage::from_pixel(1254, 1254, image::Rgba([40, 80, 120, 255]));
        let bytes = encode_png_rgba(&source, 1254, 1254).unwrap();
        let (_, _, width, height) = generated_image_from_bytes(&bytes, "1K").unwrap();

        assert_eq!((width, height), (1024, 1024));
    }

    #[test]
    fn image_candidates_include_file_content_urls() {
        let value = json!({
            "data": [
                { "object": "file", "id": "file_abc123" },
                { "file_id": "file_def456" }
            ]
        });
        let images =
            extract_image_candidates(&value, "https://api.example.com/v1/images/generations");

        assert!(
            images.contains(&"https://api.example.com/v1/files/file_abc123/content".to_string())
        );
        assert!(
            images.contains(&"https://api.example.com/v1/files/file_def456/content".to_string())
        );
    }
}
