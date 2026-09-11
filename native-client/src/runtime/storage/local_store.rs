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
    use std::io::Write;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let (mut temporary_file, temporary) = create_atomic_temporary_file(path)?;
    if let Err(error) = temporary_file
        .write_all(text.as_bytes())
        .and_then(|_| temporary_file.sync_all())
    {
        drop(temporary_file);
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    drop(temporary_file);

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
            sync_parent_directory(path)?;
            let _ = fs::remove_file(backup);
            return Ok(());
        }
    }

    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    sync_parent_directory(path)?;
    Ok(())
}

pub(super) fn user_profile_path() -> PathBuf {
    app_data_dir().join("user-profile.json")
}

pub(super) fn load_user_profile(_app: &AppWindow) {
    // TEMP(team-accounts): removed in Task 10. No lease grants no private I/O.
}

pub(super) fn apply_user_profile(app: &AppWindow, profile: UserProfileData) {
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
    if !profile.asset_type.trim().is_empty() {
        let category = resolve_category(&profile.asset_type, "");
        state.set_asset_type(category.into());
    }
}

pub(super) fn save_user_profile(app: &AppWindow, store: &Store) {
    if save_user_profile_checked(app, store).is_err() {
        app.global::<AppState>().set_generation_status("个人设置未能安全保存，请重试".into());
    }
}

pub(super) fn save_user_profile_checked(app: &AppWindow, store: &Store) -> Result<()> {
    let authority = store.private_persistence.as_ref().ok_or_else(|| anyhow!("用户命名空间尚未激活"))?;
    authority.save_profile(user_profile_data(app))
}

pub(super) fn user_profile_data(app: &AppWindow) -> UserProfileData {
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
        asset_type: resolve_category(&state.get_asset_type().to_string(), ""),
    };
    profile
}

pub(super) fn normalize_gallery_layout(value: &str) -> &'static str {
    if value.trim().eq_ignore_ascii_case("waterfall") {
        "waterfall"
    } else {
        "grid"
    }
}

pub(super) fn local_store_path() -> PathBuf {
    app_data_dir().join("local-store.json")
}

