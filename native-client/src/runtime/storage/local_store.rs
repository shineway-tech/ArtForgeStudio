use super::*;

pub(super) fn user_profile_path() -> PathBuf {
    app_data_dir().join("user-profile.json")
}

pub(super) fn load_user_profile(app: &AppWindow) {
    let Ok(text) = fs::read_to_string(user_profile_path()) else {
        return;
    };
    let Ok(profile) = serde_json::from_str::<UserProfileData>(&text) else {
        return;
    };
    let state = app.global::<AppState>();
    // Legacy local login and credit values are deliberately not trusted. A backend
    // refresh or an explicit offline choice establishes the runtime session.
    state.set_logged_in(false);
    state.set_session_state("signed_out".into());
    state.set_offline_mode(false);
    let migrated_backend_auth = profile.backend_auth_version >= 1 && profile.ever_authenticated;
    state.set_ever_authenticated(migrated_backend_auth);
    state.set_offline_available(migrated_backend_auth);
    state.set_email_mask(profile.email_mask.into());
    state.set_accepted_user_terms_version(profile.accepted_user_terms_version.into());
    state.set_accepted_privacy_version(profile.accepted_privacy_version.into());
    state.set_nickname(profile.nickname.into());
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
}

pub(super) fn save_user_profile(app: &AppWindow) {
    let state = app.global::<AppState>();
    let nickname = state.get_nickname().to_string();
    let profile = UserProfileData {
        logged_in: false,
        nickname,
        backend_auth_version: 1,
        ever_authenticated: state.get_ever_authenticated(),
        email_mask: state.get_email_mask().to_string(),
        accepted_user_terms_version: state.get_accepted_user_terms_version().to_string(),
        accepted_privacy_version: state.get_accepted_privacy_version().to_string(),
        theme_id: state.get_theme_id().to_string(),
        card_style: if state.get_card_style() == "square" {
            "square".to_string()
        } else {
            "rounded".to_string()
        },
        language: state.get_language().to_string(),
        asset_type: resolve_category(&state.get_asset_type().to_string(), ""),
    };
    if let Ok(text) = serde_json::to_string_pretty(&profile) {
        let path = user_profile_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temporary = path.with_extension("json.tmp");
        if fs::write(&temporary, text).is_ok() {
            let _ = fs::rename(temporary, path);
        }
    }
}

pub(super) fn local_store_path() -> PathBuf {
    app_data_dir().join("local-store.json")
}

pub(super) fn load_local_store(app: &AppWindow, store: &Rc<RefCell<Store>>) {
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
    let migrated_local_store = {
        let mut store_mut = store.borrow_mut();
        // Legacy provider endpoints and API keys are intentionally ignored.
        store_mut.model_groups.clear();
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
        let migrated_prompt_drafts = normalize_reserved_prompt_drafts(&mut store_mut.prompt_drafts);
        store_mut.custom_prompts = normalize_custom_prompts(data.custom_prompts);
        store_mut.custom_prompt_times = data.custom_prompt_times;
        store_mut.custom_prompt_profiles = data.custom_prompt_profiles;
        store_mut.canvas_notes = data.canvas_notes;
        normalize_canvas_groups(&mut store_mut.canvas_notes);
        store_mut.canvas_links = data.canvas_links;
        let original_prompt_times = store_mut.custom_prompt_times.clone();
        let retained = store_mut
            .custom_prompts
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        store_mut
            .custom_prompt_times
            .retain(|prompt, _| retained.contains(prompt));
        store_mut
            .custom_prompt_profiles
            .retain(|prompt, _| retained.contains(prompt));
        let migration_time = Local::now().format("%Y-%m-%d %H:%M").to_string();
        for prompt in store_mut.custom_prompts.clone() {
            store_mut
                .custom_prompt_times
                .entry(prompt)
                .or_insert_with(|| migration_time.clone());
        }
        migrated_prompt_drafts || store_mut.custom_prompt_times != original_prompt_times
    };
    let state = app.global::<AppState>();
    state.set_image_model("".into());
    state.set_reasoning_model("".into());
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    state.set_asset_type(category.clone().into());
    state.set_prompt(prompt_draft_for_category(&store.borrow().prompt_drafts, &category).into());
    if migrated_local_store {
        save_local_store(app, &store.borrow());
    }
}

pub(super) fn normalize_reserved_prompt_drafts(drafts: &mut PromptDrafts) -> bool {
    let mut migrated = false;
    for prompt in [
        &mut drafts.character,
        &mut drafts.scene,
        &mut drafts.ui,
        &mut drafts.effect,
        &mut drafts.action_sequence,
    ] {
        if prompt.trim() == "//" {
            prompt.clear();
            migrated = true;
        }
    }
    migrated
}

