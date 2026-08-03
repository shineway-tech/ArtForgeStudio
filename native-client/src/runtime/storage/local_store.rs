use super::*;

pub(super) fn json_backup_path(path: &Path) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("json");
    path.with_extension(format!("{extension}.bak"))
}

pub(super) fn restore_json_backup_if_needed(path: &Path) {
    if path.exists() {
        return;
    }
    let backup = json_backup_path(path);
    if backup.exists() {
        let _ = fs::rename(backup, path);
    }
}

pub(super) fn replace_json_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text)?;

    #[cfg(windows)]
    {
        let backup = json_backup_path(path);
        if path.exists() {
            match fs::remove_file(&backup) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    let _ = fs::remove_file(&temporary);
                    return Err(error);
                }
            }
            if let Err(error) = fs::rename(path, &backup) {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
            if let Err(error) = fs::rename(&temporary, path) {
                let _ = fs::rename(&backup, path);
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
            let _ = fs::remove_file(backup);
            return Ok(());
        }
    }

    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

pub(super) fn user_profile_path() -> PathBuf {
    app_data_dir().join("user-profile.json")
}

pub(super) fn load_user_profile(app: &AppWindow) {
    let path = user_profile_path();
    restore_json_backup_if_needed(&path);
    let Ok(text) = fs::read_to_string(path) else {
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
        let _ = replace_json_file(&path, &text);
    }
}

pub(super) fn local_store_path() -> PathBuf {
    app_data_dir().join("local-store.json")
}

pub(super) fn load_local_store(app: &AppWindow, store: &Rc<RefCell<Store>>) {
    let path = local_store_path();
    restore_json_backup_if_needed(&path);
    let Ok(text) = fs::read_to_string(path) else {
        recover_output_assets(app, store);
        save_local_store(app, &store.borrow());
        return;
    };
    let Ok(data) = serde_json::from_str::<LocalStoreData>(&text) else {
        recover_output_assets(app, store);
        save_local_store(app, &store.borrow());
        return;
    };
    let saved_image_model = data.image_model.clone();
    let saved_reasoning_model = data.reasoning_model.clone();
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
        store_mut.dismissed_prompt_history = data.dismissed_prompt_history;
        let migrated_prompt_drafts = normalize_reserved_prompt_drafts(&mut store_mut.prompt_drafts);
        store_mut.custom_prompts = normalize_custom_prompts(data.custom_prompts);
        store_mut.selected_custom_prompts = data.selected_custom_prompts;
        store_mut.custom_prompt_times = data.custom_prompt_times;
        store_mut.custom_prompt_profiles = data.custom_prompt_profiles;
        store_mut.canvas_notes = data.canvas_notes;
        normalize_canvas_groups(&mut store_mut.canvas_notes);
        let fitted_canvas_groups = fit_groups_to_children(&mut store_mut.canvas_notes);
        store_mut.canvas_links = data.canvas_links;
        store_mut.deep_prompt_job_id = data.deep_prompt_job_id;
        store_mut.deep_prompt_bindings = data.deep_prompt_bindings;
        store_mut.contact_popup_dismissed = data.contact_popup_dismissed;
        let original_prompt_times = store_mut.custom_prompt_times.clone();
        let retained = store_mut
            .custom_prompts
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        normalize_selected_custom_prompts(&mut store_mut.selected_custom_prompts, &retained);
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
        migrated_prompt_drafts
            || fitted_canvas_groups
            || store_mut.custom_prompt_times != original_prompt_times
    };
    let state = app.global::<AppState>();
    state.set_contact_popup_open(!store.borrow().contact_popup_dismissed);
    state.set_image_model(saved_image_model.into());
    state.set_reasoning_model(saved_reasoning_model.into());
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    state.set_asset_type(category.clone().into());
    state.set_prompt(prompt_draft_for_category(&store.borrow().prompt_drafts, &category).into());
    state.set_negative_prompt(
        negative_prompt_draft_for_category(&store.borrow().prompt_drafts, &category).into(),
    );
    sync_deep_prompt_binding_for_category(app, &store.borrow(), &category);
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

pub(super) fn dismiss_prompt_history_entry(store: &mut Store, prompt: &str) -> bool {
    let prompt = prompt.trim();
    !prompt.is_empty() && store.dismissed_prompt_history.insert(prompt.to_string())
}