fn archive_migrated_json(path: &Path) {
    if !path.is_file() {
        return;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("json");
    let archived = path.with_extension(format!("migrated.{extension}"));
    if !archived.exists() {
        let _ = fs::rename(path, archived);
    }
}

pub(super) fn load_local_store(_app: &AppWindow, _store: &Rc<RefCell<Store>>) -> bool {
    // TEMP(team-accounts): removed in Task 10.
    false
}

pub(super) struct PreparedPrivateStore {
    pub(super) store: Store,
    pub(super) image_model: String,
    pub(super) reasoning_model: String,
    pub(super) video_model: String,
}

/// Preparation is memory-only. Missing media remains represented, never deleted
/// from the user's model merely because a path cannot currently be opened.
pub(super) fn prepare_private_store(data: LocalStoreData) -> PreparedPrivateStore {
    let store = Rc::new(RefCell::new(Store::default()));
    let saved_image_model = data.image_model.clone();
    let saved_reasoning_model = data.reasoning_model.clone();
    let saved_video_model = data.video_model.clone();
    let _normalized = {
        let mut store_mut = store.borrow_mut();
        // Legacy provider endpoints and API keys are intentionally ignored.
        store_mut.model_groups.clear();
        store_mut.assets = data
            .assets
            .into_iter()
            .map(asset_from_stored_preserved)
            .collect();
        store_mut.generations = data
            .generations
            .into_iter()
            .map(asset_from_stored_preserved)
            .collect();
        store_mut.notifications = data.notifications;
        store_mut.references = data.references;
        store_mut.video_outputs = data.video_outputs;
        store_mut.prompt_drafts = data.prompt_drafts;
        store_mut.dismissed_prompt_history = data.dismissed_prompt_history;
        let migrated_prompt_drafts = normalize_reserved_prompt_drafts(&mut store_mut.prompt_drafts);
        store_mut.custom_prompts = normalize_custom_prompts(data.custom_prompts);
        store_mut.selected_custom_prompts = data.selected_custom_prompts;
        store_mut.custom_prompt_times = data.custom_prompt_times;
        store_mut.custom_prompt_profiles = data.custom_prompt_profiles;
        let active_canvas_workspace_id =
            normalize_canvas_workspace_id(&data.active_canvas_workspace_id);
        let migrated_canvas_workspaces = data.active_canvas_workspace_id.trim().is_empty()
            || !data
                .canvas_workspaces
                .contains_key(&active_canvas_workspace_id);
        store_mut.active_canvas_workspace_id = active_canvas_workspace_id.clone();
        store_mut.canvas_workspaces = data.canvas_workspaces;
        let migrated_canvas_prompt_text =
            normalize_canvas_workspace_prompts(&mut store_mut.canvas_workspaces);
        store_mut.canvas_notes = data.canvas_notes;
        normalize_canvas_groups(&mut store_mut.canvas_notes);
        let fitted_canvas_groups = fit_groups_to_children(&mut store_mut.canvas_notes);
        store_mut.canvas_links = data.canvas_links;
        let active_prompt = store_mut
            .canvas_workspaces
            .get(&active_canvas_workspace_id)
            .map(|workspace| workspace.prompt.clone())
            .unwrap_or_default();
        let active_canvas_references = store_mut
            .canvas_workspaces
            .get(&active_canvas_workspace_id)
            .map(|workspace| workspace.references.clone())
            .unwrap_or_default();
        store_mut.canvas_references = active_canvas_references.clone();
        let active_canvas_notes = store_mut.canvas_notes.clone();
        let active_canvas_links = store_mut.canvas_links.clone();
        store_mut.canvas_workspaces.insert(
            active_canvas_workspace_id,
            CanvasWorkspaceData {
                notes: active_canvas_notes,
                links: active_canvas_links,
                prompt: active_prompt,
                references: active_canvas_references,
            },
        );
        store_mut.deep_prompt_jobs_by_owner = data
            .deep_prompt_jobs_by_owner
            .into_iter()
            .filter_map(|(owner_user_id, job_id)| {
                let owner_user_id = owner_user_id.trim().to_string();
                let job_id = job_id.trim().to_string();
                (!owner_user_id.is_empty() && !job_id.is_empty())
                    .then_some((owner_user_id, job_id))
            })
            .collect();
        store_mut.deep_prompt_pending_requests_by_owner = data
            .deep_prompt_pending_requests_by_owner
            .into_iter()
            .filter_map(|(owner_user_id, request)| {
                let owner_user_id = owner_user_id.trim().to_string();
                let valid = !owner_user_id.is_empty()
                    && !request.client_request_id.trim().is_empty()
                    && !request.prompt.trim().is_empty();
                valid.then_some((owner_user_id, request))
            })
            .collect();
        store_mut.legacy_deep_prompt_job_id = data.deep_prompt_job_id.trim().to_string();
        store_mut.deep_prompt_bindings = data.deep_prompt_bindings;
        store_mut.pending_credit_redemptions_by_owner = data.pending_credit_redemptions_by_owner;
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
            || migrated_canvas_workspaces
            || migrated_canvas_prompt_text
            || fitted_canvas_groups
            || store_mut.custom_prompt_times != original_prompt_times
    };
    let store = match Rc::try_unwrap(store) { Ok(store) => store.into_inner(), Err(_) => unreachable!("private preparation never shares its Store") };
    PreparedPrivateStore { store, image_model: saved_image_model, reasoning_model: saved_reasoning_model, video_model: saved_video_model }
}

fn apply_local_store_data(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mut data: LocalStoreData,
) -> bool {
    directory_locations().remap_local_store(&mut data);
    let saved_image_model = data.image_model.clone();
    let saved_reasoning_model = data.reasoning_model.clone();
    let saved_video_model = data.video_model.clone();
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
        let active_canvas_workspace_id =
            normalize_canvas_workspace_id(&data.active_canvas_workspace_id);
        let migrated_canvas_workspaces = data.active_canvas_workspace_id.trim().is_empty()
            || !data
                .canvas_workspaces
                .contains_key(&active_canvas_workspace_id);
        store_mut.active_canvas_workspace_id = active_canvas_workspace_id.clone();
        store_mut.canvas_workspaces = data.canvas_workspaces;
        let migrated_canvas_prompt_text =
            normalize_canvas_workspace_prompts(&mut store_mut.canvas_workspaces);
        store_mut.canvas_notes = data.canvas_notes;
        normalize_canvas_groups(&mut store_mut.canvas_notes);
        let fitted_canvas_groups = fit_groups_to_children(&mut store_mut.canvas_notes);
        store_mut.canvas_links = data.canvas_links;
        let active_prompt = store_mut
            .canvas_workspaces
            .get(&active_canvas_workspace_id)
            .map(|workspace| workspace.prompt.clone())
            .unwrap_or_default();
        let active_canvas_references = store_mut
            .canvas_workspaces
            .get(&active_canvas_workspace_id)
            .map(|workspace| workspace.references.clone())
            .unwrap_or_default();
        store_mut.canvas_references = active_canvas_references.clone();
        let active_canvas_notes = store_mut.canvas_notes.clone();
        let active_canvas_links = store_mut.canvas_links.clone();
        store_mut.canvas_workspaces.insert(
            active_canvas_workspace_id,
            CanvasWorkspaceData {
                notes: active_canvas_notes,
                links: active_canvas_links,
                prompt: active_prompt,
                references: active_canvas_references,
            },
        );
        store_mut.deep_prompt_jobs_by_owner = data
            .deep_prompt_jobs_by_owner
            .into_iter()
            .filter_map(|(owner_user_id, job_id)| {
                let owner_user_id = owner_user_id.trim().to_string();
                let job_id = job_id.trim().to_string();
                (!owner_user_id.is_empty() && !job_id.is_empty())
                    .then_some((owner_user_id, job_id))
            })
            .collect();
        store_mut.deep_prompt_pending_requests_by_owner = data
            .deep_prompt_pending_requests_by_owner
            .into_iter()
            .filter_map(|(owner_user_id, request)| {
                let owner_user_id = owner_user_id.trim().to_string();
                let valid = !owner_user_id.is_empty()
                    && !request.client_request_id.trim().is_empty()
                    && !request.prompt.trim().is_empty();
                valid.then_some((owner_user_id, request))
            })
            .collect();
        store_mut.legacy_deep_prompt_job_id = data.deep_prompt_job_id.trim().to_string();
        store_mut.deep_prompt_bindings = data.deep_prompt_bindings;
        store_mut.pending_credit_redemptions_by_owner = data.pending_credit_redemptions_by_owner;
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
            || migrated_canvas_workspaces
            || migrated_canvas_prompt_text
            || fitted_canvas_groups
            || store_mut.custom_prompt_times != original_prompt_times
    };
    let state = app.global::<AppState>();
    state.set_contact_popup_open(!store.borrow().contact_popup_dismissed);
    state.set_image_model(saved_image_model.into());
    state.set_reasoning_model(saved_reasoning_model.into());
    state.set_video_model(saved_video_model.into());
    let active_canvas_prompt = {
        let store = store.borrow();
        store
            .canvas_workspaces
            .get(&store.active_canvas_workspace_id)
            .map(|workspace| workspace.prompt.clone())
            .unwrap_or_default()
    };
    state.set_canvas_workflow_prompt(active_canvas_prompt.into());
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
    true
}