pub(super) fn prompt_draft_for_category(drafts: &PromptDrafts, category: &str) -> String {
    match category {
        "scene" => drafts.scene.clone(),
        "ui" => drafts.ui.clone(),
        "effect" => drafts.effect.clone(),
        "action-sequence" => drafts.action_sequence.clone(),
        _ => drafts.character.clone(),
    }
}

pub(super) fn set_prompt_draft_for_category(
    drafts: &mut PromptDrafts,
    category: &str,
    prompt: String,
) {
    match category {
        "scene" => drafts.scene = prompt,
        "ui" => drafts.ui = prompt,
        "effect" => drafts.effect = prompt,
        "action-sequence" => drafts.action_sequence = prompt,
        _ => drafts.character = prompt,
    }
}

pub(super) fn store_current_prompt_draft(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    category: &str,
) {
    let prompt = app.global::<AppState>().get_prompt().to_string();
    set_prompt_draft_for_category(&mut store.borrow_mut().prompt_drafts, category, prompt);
}

pub(super) const MAX_CUSTOM_PROMPTS: usize = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SaveCustomPromptResult {
    Saved,
    Empty,
    Duplicate,
    Missing,
}

pub(super) fn save_custom_prompt_to_store(
    store: &mut Store,
    original: &str,
    raw: &str,
    timestamp: &str,
) -> SaveCustomPromptResult {
    let prompt = raw.trim();
    if prompt.is_empty() {
        return SaveCustomPromptResult::Empty;
    }
    let original = original.trim();
    if store
        .custom_prompts
        .iter()
        .any(|item| item == prompt && item != original)
    {
        return SaveCustomPromptResult::Duplicate;
    }
    if original.is_empty() {
        store.custom_prompts.insert(0, prompt.to_string());
    } else {
        let Some(index) = store
            .custom_prompts
            .iter()
            .position(|item| item == original)
        else {
            return SaveCustomPromptResult::Missing;
        };
        store.custom_prompts[index] = prompt.to_string();
        store.custom_prompt_times.remove(original);
    }
    store
        .custom_prompt_times
        .insert(prompt.to_string(), timestamp.to_string());
    store.custom_prompts.truncate(MAX_CUSTOM_PROMPTS);
    let retained = store
        .custom_prompts
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    store
        .custom_prompt_times
        .retain(|item, _| retained.contains(item));
    SaveCustomPromptResult::Saved
}

pub(super) fn remove_custom_prompt_from_store(store: &mut Store, prompt: &str) -> bool {
    let Some(index) = store.custom_prompts.iter().position(|item| item == prompt) else {
        return false;
    };
    store.custom_prompts.remove(index);
    store.custom_prompt_times.remove(prompt);
    store.custom_prompt_profiles.remove(prompt);
    true
}

pub(super) fn save_custom_prompt_profile(
    store: &mut Store,
    original: &str,
    prompt: &str,
    profile: CustomPromptProfile,
) {
    let original = original.trim();
    let prompt = prompt.trim();
    if !original.is_empty() && original != prompt {
        store.custom_prompt_profiles.remove(original);
    }
    store
        .custom_prompt_profiles
        .insert(prompt.to_string(), profile);
    let retained = store
        .custom_prompts
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    store
        .custom_prompt_profiles
        .retain(|item, _| retained.contains(item));
}

pub(super) fn normalize_custom_prompts(prompts: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for raw in prompts {
        let prompt = raw.trim();
        if prompt.is_empty() || normalized.iter().any(|item| item == prompt) {
            continue;
        }
        normalized.push(prompt.to_string());
        if normalized.len() == MAX_CUSTOM_PROMPTS {
            break;
        }
    }
    normalized
}

pub(super) fn references_for_category<'a>(
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

pub(super) fn references_for_category_mut<'a>(
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

pub(super) fn recover_output_assets(app: &AppWindow, store: &Rc<RefCell<Store>>) {
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

pub(super) fn recovered_asset_title(path: &Path) -> String {
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

pub(super) fn save_local_store(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let data = LocalStoreData {
        generations: store.generations.iter().map(stored_asset_from).collect(),
        assets: store.assets.iter().map(stored_asset_from).collect(),
        notifications: store.notifications.clone(),
        image_model: state.get_image_model().to_string(),
        reasoning_model: state.get_reasoning_model().to_string(),
        prompt_drafts: store.prompt_drafts.clone(),
        custom_prompts: store.custom_prompts.clone(),
        custom_prompt_times: store.custom_prompt_times.clone(),
        custom_prompt_profiles: store.custom_prompt_profiles.clone(),
        canvas_notes: store.canvas_notes.clone(),
        canvas_links: store.canvas_links.clone(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&data) {
        let path = local_store_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temporary = path.with_extension("json.tmp");
        if fs::write(&temporary, text).is_ok() {
            let _ = fs::rename(temporary, path);
        }
    }
}

pub(super) fn stored_asset_from(asset: &AssetData) -> StoredAssetData {
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

pub(super) fn asset_from_stored(asset: StoredAssetData) -> Option<AssetData> {
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
