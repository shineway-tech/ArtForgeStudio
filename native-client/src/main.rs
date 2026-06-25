#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use chrono::Local;
use serde_json::{json, Value};
use slint::{Image, Model, ModelRc, SharedString, VecModel, Weak};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use uuid::Uuid;

slint::include_modules!();

const TEST_CODE: &str = "123456";

#[derive(Clone)]
struct ProviderData {
    id: String,
    name: String,
    endpoint: String,
    remark: String,
    website: String,
    api_key: String,
    models: Vec<String>,
    selected_model: String,
}

#[derive(Clone)]
struct AssetData {
    id: String,
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
}

#[derive(Clone)]
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

#[derive(Default)]
struct Store {
    providers: Vec<ProviderData>,
    generations: Vec<AssetData>,
    assets: Vec<AssetData>,
    inspiration: Vec<AssetData>,
    notifications: Vec<NotificationData>,
    references: Vec<ReferenceData>,
}

fn main() -> Result<()> {
    std::env::set_var("SLINT_BACKEND", "winit-software");
    std::env::set_var("SLINT_SCALE_FACTOR", "1");
    let app = AppWindow::new()?;
    app.window().set_size(slint::PhysicalSize::new(1440, 900));
    apply_sprite_theme(&app);
    init_portable_dirs(&app)?;
    load_showcase_images(&app);

    let store = Rc::new(RefCell::new(Store::default()));
    seed_inspiration(&app, &store)?;
    push_all(&app, &store.borrow());

    wire_callbacks(&app, store.clone());
    app.run()?;
    Ok(())
}