pub(super) fn normalize_reserved_prompt_drafts(drafts: &mut PromptDrafts) -> bool {
    let mut migrated = false;
    for prompt in [
        &mut drafts.character,
        &mut drafts.scene,
        &mut drafts.ui,
        &mut drafts.effect,
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
        _ => drafts.character = prompt,
    }
}

pub(super) fn negative_prompt_draft_for_category(drafts: &PromptDrafts, category: &str) -> String {
    match category {
        "scene" => drafts.negative_scene.clone(),
        "ui" => drafts.negative_ui.clone(),
        "effect" => drafts.negative_effect.clone(),
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
    set_negative_prompt_draft_for_category(&mut store.prompt_drafts, category, negative_prompt);
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
        if let Some(selected) = store.selected_custom_prompts.get_mut(&category) {
            selected.remove(prompt);
            if selected.is_empty() {
                store.selected_custom_prompts.remove(&category);
            }
        }
        return;
    }
    let selected = store.selected_custom_prompts.entry(category).or_default();
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

pub(super) fn custom_prompt_display_name(store: &Store, prompt: &str) -> String {
    store
        .custom_prompt_profiles
        .get(prompt)
        .map(|profile| profile.name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| single_line_prompt_preview(prompt).chars().take(48).collect())
}

pub(super) fn selected_custom_prompt_replacements_for_category(
    store: &Store,
    category: &str,
) -> Vec<(String, String)> {
    selected_custom_prompts_for_category(store, category)
        .into_iter()
        .map(|prompt| (custom_prompt_display_name(store, &prompt), prompt))
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
        if !selected.is_empty() {
            normalized
                .entry(resolve_category(&category, ""))
                .or_default()
                .extend(selected);
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
        _ => &mut references.character,
    }
}

pub(super) fn normalize_settings_font_size(value: i32) -> i32 {
    value.clamp(10, 24)
}

pub(super) fn normalize_close_behavior(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "exit" => "exit",
        "tray" => "tray",
        _ => "ask",
    }
}

pub(super) fn references_for_context<'a>(
    store: &'a Store,
    category: &str,
    canvas: bool,
) -> &'a Vec<ReferenceData> {
    if canvas {
        &store.canvas_references
    } else {
        references_for_category(&store.references, category)
    }
}

pub(super) fn references_for_context_mut<'a>(
    store: &'a mut Store,
    category: &str,
    canvas: bool,
) -> &'a mut Vec<ReferenceData> {
    if canvas {
        &mut store.canvas_references
    } else {
        references_for_category_mut(&mut store.references, category)
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
            source_path: path.display().to_string(),
            reference_paths: vec![],
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable: false,
            delivery_downloading: false,
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
    if save_local_store_checked(app, store).is_err() {
        app.global::<AppState>().set_generation_status("创作状态未能安全保存，请重试".into());
    }
}