pub(super) fn clear_prompt_history_entries(store: &mut Store) -> bool {
    let prompts = store
        .generations
        .iter()
        .map(|item| item.prompt.trim())
        .filter(|prompt| !prompt.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let previous_len = store.dismissed_prompt_history.len();
    store.dismissed_prompt_history.extend(prompts);
    store.dismissed_prompt_history.len() != previous_len
}

pub(super) fn reveal_prompt_history_entry(store: &mut Store, prompt: &str) -> bool {
    store.dismissed_prompt_history.remove(prompt.trim())
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

pub(super) fn negative_prompt_draft_for_category(
    drafts: &PromptDrafts,
    category: &str,
) -> String {
    match category {
        "scene" => drafts.negative_scene.clone(),
        "ui" => drafts.negative_ui.clone(),
        "effect" => drafts.negative_effect.clone(),
        "action-sequence" => drafts.negative_action_sequence.clone(),
        _ => drafts.negative_character.clone(),
    }
}

pub(super) fn set_negative_prompt_draft_for_category(
    drafts: &mut PromptDrafts,
    category: &str,
    prompt: String,
) {
    match category {
        "scene" => drafts.negative_scene = prompt,
        "ui" => drafts.negative_ui = prompt,
        "effect" => drafts.negative_effect = prompt,
        "action-sequence" => drafts.negative_action_sequence = prompt,
        _ => drafts.negative_character = prompt,
    }
}

pub(super) fn store_current_prompt_draft(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    category: &str,
) {
    let state = app.global::<AppState>();
    let prompt = state.get_prompt().to_string();
    let negative_prompt = state.get_negative_prompt().to_string();
    let mut store = store.borrow_mut();
    set_prompt_draft_for_category(&mut store.prompt_drafts, category, prompt);
    set_negative_prompt_draft_for_category(
        &mut store.prompt_drafts,
        category,
        negative_prompt,
    );
}

pub(super) fn sync_deep_prompt_binding_for_category(
    app: &AppWindow,
    store: &Store,
    category: &str,
) {
    let state = app.global::<AppState>();
    let visible_prompt = state.get_prompt().trim().to_string();
    if let Some(binding) = store.deep_prompt_bindings.get(category) {
        if !binding.english.trim().is_empty() && visible_prompt == binding.chinese.trim() {
            state.set_deep_optimization_applied_chinese(binding.chinese.clone().into());
            state.set_deep_optimization_applied_english(binding.english.clone().into());
            return;
        }
    }
    state.set_deep_optimization_applied_chinese("".into());
    state.set_deep_optimization_applied_english("".into());
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
    normalize_selected_custom_prompts(&mut store.selected_custom_prompts, &retained);
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
    for selected in store.selected_custom_prompts.values_mut() {
        selected.remove(prompt);
    }
    store
        .selected_custom_prompts
        .retain(|_, selected| !selected.is_empty());
    store.custom_prompt_times.remove(prompt);
    store.custom_prompt_profiles.remove(prompt);
    true
}

pub(super) fn toggle_custom_prompt_selection_for_category(
    store: &mut Store,
    category: &str,
    prompt: &str,
) {
    let category = resolve_category(category, "");
    let was_selected = store
        .selected_custom_prompts
        .get(&category)
        .is_some_and(|selected| selected.contains(prompt));
    if was_selected {
        store.selected_custom_prompts.remove(&category);
        return;
    }
    let selected = store.selected_custom_prompts.entry(category).or_default();
    selected.clear();
    selected.insert(prompt.to_string());
}

pub(super) fn custom_prompt_selected_for_category(
    store: &Store,
    category: &str,
    prompt: &str,
) -> bool {
    let category = resolve_category(category, "");
    store
        .selected_custom_prompts
        .get(&category)
        .is_some_and(|selected| selected.contains(prompt))
}

pub(super) fn selected_custom_prompts_for_category(store: &Store, category: &str) -> Vec<String> {
    store
        .custom_prompts
        .iter()
        .filter(|prompt| custom_prompt_selected_for_category(store, category, prompt))
        .cloned()
        .collect()
}

pub(super) fn replace_selected_custom_prompt(
    store: &mut Store,
    original_prompt: &str,
    replacement_prompt: &str,
) {
    for selected in store.selected_custom_prompts.values_mut() {
        if selected.remove(original_prompt) {
            selected.insert(replacement_prompt.to_string());
        }
    }
}

fn normalize_selected_custom_prompts(
    selected_by_category: &mut BTreeMap<String, BTreeSet<String>>,
    retained_prompts: &BTreeSet<String>,
) {
    let mut normalized = BTreeMap::<String, BTreeSet<String>>::new();
    for (category, mut selected) in std::mem::take(selected_by_category) {
        selected.retain(|prompt| retained_prompts.contains(prompt));
        if let Some(prompt) = selected.into_iter().next() {
            let normalized_selection = normalized
                .entry(resolve_category(&category, ""))
                .or_default();
            if normalized_selection.is_empty() {
                normalized_selection.insert(prompt);
            }
        }
    }
    *selected_by_category = normalized;
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
            origin: "local_recovery".to_string(),
            width,
            height,
            image,
            source_path: path.display().to_string(),
            reference_paths: vec![],
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
        dismissed_prompt_history: store.dismissed_prompt_history.clone(),
        custom_prompts: store.custom_prompts.clone(),
        selected_custom_prompts: store.selected_custom_prompts.clone(),
        custom_prompt_times: store.custom_prompt_times.clone(),
        custom_prompt_profiles: store.custom_prompt_profiles.clone(),
        canvas_notes: store.canvas_notes.clone(),
        canvas_links: store.canvas_links.clone(),
        deep_prompt_job_id: store.deep_prompt_job_id.clone(),
        deep_prompt_bindings: store.deep_prompt_bindings.clone(),
        contact_popup_dismissed: store.contact_popup_dismissed,
    };
    if let Ok(text) = serde_json::to_string_pretty(&data) {
        let path = local_store_path();
        let _ = replace_json_file(&path, &text);
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
        origin: asset.origin.clone(),
        width: asset.width,
        height: asset.height,
        source_path: asset.source_path.clone(),
        reference_paths: asset.reference_paths.clone(),
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
        origin: asset.origin,
        width: asset.width,
        height: asset.height,
        image,
        source_path: asset.source_path,
        reference_paths: asset.reference_paths,
        cutout_done: asset.cutout_done,
        remove_black_done: asset.remove_black_done,
        upscale_done: asset.upscale_done,
    })
}