fn wire_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        state.on_use_now(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            if state.get_logged_in() {
                navigate_to(&app, "workspace");
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
            let Some(app) = app_weak.upgrade() else { return; };
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
            state.set_auth_open(false);
            state.set_auth_error("".into());
            navigate_to(&app, "workspace");
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_logout(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_logged_in(false);
                state.set_page("welcome".into());
                state.set_theme_id("sprite".into());
                apply_sprite_theme(&app);
                state.set_profile_open(false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_save_profile(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let name = state.get_profile_name().trim().to_string();
                state.set_nickname(if name.is_empty() { state.get_phone_mask() } else { name.into() });
                state.set_profile_open(false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_navigate(move |page| {
            if let Some(app) = app_weak.upgrade() {
                navigate_to(&app, &page);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_back(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let page = state.get_page().to_string();
                if page == "workspace" {
                    return;
                }
                state.set_page("workspace".into());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_theme(move |theme| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_theme_id(theme.clone());
                if theme == "sprite" {
                    apply_sprite_theme(&app);
                } else {
                    apply_dark_theme(&app);
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_set_language(move |lang| {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().set_language(lang);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_pick_dir(move |kind| {
            let Some(app) = app_weak.upgrade() else { return; };
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
                let fonts = vec![
                    SharedString::from("Microsoft YaHei UI"),
                    SharedString::from("Microsoft YaHei"),
                    SharedString::from("SimSun"),
                    SharedString::from("SimHei"),
                    SharedString::from("DengXian"),
                    SharedString::from("Segoe UI"),
                    SharedString::from("Arial"),
                ];
                app.global::<AppState>().set_font_list(ModelRc::new(VecModel::from(fonts)));
            }
        });
    }

    wire_provider_callbacks(app, store.clone());
    wire_reference_callbacks(app, store.clone());
    wire_generation_callbacks(app, store.clone());
    wire_viewer_callbacks(app, store.clone());
    wire_notification_callbacks(app, store);
}

fn wire_provider_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

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
                app.global::<AppState>().set_provider_editor_open(false);
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
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            let name = state.get_provider_name().trim().to_string();
            if name.is_empty() {
                state.set_provider_message("供应商名称不能为空".into());
                return;
            }
            let mut models = state
                .get_fetched_models()
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>();
            let selected = state.get_provider_model_name().trim().to_string();
            if !selected.is_empty() && !models.iter().any(|m| m == &selected) {
                models.push(selected.clone());
            }
            let mut store = store.borrow_mut();
            let id = state.get_edit_provider_id().to_string();
            let provider = ProviderData {
                id: if id.is_empty() { Uuid::new_v4().to_string() } else { id.clone() },
                name,
                endpoint: state.get_provider_endpoint().trim().to_string(),
                remark: state.get_provider_remark().trim().to_string(),
                website: state.get_provider_website().trim().to_string(),
                api_key: state.get_provider_api_key().trim().to_string(),
                selected_model: selected,
                models,
            };
            if let Some(existing) = store.providers.iter_mut().find(|p| p.id == provider.id) {
                *existing = provider;
            } else {
                store.providers.push(provider);
            }
            state.set_provider_editor_open(false);
            push_all(&app, &store);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_fetch_models(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            let endpoint = state.get_provider_endpoint().to_string();
            let key = state.get_provider_api_key().to_string();
            if endpoint.trim().is_empty() || key.trim().is_empty() {
                state.set_provider_message("请先填写 API Key 和 API 请求地址".into());
                return;
            }
            match list_models(&endpoint, &key) {
                Ok(models) => {
                    let selected = models.first().cloned().unwrap_or_default();
                    let shared = models.into_iter().map(SharedString::from).collect::<Vec<_>>();
                    state.set_fetched_models(ModelRc::new(VecModel::from(shared)));
                    if !selected.is_empty() {
                        state.set_provider_model_name(selected.into());
                    }
                    state.set_provider_message("获取模型完成".into());
                }
                Err(err) => state.set_provider_message(format!("获取模型失败：{}", zh_error(&err.to_string())).into()),
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_test_provider(move |id| {
            let Some(app) = app_weak.upgrade() else { return; };
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
                }
            } else {
                let id = id.to_string();
                store.borrow().providers.iter().find(|p| p.id == id).cloned().unwrap_or_else(|| ProviderData {
                    id: String::new(),
                    name: String::new(),
                    endpoint: String::new(),
                    remark: String::new(),
                    website: String::new(),
                    api_key: String::new(),
                    selected_model: String::new(),
                    models: vec![],
                })
            };
            match list_models(&provider.endpoint, &provider.api_key) {
                Ok(_) => state.set_provider_message("测速完成，API 连接正常。".into()),
                Err(err) => state.set_provider_message(format!("测速失败：{}", zh_error(&err.to_string())).into()),
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_select_image_model(move |provider_id, model| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_image_provider_id(provider_id);
                state.set_image_model(model);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_select_reasoning_model(move |provider_id, model| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_reasoning_provider_id(provider_id);
                state.set_reasoning_model(model);
            }
        });
    }
}

fn wire_reference_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            if let Some(files) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_files()
            {
                let mut store = store.borrow_mut();
                for path in files {
                    if let Ok(image) = load_image(&path) {
                        store.references.push(ReferenceData {
                            id: Uuid::new_v4().to_string(),
                            image,
                            source_path: path.display().to_string(),
                        });
                    }
                }
                push_references(&app, &store);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_paste_reference(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Ok(img) = clipboard.get_image() {
                    let image = image_from_clipboard(img);
                    store.borrow_mut().references.push(ReferenceData {
                        id: Uuid::new_v4().to_string(),
                        image,
                        source_path: String::new(),
                    });
                    push_references(&app, &store.borrow());
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_remove_reference(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let id = id.to_string();
                store.borrow_mut().references.retain(|r| r.id != id);
                push_references(&app, &store.borrow());
            }
        });
    }
}

fn wire_generation_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_generate(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            start_generation(&app, store.clone(), None);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_regenerate(move |id| {
            let Some(app) = app_weak.upgrade() else { return; };
            let prompt = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id.to_string())
                .map(|g| g.prompt.clone());
            start_generation(&app, store.clone(), prompt);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_quote_generation(move |id| {
            let Some(app) = app_weak.upgrade() else { return; };
            let id = id.to_string();
            if let Some(item) = store.borrow().generations.iter().find(|g| g.id == id).cloned() {
                let state = app.global::<AppState>();
                state.set_quote_title(item.title.into());
                state.set_quote_prompt(item.prompt.into());
                state.set_quote_ratio(item.ratio.into());
                state.set_quote_quality(item.quality.into());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_asset(move |id| {
            let Some(app) = app_weak.upgrade() else { return; };
            let mut store = store.borrow_mut();
            let id = id.to_string();
            if let Some(item) = store.generations.iter().find(|g| g.id == id).cloned() {
                if !store.assets.iter().any(|a| a.id == item.id) {
                    let mut asset = item.clone();
                    asset.source_path = "asset".to_string();
                    store.assets.insert(0, asset);
                }
            }
            push_all(&app, &store);
        });
    }
}

fn wire_viewer_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_viewer(move |id, source| {
            let Some(app) = app_weak.upgrade() else { return; };
            open_viewer(&app, &store.borrow(), id.as_str(), source.as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_viewer(move || {
            if let Some(app) = app_weak.upgrade() {
                app.global::<AppState>().set_viewer_open(false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_add_asset(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            let id = state.get_viewer_id().to_string();
            let mut store = store.borrow_mut();
            if let Some(item) = store.generations.iter().find(|g| g.id == id).cloned() {
                if !store.assets.iter().any(|a| a.id == id) {
                    let mut asset = item;
                    asset.source_path = "asset".into();
                    store.assets.insert(0, asset);
                }
            }
            state.set_viewer_in_assets(true);
            push_all(&app, &store);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_regenerate(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let prompt = app.global::<AppState>().get_viewer_prompt().to_string();
            app.global::<AppState>().set_viewer_open(false);
            start_generation(&app, store.clone(), Some(prompt));
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_viewer_edit(move || {
            let Some(app) = app_weak.upgrade() else { return; };
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
        state.on_viewer_use_same(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let prompt = app.global::<AppState>().get_viewer_prompt().to_string();
            app.global::<AppState>().set_viewer_open(false);
            app.global::<AppState>().set_prompt(prompt.into());
            navigate_to(&app, "workspace");
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_use_reference(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            store.borrow_mut().references.push(ReferenceData {
                id: Uuid::new_v4().to_string(),
                image: state.get_viewer_image(),
                source_path: String::new(),
            });
            push_references(&app, &store.borrow());
            state.set_viewer_open(false);
            navigate_to(&app, "workspace");
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_request_delete_asset(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_pending_delete_id(id);
                state.set_delete_confirm_open(true);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_confirm_delete(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            let id = state.get_pending_delete_id().to_string();
            store.borrow_mut().assets.retain(|a| a.id != id);
            state.set_delete_confirm_open(false);
            state.set_viewer_open(false);
            push_all(&app, &store.borrow());
        });
    }
}

fn wire_notification_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();
    let app_weak = app.as_weak();
    state.on_mark_notification_read(move |id| {
        if let Some(app) = app_weak.upgrade() {
            let mut store = store.borrow_mut();
            let id = id.to_string();
            if let Some(item) = store.notifications.iter_mut().find(|n| n.id == id) {
                item.read = true;
            }
            push_notifications(&app, &store);
        }
    });
}

fn start_generation(app: &AppWindow, store: Rc<RefCell<Store>>, override_prompt: Option<String>) {
    let state = app.global::<AppState>();
    let raw_prompt = override_prompt.unwrap_or_else(|| state.get_prompt().trim().to_string());
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
    let reasoning_id = state.get_reasoning_provider_id().to_string();
    let reasoning_model = state.get_reasoning_model().to_string();
    let store_ref = store.borrow();
    let Some(image_provider) = store_ref.providers.iter().find(|p| p.id == provider_id).cloned() else {
        state.set_generation_status("请先选择生图模型".into());
        navigate_to(app, "models");
        return;
    };
    let reasoning_provider = store_ref.providers.iter().find(|p| p.id == reasoning_id).cloned();
    drop(store_ref);

    state.set_generating(true);
    state.set_generation_status("正在优化提示词...".into());
    navigate_to(app, "generation");

    let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
    let ratio = resolve_ratio(&state.get_ratio().to_string(), &raw_prompt, &state.get_quote_ratio().to_string());
    let quality = state.get_quality().to_string();
    let count = state.get_count().clamp(1, 4);
    let prompt_for_thread = raw_prompt.clone();

    let placeholder = ConversationItem {
        id: SharedString::from(Uuid::new_v4().to_string()),
        title: SharedString::from(short_text(&prompt_for_thread, 10)),
        image: Image::default(),
        loading: true,
    };
    {
        let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
        conversations.insert(0, placeholder);
        state.set_conversations(ModelRc::new(VecModel::from(conversations)));
    }

    let optimized = if let Some(reasoning) = reasoning_provider {
        match optimize_prompt(&reasoning, &reasoning_model, &raw_prompt, &category, &ratio, &quality, &state) {
            Ok(text) if !text.trim().is_empty() => text,
            Ok(_) => raw_prompt.clone(),
            Err(_) => raw_prompt.clone(),
        }
    } else {
        raw_prompt.clone()
    };

    state.set_generation_status("正在调用生图模型...".into());
    let result = generate_images(&image_provider, &image_model, &optimized, count, &ratio, &quality);
    let time = Local::now().format("%Y-%m-%d %H:%M").to_string();
    match result {
        Ok(images) => {
            let mut created = Vec::new();
            for image in images {
                let (width, height) = ratio_dimensions(&ratio);
                created.push(AssetData {
                    id: Uuid::new_v4().to_string(),
                    title: short_text(&raw_prompt, 18),
                    category: category.clone(),
                    kind: state.get_mode().to_string(),
                    time: time.clone(),
                    prompt: optimized.clone(),
                    ratio: ratio.clone(),
                    quality: quality.clone(),
                    model: image_model.clone(),
                    width,
                    height,
                    image,
                    source_path: String::new(),
                });
            }
            let mut store_mut = store.borrow_mut();
            for item in created.into_iter().rev() {
                store_mut.generations.insert(0, item);
            }
            if let Some(first) = store_mut.generations.first() {
                let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
                if let Some(row) = conversations.iter_mut().find(|c| c.loading) {
                    row.image = first.image.clone();
                    row.loading = false;
                }
                state.set_conversations(ModelRc::new(VecModel::from(conversations)));
            }
            store_mut.notifications.insert(0, NotificationData {
                id: Uuid::new_v4().to_string(),
                title: format!("生成成功：{}", short_text(&raw_prompt, 24)),
                model: image_model,
                time,
                reason: String::new(),
                success: true,
                read: false,
            });
            state.set_prompt("".into());
            state.set_generation_status("生成完成".into());
            state.set_generating(false);
            push_all(app, &store_mut);
        }
        Err(err) => {
            let reason = zh_error(&err.to_string());
            let mut store_mut = store.borrow_mut();
            store_mut.notifications.insert(0, NotificationData {
                id: Uuid::new_v4().to_string(),
                title: format!("生成失败：{}", short_text(&raw_prompt, 24)),
                model: image_model,
                time,
                reason: reason.clone(),
                success: false,
                read: false,
            });
            state.set_generation_status(format!("生成失败：{}", reason).into());
            state.set_generating(false);
            push_notifications(app, &store_mut);
        }
    }
}

fn optimize_prompt(
    provider: &ProviderData,
    model: &str,
    prompt: &str,
    category: &str,
    ratio: &str,
    quality: &str,
    state: &AppState,
) -> Result<String> {
    if provider.endpoint.trim().is_empty() || provider.api_key.trim().is_empty() || model.trim().is_empty() {
        return Ok(prompt.to_string());
    }
    let endpoint = normalize_chat_endpoint(&provider.endpoint);
    let user = format!(
        "用户需求：{}\n分类：{}\n比例：{}\n清晰度：{}\n引用图片标题：{}\n引用图片提示词：{}\n引用图片比例：{}\n引用图片清晰度：{}\n请只输出优化后的图片生成提示词。",
        prompt,
        category,
        ratio,
        quality,
        state.get_quote_title(),
        state.get_quote_prompt(),
        state.get_quote_ratio(),
        state.get_quote_quality()
    );
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": "你是 ArtForgeStudio 的推理模型，主要用于提示词和剧本优化。请把用户需求优化为可用于生图模型的高质量提示词，只输出最终提示词，不展示思考过程。" },
            { "role": "user", "content": user }
        ],
        "temperature": 0.4,
        "max_tokens": 800
    });
    let value = request_json("POST", &endpoint, &provider.api_key, Some(body))?;
    Ok(extract_text(&value).unwrap_or_else(|| prompt.to_string()))
}

fn generate_images(provider: &ProviderData, model: &str, prompt: &str, count: i32, ratio: &str, quality: &str) -> Result<Vec<Image>> {
    if provider.endpoint.trim().is_empty() || provider.api_key.trim().is_empty() || model.trim().is_empty() {
        return Err(anyhow!("缺少 API 请求地址、API Key 或模型名称"));
    }
    let endpoint = normalize_image_endpoint(&provider.endpoint);
    let size = size_for(ratio, quality);
    let mut images = Vec::new();
    for _ in 0..count {
        let body = json!({ "model": model, "prompt": prompt, "n": 1, "size": size });
        let value = request_json("POST", &endpoint, &provider.api_key, Some(body))?;
        let raw = extract_image(&value).ok_or_else(|| anyhow!("接口没有返回图片"))?;
        images.push(image_from_response(&raw)?);
    }
    Ok(images)
}

fn list_models(endpoint: &str, api_key: &str) -> Result<Vec<String>> {
    if endpoint.trim().is_empty() || api_key.trim().is_empty() {
        return Err(anyhow!("缺少 API 请求地址或 API Key"));
    }
    let value = request_json("GET", &normalize_models_endpoint(endpoint), api_key, None)?;
    let source = value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .or_else(|| value.get("models").and_then(Value::as_array))
        .ok_or_else(|| anyhow!("接口没有返回模型列表"))?;
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
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()?;
    let mut request = match method {
        "GET" => client.get(endpoint),
        _ => client.post(endpoint),
    }
    .header("Accept", "application/json")
    .bearer_auth(api_key);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().context("请求超时，请检查网络环境或服务商接口状态后重试")?;
    let status = response.status();
    let text = response.text().unwrap_or_default();
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
            .unwrap_or_else(|| text.clone());
        return Err(anyhow!(message));
    }
    if text.trim().is_empty() {
        Ok(json!({}))
    } else {
        Ok(serde_json::from_str(&text).unwrap_or_else(|_| json!({ "text": text })))
    }
}

fn extract_text(value: &Value) -> Option<String> {
    for pointer in ["/output_text", "/content", "/text", "/choices/0/message/content", "/choices/0/text"] {
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
    if let Some(data) = value.get("data").and_then(Value::as_array) {
        for item in data {
            if let Some(b64) = item.get("b64_json").and_then(Value::as_str) {
                return Some(format!("data:image/png;base64,{b64}"));
            }
            if let Some(url) = item.get("url").and_then(Value::as_str) {
                return Some(url.to_string());
            }
        }
    }
    value
        .get("image")
        .and_then(Value::as_str)
        .or_else(|| value.get("url").and_then(Value::as_str))
        .map(str::to_string)
}

fn image_from_response(raw: &str) -> Result<Image> {
    if raw.starts_with("data:image") {
        let (_, data) = raw.split_once(',').ok_or_else(|| anyhow!("图片数据格式不正确"))?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
        return image_from_bytes(&bytes);
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        let bytes = reqwest::blocking::get(raw)?.bytes()?;
        return image_from_bytes(&bytes);
    }
    image_from_bytes(&base64::engine::general_purpose::STANDARD.decode(raw)?)
}

fn image_from_bytes(bytes: &[u8]) -> Result<Image> {
    let img = image::load_from_memory(bytes)?.to_rgba8();
    let (w, h) = img.dimensions();
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Ok(Image::from_rgba8(buffer))
}

fn image_from_clipboard(img: arboard::ImageData<'_>) -> Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        img.bytes.as_ref(),
        img.width as u32,
        img.height as u32,
    );
    Image::from_rgba8(buffer)
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

fn init_portable_dirs(app: &AppWindow) -> Result<()> {
    let data_dir = app_dir().join("data");
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
    let mut bases = vec![app_dir()];
    if let Ok(current_dir) = std::env::current_dir() {
        if !bases.iter().any(|dir| dir == &current_dir) {
            bases.push(current_dir);
        }
    }
    bases
        .into_iter()
        .map(|base| base.join("assets").join(relative))
        .find(|path| path.exists())
}

fn seed_inspiration(app: &AppWindow, store: &Rc<RefCell<Store>>) -> Result<()> {
    let dirs = inspiration_dirs();
    let mut items = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                continue;
            }
            if let Ok(image) = load_image(&path) {
                let index = items.len() + 1;
                let (title, category, kind) = inspiration_meta(index);
                let (width, height) = image::image_dimensions(&path)
                    .map(|(w, h)| (w as i32, h as i32))
                    .unwrap_or((1254, 1254));
                items.push(AssetData {
                    id: format!("inspiration-{index}"),
                    title: title.to_string(),
                    category: category.to_string(),
                    kind: kind.to_string(),
                    time: "官方示例".to_string(),
                    prompt: "官方灵感示例，可用于做同款或作为参考图继续创作。".to_string(),
                    ratio: "1:1".to_string(),
                    quality: "1K".to_string(),
                    model: "官方示例".to_string(),
                    width,
                    height,
                    image,
                    source_path: path.display().to_string(),
                });
            }
        }
    }
    store.borrow_mut().inspiration = items;
    push_all(app, &store.borrow());
    Ok(())
}

fn inspiration_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![app_dir().join("assets").join("sucai")];
    if let Ok(current_dir) = std::env::current_dir() {
        let current_assets = current_dir.join("assets").join("sucai");
        if !dirs.iter().any(|dir| dir == &current_assets) {
            dirs.push(current_assets);
        }
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
        ("城市战场", "scene", "film"),
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

fn open_provider_editor(app: &AppWindow, store: &Store, id: &str) {
    let state = app.global::<AppState>();
    state.set_edit_provider_id(id.into());
    if let Some(provider) = store.providers.iter().find(|p| p.id == id) {
        state.set_provider_name(provider.name.clone().into());
        state.set_provider_remark(provider.remark.clone().into());
        state.set_provider_website(provider.website.clone().into());
        state.set_provider_endpoint(provider.endpoint.clone().into());
        state.set_provider_api_key(provider.api_key.clone().into());
        state.set_provider_model_name(provider.selected_model.clone().into());
        state.set_fetched_models(ModelRc::new(VecModel::from(
            provider.models.iter().cloned().map(SharedString::from).collect::<Vec<_>>(),
        )));
    } else {
        state.set_provider_name("".into());
        state.set_provider_remark("".into());
        state.set_provider_website("".into());
        state.set_provider_endpoint("".into());
        state.set_provider_api_key("".into());
        state.set_provider_model_name("".into());
        state.set_fetched_models(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
    }
    state.set_provider_message("".into());
    state.set_provider_editor_open(true);
}

fn open_viewer(app: &AppWindow, store: &Store, id: &str, source: &str) {
    let item = match source {
        "asset" => store.assets.iter().find(|a| a.id == id),
        "inspiration" => store.inspiration.iter().find(|a| a.id == id),
        _ => store.generations.iter().find(|a| a.id == id),
    };
    let Some(item) = item else { return; };
    let state = app.global::<AppState>();
    state.set_viewer_id(item.id.clone().into());
    state.set_viewer_source(source.into());
    state.set_viewer_image(item.image.clone());
    state.set_viewer_title(item.title.clone().into());
    state.set_viewer_prompt(item.prompt.clone().into());
    state.set_viewer_time(item.time.clone().into());
    state.set_viewer_ratio(item.ratio.clone().into());
    state.set_viewer_quality(item.quality.clone().into());
    state.set_viewer_model(item.model.clone().into());
    state.set_viewer_width(item.width);
    state.set_viewer_height(item.height);
    state.set_viewer_in_assets(store.assets.iter().any(|a| a.id == id));
    state.set_viewer_open(true);
}

fn navigate_to(app: &AppWindow, page: &str) {
    let state = app.global::<AppState>();
    if page != "welcome" && !state.get_logged_in() {
        state.set_auth_open(true);
        return;
    }
    state.set_page(page.into());
}

fn push_all(app: &AppWindow, store: &Store) {
    push_providers(app, store);
    push_assets(app, store);
    push_generations(app, store);
    push_inspiration(app, store);
    push_notifications(app, store);
    push_references(app, store);
}

fn push_providers(app: &AppWindow, store: &Store) {
    app.global::<AppState>().set_providers(ModelRc::new(VecModel::from(
        store.providers.iter().map(to_provider_view).collect::<Vec<_>>(),
    )));
}

fn push_assets(app: &AppWindow, store: &Store) {
    app.global::<AppState>().set_assets(ModelRc::new(VecModel::from(
        store.assets.iter().map(to_asset_view).collect::<Vec<_>>(),
    )));
}

fn push_generations(app: &AppWindow, store: &Store) {
    app.global::<AppState>().set_generations(ModelRc::new(VecModel::from(
        store.generations.iter().map(to_asset_view).collect::<Vec<_>>(),
    )));
}

fn push_inspiration(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    state.set_inspiration(ModelRc::new(VecModel::from(
        store.inspiration.iter().map(to_asset_view).collect::<Vec<_>>(),
    )));
    let mut cols: [Vec<AssetItem>; 5] = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for (index, item) in store.inspiration.iter().enumerate() {
        cols[index % 5].push(to_asset_view(item));
    }
    state.set_inspiration_col_0(ModelRc::new(VecModel::from(cols[0].clone())));
    state.set_inspiration_col_1(ModelRc::new(VecModel::from(cols[1].clone())));
    state.set_inspiration_col_2(ModelRc::new(VecModel::from(cols[2].clone())));
    state.set_inspiration_col_3(ModelRc::new(VecModel::from(cols[3].clone())));
    state.set_inspiration_col_4(ModelRc::new(VecModel::from(cols[4].clone())));
}

fn push_references(app: &AppWindow, store: &Store) {
    app.global::<AppState>().set_references(ModelRc::new(VecModel::from(
        store.references.iter().map(|item| ReferenceItem {
            id: item.id.clone().into(),
            image: item.image.clone(),
            source_path: item.source_path.clone().into(),
        }).collect::<Vec<_>>(),
    )));
}

fn push_notifications(app: &AppWindow, store: &Store) {
    let has_unread = store.notifications.iter().any(|n| !n.read);
    let state = app.global::<AppState>();
    state.set_has_unread(has_unread);
    state.set_notifications(ModelRc::new(VecModel::from(
        store.notifications.iter().map(|n| NotificationItem {
            id: n.id.clone().into(),
            title: n.title.clone().into(),
            model: n.model.clone().into(),
            time: n.time.clone().into(),
            reason: n.reason.clone().into(),
            success: n.success,
            read: n.read,
        }).collect::<Vec<_>>(),
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
        models: ModelRc::new(VecModel::from(provider.models.iter().cloned().map(SharedString::from).collect::<Vec<_>>())),
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
    }
}

fn start_countdown(app_weak: Weak<AppWindow>) {
    let timer = Rc::new(slint::Timer::default());
    let timer_for_tick = timer.clone();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(1), move || {
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
    });
}

fn apply_sprite_theme(app: &AppWindow) {
    let p = app.global::<AppTheme>();
    p.set_bg(slint::Color::from_rgb_u8(236, 251, 244));
    p.set_panel(slint::Color::from_rgb_u8(255, 255, 255));
    p.set_panel_soft(slint::Color::from_rgb_u8(224, 248, 238));
    p.set_border(slint::Color::from_rgb_u8(194, 235, 217));
    p.set_text(slint::Color::from_rgb_u8(7, 19, 15));
    p.set_muted(slint::Color::from_rgb_u8(80, 98, 91));
    p.set_weak(slint::Color::from_rgb_u8(141, 160, 150));
    p.set_accent(slint::Color::from_rgb_u8(0, 217, 130));
    p.set_accent_dark(slint::Color::from_rgb_u8(6, 185, 111));
}

fn apply_dark_theme(app: &AppWindow) {
    let p = app.global::<AppTheme>();
    p.set_bg(slint::Color::from_rgb_u8(8, 11, 18));
    p.set_panel(slint::Color::from_rgb_u8(18, 22, 32));
    p.set_panel_soft(slint::Color::from_rgb_u8(31, 38, 54));
    p.set_border(slint::Color::from_rgb_u8(46, 55, 76));
    p.set_text(slint::Color::from_rgb_u8(236, 242, 248));
    p.set_muted(slint::Color::from_rgb_u8(157, 170, 188));
    p.set_weak(slint::Color::from_rgb_u8(115, 127, 146));
    p.set_accent(slint::Color::from_rgb_u8(0, 217, 130));
    p.set_accent_dark(slint::Color::from_rgb_u8(6, 185, 111));
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

fn size_for(ratio: &str, quality: &str) -> String {
    let base = match quality {
        "4K" => 2048,
        "2K" => 1536,
        _ => 1024,
    };
    let (w, h) = ratio_dimensions(ratio);
    if w == h {
        return format!("{base}x{base}");
    }
    let rw = w as f32;
    let rh = h as f32;
    if rw >= rh {
        format!("{}x{}", base, ((base as f32) * rh / rw).round() as i32)
    } else {
        format!("{}x{}", ((base as f32) * rw / rh).round() as i32, base)
    }
}

fn ratio_dimensions(ratio: &str) -> (i32, i32) {
    match ratio {
        "21:9" => (21, 9),
        "16:9" => (16, 9),
        "3:2" => (3, 2),
        "4:3" => (4, 3),
        "3:4" => (3, 4),
        "2:3" => (2, 3),
        "9:16" => (9, 16),
        "5:4" => (5, 4),
        "4:5" => (4, 5),
        "2:1" => (2, 1),
        "1:2" => (1, 2),
        "9:21" => (9, 21),
        _ => (1, 1),
    }
}

fn resolve_category(selected: &str, prompt: &str) -> String {
    if selected != "smart" {
        return selected.to_string();
    }
    let text = prompt.to_lowercase();
    if text.contains("角色") || text.contains("人物") || text.contains("character") {
        "character"
    } else if text.contains("场景") || text.contains("地图") || text.contains("scene") || text.contains("map") {
        "scene"
    } else if text.contains("ui") || text.contains("界面") || text.contains("按钮") {
        "ui"
    } else if text.contains("特效") || text.contains("技能") || text.contains("effect") || text.contains("vfx") {
        "effect"
    } else {
        "other"
    }
    .to_string()
}

fn resolve_ratio(selected: &str, prompt: &str, quoted: &str) -> String {
    if selected != "smart" {
        return selected.to_string();
    }
    let text = prompt.to_lowercase();
    for ratio in ["21:9", "16:9", "3:2", "4:3", "1:1", "3:4", "2:3", "9:16", "5:4", "4:5", "2:1", "1:2", "9:21"] {
        if text.contains(ratio) {
            return ratio.to_string();
        }
    }
    if !quoted.is_empty() {
        return quoted.to_string();
    }
    "1:1".to_string()
}

fn short_text(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push('…');
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

fn zh_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("timeout") || raw.contains("超时") {
        "请求超时，请检查网络环境或服务商接口状态后重试。".to_string()
    } else if lower.contains("unauthorized") || lower.contains("invalid api key") || lower.contains("401") {
        "API Key 无效或权限不足，请检查模型配置。".to_string()
    } else if lower.contains("not found") || lower.contains("404") {
        "模型或接口地址不存在，请检查模型名称和 API 请求地址。".to_string()
    } else if lower.contains("rate") || lower.contains("429") {
        "请求过于频繁或额度不足，请稍后重试。".to_string()
    } else if lower.contains("size") || lower.contains("resolution") {
        "当前模型不支持所选尺寸，请更换比例或分辨率。".to_string()
    } else if raw.trim().is_empty() {
        "接口返回错误，请检查 API 配置、模型能力或稍后重试。".to_string()
    } else {
        raw.to_string()
    }
}