pub(super) fn save_local_store_checked(app: &AppWindow, store: &Store) -> Result<()> {
    let authority = store.private_persistence.as_ref().ok_or_else(|| anyhow!("用户命名空间尚未激活"))?;
    authority.save_store(local_store_data(app, store))
}

pub(super) fn save_local_store_checked_for_namespace(
    app: &AppWindow,
    store: &Store,
    writer: &ClientStateWriter,
    lease: &NamespaceLease,
) -> Result<()> {
    writer
        .persist_client_state_checked_for_namespace(lease, local_store_data(app, store))
        .map_err(Into::into)
}

pub(super) fn local_store_data(app: &AppWindow, store: &Store) -> LocalStoreData {
    let state = app.global::<AppState>();
    let active_canvas_workspace_id =
        normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    let mut canvas_workspaces = store.canvas_workspaces.clone();
    normalize_canvas_workspace_prompts(&mut canvas_workspaces);
    canvas_workspaces.insert(
        active_canvas_workspace_id.clone(),
        CanvasWorkspaceData {
            notes: store.canvas_notes.clone(),
            links: store.canvas_links.clone(),
            prompt: normalize_canvas_workflow_prompt(
                state.get_canvas_workflow_prompt().as_str(),
            ),
            references: store.canvas_references.clone(),
        },
    );
    LocalStoreData {
        video_outputs: store.video_outputs.clone(),
        references: store.references.clone(),
        pending_credit_redemptions_by_owner: store.pending_credit_redemptions_by_owner.clone(),
        generations: store.generations.iter().map(stored_asset_from).collect(),
        assets: store.assets.iter().map(stored_asset_from).collect(),
        notifications: store.notifications.clone(),
        image_model: state.get_image_model().to_string(),
        reasoning_model: state.get_reasoning_model().to_string(),
        video_model: state.get_video_model().to_string(),
        prompt_drafts: store.prompt_drafts.clone(),
        dismissed_prompt_history: store.dismissed_prompt_history.clone(),
        custom_prompts: store.custom_prompts.clone(),
        selected_custom_prompts: store.selected_custom_prompts.clone(),
        custom_prompt_times: store.custom_prompt_times.clone(),
        custom_prompt_profiles: store.custom_prompt_profiles.clone(),
        canvas_notes: store.canvas_notes.clone(),
        canvas_links: store.canvas_links.clone(),
        active_canvas_workspace_id,
        canvas_workspaces,
        deep_prompt_job_id: store.legacy_deep_prompt_job_id.clone(),
        deep_prompt_jobs_by_owner: store.deep_prompt_jobs_by_owner.clone(),
        deep_prompt_pending_requests_by_owner: store
            .deep_prompt_pending_requests_by_owner
            .clone(),
        deep_prompt_bindings: store.deep_prompt_bindings.clone(),
        contact_popup_dismissed: store.contact_popup_dismissed,
    }
}

pub(super) fn persist_generated_asset_checked(
    app: &AppWindow,
    store: &mut Store,
    item: AssetData,
    notification: NotificationData,
    include_in_generations: bool,
    reveal_prompt: Option<&str>,
) -> Result<()> {
    persist_generated_asset_with(
        store,
        item,
        notification,
        include_in_generations,
        reveal_prompt,
        |store| save_local_store_checked(app, store),
    )
}

pub(super) fn persist_generated_asset_checked_for_namespace(
    app: &AppWindow,
    store: &mut Store,
    writer: &ClientStateWriter,
    lease: &NamespaceLease,
    item: AssetData,
    notification: NotificationData,
    include_in_generations: bool,
    reveal_prompt: Option<&str>,
) -> Result<()> {
    persist_generated_asset_with(
        store,
        item,
        notification,
        include_in_generations,
        reveal_prompt,
        |pending| save_local_store_checked_for_namespace(app, pending, writer, lease),
    )
}

fn persist_generated_asset_with<F>(
    store: &mut Store,
    item: AssetData,
    notification: NotificationData,
    include_in_generations: bool,
    reveal_prompt: Option<&str>,
    persist: F,
) -> Result<()>
where
    F: FnOnce(&Store) -> Result<()>,
{
    let item_id = item.id.clone();
    let notification_id = notification.id.clone();
    let revealed_prompt = reveal_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .map(str::to_string);
    let restore_dismissed_prompt = revealed_prompt
        .as_ref()
        .is_some_and(|prompt| store.dismissed_prompt_history.contains(prompt));

    if let Some(prompt) = revealed_prompt.as_deref() {
        reveal_prompt_history_entry(store, prompt);
    }
    store.assets.insert(0, item.clone());
    if include_in_generations {
        store.generations.insert(0, item);
    }
    store.notifications.insert(0, notification);

    if let Err(error) = persist(store) {
        remove_front_asset_if_matches(&mut store.assets, &item_id);
        if include_in_generations {
            remove_front_asset_if_matches(&mut store.generations, &item_id);
        }
        if store
            .notifications
            .first()
            .is_some_and(|item| item.id == notification_id)
        {
            store.notifications.remove(0);
        }
        if restore_dismissed_prompt {
            if let Some(prompt) = revealed_prompt {
                store.dismissed_prompt_history.insert(prompt);
            }
        }
        return Err(error);
    }

    Ok(())
}

