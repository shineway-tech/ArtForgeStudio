use super::*;

pub(super) fn open_viewer(app: &AppWindow, store: &Store, id: &str, source: &str) {
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

pub(super) fn estimated_prompt_lines(prompt: &str) -> i32 {
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

pub(super) fn move_viewer(app: &AppWindow, store: &Store, direction: i32) {
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

pub(super) fn viewer_ids(app: &AppWindow, store: &Store, source: &str) -> Vec<String> {
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

pub(super) fn navigate_to(app: &AppWindow, page: &str) {
    let state = app.global::<AppState>();
    if page != "welcome" && !state.get_logged_in() {
        state.set_auth_open(true);
        if state.get_auth_method().as_str() == "wechat"
            && !state.get_auth_wechat_busy()
            && !state.get_auth_wechat_qr_ready()
        {
            state.invoke_start_wechat_login();
        }
        return;
    }
    state.set_page(page.into());
}

pub(super) fn navigate_to_with_store(app: &AppWindow, store: &Store, page: &str) {
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

pub(super) fn push_all(app: &AppWindow, store: &Store) {
    push_model_groups(app, store);
    push_conversations(app, store);
    push_prompt_history(app, store);
    push_custom_prompts(app, store);
    push_canvas_notes(app, store);
    push_assets(app, store);
    push_generations(app, store);
    push_inspiration(app, store);
    push_notifications(app, store);
    push_references(app, store);
}

pub(super) fn recent_prompt_history<'a>(
    prompts: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let mut seen = BTreeSet::new();
    let mut history = Vec::new();
    for raw in prompts {
        let prompt = raw.trim();
        if prompt.is_empty() || !seen.insert(prompt.to_string()) {
            continue;
        }
        history.push(prompt.to_string());
        if history.len() == limit {
            break;
        }
    }
    history
}

pub(super) fn push_prompt_history(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let history = recent_prompt_history(
        store.generations.iter().map(|item| item.prompt.as_str()),
        20,
    );
    if history.is_empty() {
        state.set_prompt_history_open(false);
    }
    state.set_prompt_history_previews(ModelRc::new(VecModel::from(
        history
            .iter()
            .map(|prompt| SharedString::from(single_line_prompt_preview(prompt)))
            .collect::<Vec<_>>(),
    )));
    state.set_prompt_history(ModelRc::new(VecModel::from(
        history
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}

pub(super) fn push_custom_prompts(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    state.set_custom_prompt_items(ModelRc::new(VecModel::from(
        store
            .custom_prompts
            .iter()
            .map(|prompt| CustomPromptItem {
                content: prompt.clone().into(),
                time: store
                    .custom_prompt_times
                    .get(prompt)
                    .cloned()
                    .unwrap_or_default()
                    .into(),
            })
            .collect::<Vec<_>>(),
    )));
    state.set_custom_prompt_previews(ModelRc::new(VecModel::from(
        store
            .custom_prompts
            .iter()
            .map(|prompt| SharedString::from(single_line_prompt_preview(prompt)))
            .collect::<Vec<_>>(),
    )));
    state.set_custom_prompts(ModelRc::new(VecModel::from(
        store
            .custom_prompts
            .iter()
            .cloned()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}

pub(super) fn push_canvas_notes(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    state.set_canvas_notes(ModelRc::new(VecModel::from(
        store
            .canvas_notes
            .iter()
            .map(|note| CanvasNote {
                id: note.id.clone().into(),
                kind: note.kind.clone().into(),
                content: note.content.clone().into(),
                linked_input: canvas_linked_input(store, &note.id).into(),
                x: note.x,
                y: note.y,
                width: note.width,
                height: note.height,
                parent_group_id: note.parent_group_id.clone().into(),
                z_index: note.z_index,
                selected: note.selected,
            })
            .collect::<Vec<_>>(),
    )));
    state.set_canvas_links(ModelRc::new(VecModel::from(
        store
            .canvas_links
            .iter()
            .filter_map(|link| {
                let source = store
                    .canvas_notes
                    .iter()
                    .find(|note| note.id == link.source_id)?;
                let target = store
                    .canvas_notes
                    .iter()
                    .find(|note| note.id == link.target_id)?;
                Some(CanvasLink {
                    id: link.id.clone().into(),
                    source_id: link.source_id.clone().into(),
                    target_id: link.target_id.clone().into(),
                    start_x: source.x + source.width,
                    start_y: source.y + source.height / 2.0,
                    end_x: target.x,
                    end_y: target.y + target.height / 2.0,
                    source_selected: source.selected,
                    target_selected: target.selected,
                })
            })
            .collect::<Vec<_>>(),
    )));
}

fn canvas_linked_input(store: &Store, target_id: &str) -> String {
    let mut visiting = BTreeSet::new();
    let mut seen = BTreeSet::new();
    store
        .canvas_links
        .iter()
        .filter(|link| link.target_id == target_id)
        .filter_map(|link| resolved_canvas_content(store, &link.source_id, &mut visiting))
        .filter(|content| seen.insert(content.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn resolved_canvas_content(
    store: &Store,
    node_id: &str,
    visiting: &mut BTreeSet<String>,
) -> Option<String> {
    if !visiting.insert(node_id.to_string()) {
        return None;
    }
    let Some(note) = store.canvas_notes.iter().find(|note| note.id == node_id) else {
        visiting.remove(node_id);
        return None;
    };
    let mut seen = BTreeSet::new();
    let mut parts = store
        .canvas_links
        .iter()
        .filter(|link| link.target_id == node_id)
        .filter_map(|link| resolved_canvas_content(store, &link.source_id, visiting))
        .filter(|content| seen.insert(content.clone()))
        .collect::<Vec<_>>();
    let own = meaningful_canvas_content(note);
    if !own.is_empty() && seen.insert(own.to_string()) {
        parts.push(own.to_string());
    }
    visiting.remove(node_id);
    let resolved = parts.join("\n");
    (!resolved.is_empty()).then_some(resolved)
}

fn meaningful_canvas_content(note: &CanvasNoteData) -> &str {
    let content = note.content.trim();
    let placeholder = matches!(
        content,
        "描述要生成的图片内容"
            | "描述要生成的视频内容"
            | "描述要生成的音频内容"
            | "Describe the image you want to generate"
            | "Describe the video you want to generate"
            | "Describe the audio you want to generate"
    );
    if placeholder {
        ""
    } else {
        content
    }
}

pub(super) fn single_line_prompt_preview(prompt: &str) -> String {
    prompt.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn push_model_groups(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let image_options = model_picker_options(store, "image");
    let reasoning_options = model_picker_options(store, "reasoning");
    state.set_model_image_options(ModelRc::new(VecModel::from(image_options)));
    state.set_model_reasoning_options(ModelRc::new(VecModel::from(reasoning_options)));
    state.set_model_groups(ModelRc::new(VecModel::from(
        store
            .model_groups
            .iter()
            .map(to_model_group_view)
            .collect::<Vec<_>>(),
    )));
}

fn model_picker_options(store: &Store, kind: &str) -> Vec<ModelOption> {
    store
        .model_groups
        .iter()
        .filter(|group| group.kind == kind)
        .flat_map(|group| {
            group.models.iter().map(|model| ModelOption {
                code: model.code.clone().into(),
                name: format!("{} / {}", group.name, model.name).into(),
            })
        })
        .collect()
}

pub(super) fn push_conversations(app: &AppWindow, store: &Store) {
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

pub(super) fn push_assets(app: &AppWindow, store: &Store) {
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

pub(super) fn push_generations(app: &AppWindow, store: &Store) {
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

pub(super) fn push_inspiration(app: &AppWindow, store: &Store) {
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

pub(super) fn include_gallery_item(item: &AssetData, kind: &str, category: &str) -> bool {
    if kind != "all" && item.kind != kind {
        return false;
    }
    if category == "all" {
        return true;
    }
    item.category == category
}

pub(super) fn group_asset_views(items: &[&AssetData], language: &str) -> Vec<AssetGroup> {
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

pub(super) fn time_group_label(time: &str, language: &str) -> String {
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

pub(super) fn split_gallery_columns(items: Vec<&AssetData>) -> [Vec<AssetItem>; 5] {
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

pub(super) fn split_asset_row_columns(items: Vec<&AssetData>) -> [Vec<AssetItem>; 5] {
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

pub(super) fn gallery_height_score(item: &AssetData) -> i64 {
    if item.width <= 0 || item.height <= 0 {
        return 248;
    }
    ((item.height as i64) * 220 / (item.width as i64)).max(128)
}

pub(super) fn count_assets(store: &Store, category: &str) -> i32 {
    store
        .assets
        .iter()
        .filter(|item| item.kind == "game" && item.category == category)
        .count() as i32
}

pub(super) fn push_references(app: &AppWindow, store: &Store) {
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

pub(super) fn push_notifications(app: &AppWindow, store: &Store) {
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

pub(super) fn to_model_group_view(group: &ModelGroupData) -> ModelGroup {
    ModelGroup {
        kind: group.kind.clone().into(),
        name: group.name.clone().into(),
        models: ModelRc::new(VecModel::from(
            group
                .models
                .iter()
                .map(|model| ModelOption {
                    code: model.code.clone().into(),
                    name: model.name.clone().into(),
                })
                .collect::<Vec<_>>(),
        )),
        used_models: ModelRc::new(VecModel::from(
            normalized_used_models(
                group.used_models.clone(),
                &group
                    .models
                    .iter()
                    .map(|model| model.code.clone())
                    .collect::<Vec<_>>(),
            )
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
        )),
        selected_model: group.selected_model.clone().into(),
    }
}

pub(super) fn to_asset_view(asset: &AssetData) -> AssetItem {
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

#[cfg(test)]
mod canvas_link_tests {
    use super::*;

    fn note(id: &str, kind: &str, content: &str) -> CanvasNoteData {
        CanvasNoteData {
            id: id.to_string(),
            kind: kind.to_string(),
            content: content.to_string(),
            width: 320.0,
            height: 210.0,
            ..CanvasNoteData::default()
        }
    }

    #[test]
    fn connected_nodes_resolve_upstream_content_in_dependency_order() {
        let store = Store {
            canvas_notes: vec![
                note("brief", "text", "雨夜城市"),
                note("style", "text", "电影感霓虹灯"),
                note("image", "image", "描述要生成的图片内容"),
            ],
            canvas_links: vec![
                CanvasLinkData {
                    id: "one".to_string(),
                    source_id: "brief".to_string(),
                    target_id: "image".to_string(),
                },
                CanvasLinkData {
                    id: "two".to_string(),
                    source_id: "style".to_string(),
                    target_id: "image".to_string(),
                },
            ],
            ..Store::default()
        };

        assert_eq!(
            canvas_linked_input(&store, "image"),
            "雨夜城市\n电影感霓虹灯"
        );
        assert_eq!(meaningful_canvas_content(&store.canvas_notes[2]), "");
    }
}