fn remove_front_asset_if_matches(items: &mut Vec<AssetData>, expected_id: &str) {
    if items
        .first()
        .is_some_and(|item| item.id == expected_id)
    {
        items.remove(0);
    }
}

pub(super) fn replace_failed_delivery_asset_with<F>(
    store: &mut Store,
    failed_asset_id: &str,
    completed_asset: AssetData,
    notification: NotificationData,
    persist: F,
) -> Result<()>
where
    F: FnOnce(&Store) -> Result<()>,
{
    if failed_asset_id.trim().is_empty()
        || completed_asset.id != failed_asset_id
        || completed_asset.source_path == "failed"
        || completed_asset.source_path.trim().is_empty()
    {
        anyhow::bail!("failed delivery replacement metadata is invalid");
    }
    let matching_generation_indexes = store
        .generations
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (item.id == failed_asset_id).then_some(index))
        .collect::<Vec<_>>();
    let [generation_index] = matching_generation_indexes.as_slice() else {
        anyhow::bail!("failed delivery card is missing or ambiguous");
    };
    if store.generations[*generation_index].source_path != "failed"
        || store
            .assets
            .iter()
            .any(|item| item.id == failed_asset_id)
    {
        anyhow::bail!("failed delivery card cannot be replaced safely");
    }

    let failed_card = std::mem::replace(
        &mut store.generations[*generation_index],
        completed_asset.clone(),
    );
    let notification_id = notification.id.clone();
    store.assets.insert(0, completed_asset);
    store.notifications.insert(0, notification);

    if let Err(error) = persist(store) {
        store.generations[*generation_index] = failed_card;
        remove_front_asset_if_matches(&mut store.assets, failed_asset_id);
        if store
            .notifications
            .first()
            .is_some_and(|item| item.id == notification_id)
        {
            store.notifications.remove(0);
        }
        return Err(error);
    }

    Ok(())
}

pub(super) fn replace_failed_delivery_asset_checked_for_namespace(
    app: &AppWindow,
    store: &mut Store,
    writer: &ClientStateWriter,
    lease: &NamespaceLease,
    failed_asset_id: &str,
    completed_asset: AssetData,
    notification: NotificationData,
) -> Result<()> {
    replace_failed_delivery_asset_with(
        store,
        failed_asset_id,
        completed_asset,
        notification,
        |pending| save_local_store_checked_for_namespace(app, pending, writer, lease),
    )
}

#[cfg(test)]
mod generated_asset_persistence_tests {
    use super::*;

    fn asset(id: &str, prompt: &str) -> AssetData {
        AssetData {
            id: id.to_string(),
            conversation_id: "conversation".to_string(),
            title: "generated".to_string(),
            category: "other".to_string(),
            kind: "game".to_string(),
            time: "now".to_string(),
            prompt: prompt.to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model: "test".to_string(),
            origin: "test".to_string(),
            width: 1,
            height: 1,
            source_path: "/tmp/generated.png".to_string(),
            reference_paths: vec![],
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable: false,
            delivery_downloading: false,
        }
    }

    fn notification(id: &str) -> NotificationData {
        NotificationData {
            id: id.to_string(),
            title: "complete".to_string(),
            model: "test".to_string(),
            time: "now".to_string(),
            reason: String::new(),
            success: true,
            read: false,
        }
    }

    #[test]
    fn failed_checked_save_rolls_back_asset_notification_generation_and_prompt_reveal() {
        let mut store = Store::default();
        store.dismissed_prompt_history.insert("paid prompt".to_string());

        let result = persist_generated_asset_with(
            &mut store,
            asset("asset-1", "paid prompt"),
            notification("notification-1"),
            true,
            Some("paid prompt"),
            |pending| {
                assert_eq!(pending.assets.len(), 1);
                assert_eq!(pending.generations.len(), 1);
                assert_eq!(pending.notifications.len(), 1);
                assert!(!pending.dismissed_prompt_history.contains("paid prompt"));
                Err(anyhow::anyhow!("disk full"))
            },
        );

        assert!(result.is_err());
        assert!(store.assets.is_empty());
        assert!(store.generations.is_empty());
        assert!(store.notifications.is_empty());
        assert!(store.dismissed_prompt_history.contains("paid prompt"));
    }

    #[test]
    fn successful_checked_save_keeps_an_asset_only_delivery_once() {
        let mut store = Store::default();

        persist_generated_asset_with(
            &mut store,
            asset("asset-1", "tool result"),
            notification("notification-1"),
            false,
            None,
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(store.assets.len(), 1);
        assert!(store.generations.is_empty());
        assert_eq!(store.notifications.len(), 1);
    }

    #[test]
    fn every_remote_generation_result_uses_checked_asset_persistence_before_ack() {
        // Historical structural guard, not a claim that ordinary/Canvas output
        // has migrated to the namespace ordered-delivery protocol. That remains OPEN.
        fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
            source.split_once(start).expect("production entry exists").1
                .split_once(end).expect("next production boundary exists").0
        }
        let controller = include_str!("../generation/controller.rs");
        let generation_poll = include_str!("../generation/poll.rs");
        let delivery = include_str!("../generation/delivery_retry.rs");
        let cutout = include_str!("../callbacks/image_cutout.rs");
        let enhancement = include_str!("../callbacks/image_enhancement.rs");
        let toolbox = include_str!("../callbacks/toolbox.rs");

        // Keep the original ordinary-save requirements; do not substitute the
        // newer D/E Prepared branch for the still-existing ImageSuccess consumer.
        let ordinary = controller
            .split("pub(super) fn add_stream_success_item")
            .nth(1)
            .and_then(|value| {
                value
                    .split("pub(super) fn add_canvas_stream_success_item")
                    .next()
            })
            .unwrap();
        for save in [ordinary] {
            assert!(save.contains("persist_generated_asset_checked("));
            assert!(!save.contains("save_local_store(app"));
        }
        let ordinary_flow = section(generation_poll, "GenerationOutcome::ImageSuccess {", "GenerationOutcome::ImageFailure {");
        assert!(ordinary_flow.contains("add_canvas_stream_success_item("));
        assert!(ordinary_flow.contains("replace_failed_delivery_asset_checked("));
        for (flow, save_call) in [(ordinary_flow, "add_stream_success_item(")] {
            assert!(flow.find(save_call).unwrap() < flow.find("pending_delivery_saved(").unwrap());
        }
        assert!(ordinary_flow.find("match saved_result {").unwrap()
            < ordinary_flow.find("pending_delivery_saved_then_acknowledge_with(").unwrap());
        assert!(ordinary_flow.find("pending_delivery_saved(").unwrap()
            < ordinary_flow.find("acknowledge_delivery_after_local_save(").unwrap());

        // Replaced toolbox save functions now forward the owned proof to one
        // shared commit boundary; their callbacks cannot use unchecked Store saves.
        let cutout_save = section(cutout, "fn finish_cutout_work(", "fn cutout_worker_current(");
        let enhancement_save = section(enhancement, "fn finish_enhancement_work(", "fn enhancement_worker_current(");
        let toolbox_save = section(toolbox, "fn enqueue_toolbox_remote_delivery(", "#[derive(Clone, Copy)]");
        for save in [cutout_save, enhancement_save, toolbox_save] {
            assert!(save.contains("start_image_delivery_commit("));
            assert!(!save.contains("save_local_store(app"));
            assert!(!save.contains("acknowledge_delivery_scoped("));
        }
        for (poll, end, kind) in [
            ("fn poll_watermark_outcomes(", "fn start_image_colorization(", "ToolboxRemoteKind::Watermark"),
            ("fn poll_image_colorization_outcomes(", "#[cfg(test)]", "ToolboxRemoteKind::Colorization"),
        ] {
            let flow = section(toolbox, poll, end);
            let prepared = section(flow, "ToolboxRemoteOutcome::Prepared(prepared) =>", "ToolboxRemoteOutcome::CreditInsufficient");
            assert!(prepared.contains("enqueue_toolbox_remote_delivery("));
            assert!(prepared.contains("persistence.clone(),\n                    *prepared,"));
            assert!(prepared.contains(kind));
            assert!(!prepared.contains("save_local_store(app"));
        }

        let commit = section(controller, "pub(super) fn start_image_delivery_commit(", "fn poll_image_delivery_commit(");
        assert!(commit.find("persistence.prepare_ordered_save()?").unwrap()
            < commit.find("enqueue_delivery(").unwrap());
        assert!(commit.find("enqueue_delivery(").unwrap() < commit.find("pending.wait()").unwrap());
        let ack_worker = section(commit, ".name(\"delivery-store-ack\".into()).spawn(move||{", "\n    match spawned {");
        assert!(ack_worker.find("pending.wait().map_err(DeliveryRetryError::from).and_then(|receipt|").unwrap()
            < ack_worker.find("acknowledge_namespace_delivery(receipt)").unwrap());
        let terminal = "Err(DeliveryRetryError::Api(error)) if error.is_terminal_session_error()=>Err(error.into())";
        assert!(ack_worker.find(terminal).unwrap() < ack_worker.find("Err(_)=>Ok(false)").unwrap());
        assert!(ack_worker.find("acknowledge_namespace_delivery(receipt)").unwrap()
            < ack_worker.find("sender.send(result)").unwrap());
        let enqueue = section(controller, "pub(super) fn enqueue_delivery(", "fn stage_namespace_delivery(");
        assert!(enqueue.find("stage_namespace_delivery(store,&prepared,time)").unwrap()
            < enqueue.find("self.enqueue(local_store_data(app,store))").unwrap());
        assert!(enqueue.contains("Ok(receiver)=>Ok(PendingNamespaceDeliveryCommit {receiver,proof:prepared.into_proof(),asset_id})"));
        assert!(enqueue.contains("Err(error)=>Err(GuardedDeliveryEnqueueError"));
        assert!(!enqueue.contains("save_local_store(app"));
        // Queue/ack failure retains idempotently staged output, not an unsafe
        // rollback of whatever newer full Store snapshot may already exist.
        assert!(!enqueue.contains("store.assets.remove(0)"));
        assert!(!enqueue.contains("store.notifications.remove(0)"));
        let waited = section(controller, "impl PendingNamespaceDeliveryCommit {", "pub(super) enum DeliveryEnqueueOwnership");
        assert!(waited.find("self.receiver.recv().map_err(").unwrap()
            < waited.find("Ok(CommittedNamespaceDelivery { prepared: self.proof })").unwrap());
        assert!(waited.contains("delivery writer acknowledgment disconnected\"))??;"));
        let acknowledged = section(delivery, "pub(super) fn acknowledge_namespace_delivery(", "impl NamespaceDeliveryProof {");
        for check in ["prepared.ensure_current()?", "verify_namespace_delivery_file(", "prepared.ensure_index_current()?"] {
            assert!(acknowledged.find(check).unwrap()
                < acknowledged.find("prepared.api.acknowledge_delivery_scoped(").unwrap());
        }
        assert!(acknowledged.find("pending_delivery_saved_for_namespace(").unwrap()
            < acknowledged.find("prepared.api.acknowledge_delivery_scoped(").unwrap());
        assert!(acknowledged.contains("&prepared.confirmation.sha256,\n        prepared.confirmation.size_bytes,"));
        assert!(acknowledged.find("prepared.api.acknowledge_delivery_scoped(").unwrap()
            < acknowledged.find("pending_delivery_acknowledged_for_namespace(").unwrap());
    }
}

#[cfg(test)]
mod failed_card_replacement_tests {
    use super::*;

    fn asset(id: &str, source_path: &str) -> AssetData {
        AssetData {
            id: id.to_string(),
            conversation_id: "conversation".to_string(),
            title: "generated".to_string(),
            category: "other".to_string(),
            kind: "game".to_string(),
            time: "now".to_string(),
            prompt: "paid prompt".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model: "test".to_string(),
            origin: "generation".to_string(),
            width: 1,
            height: 1,
            source_path: source_path.to_string(),
            reference_paths: vec![],
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: source_path != "failed",
            delivery_recoverable: source_path == "failed",
            delivery_downloading: false,
        }
    }

    fn notification() -> NotificationData {
        NotificationData {
            id: "notification-1".to_string(),
            title: "图片下载完成：paid prompt".to_string(),
            model: "test".to_string(),
            time: "now".to_string(),
            reason: String::new(),
            success: true,
            read: false,
        }
    }

    #[test]
    fn failed_card_replacement_preserves_position_and_adds_one_notification() {
        let mut store = Store::default();
        store.generations.push(asset("before", "/saved/before.png"));
        store.generations.push(asset("failed-1", "failed"));
        store.generations.push(asset("after", "/saved/after.png"));

        replace_failed_delivery_asset_with(
            &mut store,
            "failed-1",
            asset("failed-1", "/saved/image.png"),
            notification(),
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(store.generations[1].id, "failed-1");
        assert_eq!(store.generations[1].source_path, "/saved/image.png");
        assert_eq!(
            store
                .assets
                .iter()
                .filter(|item| item.id == "failed-1")
                .count(),
            1
        );
        assert_eq!(store.notifications.len(), 1);
    }

    #[test]
    fn failed_card_replacement_rolls_back_every_change_when_persistence_fails() {
        let mut store = Store::default();
        store.generations.push(asset("failed-1", "failed"));

        let result = replace_failed_delivery_asset_with(
            &mut store,
            "failed-1",
            asset("failed-1", "/saved/image.png"),
            notification(),
            |pending| {
                assert_eq!(pending.generations[0].source_path, "/saved/image.png");
                assert_eq!(pending.assets.len(), 1);
                assert_eq!(pending.notifications.len(), 1);
                Err(anyhow!("disk full"))
            },
        );

        assert!(result.is_err());
        assert_eq!(store.generations[0].source_path, "failed");
        assert!(store.assets.is_empty());
        assert!(store.notifications.is_empty());
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
    if asset.source_path != "failed"
        && (asset.source_path.trim().is_empty() || !Path::new(&asset.source_path).is_file())
    {
        return None;
    }
    Some(asset_from_stored_preserved(asset))
}

fn asset_from_stored_preserved(asset: StoredAssetData) -> AssetData {
    AssetData {
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
        source_path: asset.source_path,
        reference_paths: asset.reference_paths,
        cutout_done: asset.cutout_done,
        remove_black_done: asset.remove_black_done,
        upscale_done: asset.upscale_done,
        is_new: false,
        delivery_recoverable: false,
        delivery_downloading: false,
    }
}

pub(super) fn load_device_settings_into_app(app: &AppWindow) -> Result<()> {
    apply_device_settings(app, load_device_settings()?.unwrap_or_default());
    Ok(())
}

pub(super) fn apply_device_settings(app: &AppWindow, settings: DeviceSettings) {
    let settings = settings.normalized();
    let state = app.global::<AppState>();
    state.set_theme_id(settings.theme_id.clone().into());
    apply_theme(app, &settings.theme_id);
    state.set_card_style(settings.card_style.into());
    state.set_language(settings.language.into());
    state.set_close_behavior(settings.close_behavior.into());
    state.set_settings_font_family(settings.font_family.into());
    state.set_settings_font_size(settings.font_size);
    state.set_generation_gallery_layout(settings.generation_gallery_layout.into());
    state.set_asset_gallery_layout(settings.asset_gallery_layout.into());
    state.set_inspiration_gallery_layout(settings.inspiration_gallery_layout.into());
}

fn device_settings_data(app: &AppWindow) -> DeviceSettings {
    let state = app.global::<AppState>();
    DeviceSettings {
        theme_id: state.get_theme_id().to_string(),
        card_style: state.get_card_style().to_string(),
        language: state.get_language().to_string(),
        close_behavior: normalize_close_behavior(&state.get_close_behavior()).into(),
        font_family: state.get_settings_font_family().to_string(),
        font_size: normalize_settings_font_size(state.get_settings_font_size()),
        generation_gallery_layout: state.get_generation_gallery_layout().to_string(),
        asset_gallery_layout: state.get_asset_gallery_layout().to_string(),
        inspiration_gallery_layout: state.get_inspiration_gallery_layout().to_string(),
    }
    .normalized()
}

pub(super) fn save_device_settings(app: &AppWindow) {
    let _ = persist_device_settings_async(device_settings_data(app));
}

pub(super) fn save_device_settings_checked(app: &AppWindow) -> Result<()> {
    persist_device_settings_checked(device_settings_data(app))
}

#[cfg(test)]
mod device_profile_isolation_tests {
    use super::*;

    #[test]
    fn applying_a_b_a_private_profiles_cannot_overwrite_device_presentation() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let settings = DeviceSettings {
            theme_id: "dark".into(),
            card_style: "square".into(),
            language: "en".into(),
            close_behavior: "tray".into(),
            font_family: "Microsoft YaHei UI".into(),
            font_size: 18,
            generation_gallery_layout: "waterfall".into(),
            asset_gallery_layout: "waterfall".into(),
            inspiration_gallery_layout: "grid".into(),
        };
        apply_device_settings(&app, settings.clone());
        for nickname in ["Alice", "Bob", "Alice"] {
            apply_user_profile(
                &app,
                UserProfileData {
                    nickname: nickname.into(),
                    asset_type: "scene".into(),
                    ..Default::default()
                },
            );
            assert_eq!(app.global::<AppState>().get_nickname(), nickname);
            assert_eq!(device_settings_data(&app), settings);
            let profile = serde_json::to_value(user_profile_data(&app)).unwrap();
            for key in KNOWN_DEVICE_SETTING_KEYS {
                assert!(profile.get(key).is_none());
            }
            assert!(profile.get("ui_preferences").is_none());
        }
    }

    #[test]
    fn no_lease_local_store_entrypoints_are_no_io_and_do_not_apply_private_state() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let store = Rc::new(RefCell::new(Store::default()));
        app.global::<AppState>().set_nickname("Current".into());
        store.borrow_mut().custom_prompts = vec!["current private draft".into()];
        load_user_profile(&app);
        assert!(!load_local_store(&app, &store));
        save_user_profile(&app, &store.borrow());
        save_local_store(&app, &store.borrow());
        assert!(save_user_profile_checked(&app, &store.borrow()).is_err());
        assert!(save_local_store_checked(&app, &store.borrow()).is_err());
        assert_eq!(app.global::<AppState>().get_nickname(), "Current");
        assert_eq!(store.borrow().custom_prompts, ["current private draft"]);
    }
}
