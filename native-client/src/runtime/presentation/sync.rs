use super::*;

const GALLERY_PAGE_SIZE: i32 = 24;
const GALLERY_OVERSCAN_SCREENS: f32 = 1.0;
const MAX_GALLERY_WINDOW_ITEMS: usize = 192;
static CANVAS_PREVIEW_EPOCH: AtomicU64 = AtomicU64::new(0);
static COMPRESSION_PREVIEW_EPOCH: AtomicU64 = AtomicU64::new(0);
static CONVERSION_PREVIEW_EPOCH: AtomicU64 = AtomicU64::new(0);
static CONVERSATION_PREVIEW_EPOCH: AtomicU64 = AtomicU64::new(0);
static REFERENCE_PREVIEW_EPOCH: AtomicU64 = AtomicU64::new(0);
static VIEWER_PREVIEW_EPOCH: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
struct GalleryViewportState {
    top: f32,
    height: f32,
    width: f32,
    card_width: f32,
    loading_count: i32,
}

impl Default for GalleryViewportState {
    fn default() -> Self {
        Self {
            top: 0.0,
            height: 720.0,
            width: 900.0,
            card_width: 200.0,
            loading_count: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GalleryLayoutKey {
    width_px: i32,
    card_width_px: i32,
    layout_mode: String,
    category: String,
    language: String,
    loading_count: i32,
}

#[derive(Clone, Debug)]
struct GalleryLayoutRow {
    source_index: usize,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    gap: f32,
    masonry: bool,
}

#[derive(Clone, Debug)]
struct GalleryHeaderRow {
    title: String,
    y: f32,
}

#[derive(Clone, Debug)]
struct GalleryLoaderRow {
    sequence_index: i32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    gap: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GalleryWindowKey {
    rows: Vec<usize>,
    headers: Vec<usize>,
    loaders: Vec<usize>,
}

struct GalleryLayoutCache {
    key: GalleryLayoutKey,
    rows: Vec<GalleryLayoutRow>,
    rows_by_y: Vec<usize>,
    headers: Vec<GalleryHeaderRow>,
    loaders: Vec<GalleryLoaderRow>,
    content_height: f32,
    max_row_height: f32,
    published_window: Option<GalleryWindowKey>,
}

#[derive(Default)]
struct GalleryVirtualSlot {
    viewport: GalleryViewportState,
    layout_mode: String,
    cache: Option<GalleryLayoutCache>,
}

#[derive(Default)]
struct GalleryVirtualState {
    assets: GalleryVirtualSlot,
    generations: GalleryVirtualSlot,
    inspiration: GalleryVirtualSlot,
}

impl GalleryVirtualState {
    fn slot_mut(&mut self, collection: PreviewCollection) -> &mut GalleryVirtualSlot {
        match collection {
            PreviewCollection::Assets => &mut self.assets,
            PreviewCollection::Generations => &mut self.generations,
            PreviewCollection::Inspiration => &mut self.inspiration,
        }
    }
}

thread_local! {
    static GALLERY_VIRTUAL_STATE: RefCell<GalleryVirtualState> = RefCell::new(GalleryVirtualState::default());
}

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
    state.set_viewer_source_path(item.source_path.clone().into());
    state.set_viewer_image(viewer_placeholder_image(&state, id, source));
    state.set_viewer_title(item.title.clone().into());
    let viewer_prompt = readable_deep_prompt(&item.prompt, &store.deep_prompt_bindings);
    state.set_viewer_prompt(viewer_prompt.clone().into());
    state.set_viewer_prompt_lines(estimated_prompt_lines(&viewer_prompt));
    state.set_viewer_time(item.time.clone().into());
    state.set_viewer_ratio(item.ratio.clone().into());
    state.set_viewer_quality(item.quality.clone().into());
    state.set_viewer_model(item.model.clone().into());
    state.set_viewer_repeat_enabled(
        item.origin != "watermark_removal"
            && item.origin != "image_enhancement"
            && item.origin != "image_colorization"
            && item.origin != "image_crop",
    );
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
    schedule_viewer_preview(app, item, source);
}

fn viewer_placeholder_image(state: &AppState, id: &str, source: &str) -> Image {
    let items = match source {
        "asset" => state.get_assets(),
        "inspiration" => state.get_inspiration(),
        _ => state.get_generations(),
    };
    items
        .iter()
        .find(|item| item.id.as_str() == id)
        .map(|item| item.image)
        .unwrap_or_default()
}

fn schedule_viewer_preview(app: &AppWindow, item: &AssetData, source: &str) {
    if item.source_path.trim().is_empty() || item.source_path == "failed" {
        return;
    }
    let preview_epoch = VIEWER_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    let item_id = item.id.clone();
    let source_kind = source.to_string();
    let source_path = item.source_path.clone();
    let weak = app.as_weak();
    let _ = std::thread::Builder::new()
        .name("viewer-preview-loader".to_string())
        .spawn(move || {
            let Ok(Some(prepared)) = prepare_preview_image_if(
                Path::new(&source_path),
                PreviewPurpose::Viewer,
                || VIEWER_PREVIEW_EPOCH.load(Ordering::Acquire) == preview_epoch,
            )
            else {
                return;
            };
            let _ = weak.upgrade_in_event_loop(move |app| {
                let state = app.global::<AppState>();
                if VIEWER_PREVIEW_EPOCH.load(Ordering::Acquire) != preview_epoch
                    || !state.get_viewer_open()
                    || state.get_viewer_id().as_str() != item_id
                    || state.get_viewer_source().as_str() != source_kind
                    || state.get_viewer_source_path().as_str() != source_path
                {
                    return;
                }
                state.set_viewer_image(materialize_prepared_preview(prepared));
            });
        });
}

fn readable_deep_prompt(prompt: &str, bindings: &BTreeMap<String, DeepPromptBinding>) -> String {
    let prompt = prompt.trim();
    for binding in bindings.values() {
        let english = binding.english.trim();
        let chinese = binding.chinese.trim();
        if english.is_empty() || chinese.is_empty() {
            continue;
        }
        if let Some(prefix) = prompt.strip_suffix(english) {
            return format!("{prefix}{chinese}");
        }
    }
    prompt.to_string()
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

#[cfg(test)]
mod deep_prompt_display_tests {
    use super::{readable_deep_prompt, DeepPromptBinding};
    use std::collections::BTreeMap;

    #[test]
    fn viewer_replaces_a_saved_english_deep_prompt_with_its_chinese_version() {
        let bindings = BTreeMap::from([(
            "character".to_string(),
            DeepPromptBinding {
                chinese: "月下的古风少女".to_string(),
                english: "an ancient-style girl under moonlight".to_string(),
            },
        )]);

        assert_eq!(
            readable_deep_prompt(
                "自定义风格\n\nan ancient-style girl under moonlight",
                &bindings,
            ),
            "自定义风格\n\n月下的古风少女",
        );
    }
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
    match source {
        "asset" => {
            let category = state.get_asset_category_filter().to_string();
            store
                .assets
                .iter()
                .filter(|item| include_gallery_item(item, "all", &category))
                .map(|item| item.id.clone())
                .collect()
        }
        "inspiration" => store
            .inspiration
            .iter()
            .filter(|item| {
                include_gallery_item(
                    item,
                    "all",
                    state.get_inspiration_category_filter().as_str(),
                )
            })
            .map(|item| item.id.clone())
            .collect(),
        _ => {
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            store
                .generations
                .iter()
                .filter(|item| item.category == category)
                .map(|item| item.id.clone())
                .collect()
        }
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
    release_inactive_page_images(&state, page);
    state.set_page(page.into());
    restore_page_previews(app, page);
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
        push_conversations(app, store);
        push_references(app, store);
    }
    if page == "inspiration" && app.global::<AppState>().get_logged_in() {
        push_inspiration(app, store);
    }
    if page == "canvas" {
        push_canvas_notes(app, store);
        push_canvas_references(app, store);
    }
}

pub(super) fn push_startup_state(app: &AppWindow, store: &Store) {
    push_model_groups(app, store);
    push_prompt_history(app, store);
    push_custom_prompts(app, store);
    push_notifications(app, store);
}

fn release_inactive_page_images(state: &AppState, target_page: &str) {
    if target_page != "assets" {
        cancel_gallery_previews(PreviewCollection::Assets);
        invalidate_virtual_gallery(PreviewCollection::Assets);
        state.set_asset_visible_limit(GALLERY_PAGE_SIZE);
        state.set_assets(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_asset_groups(ModelRc::new(VecModel::<AssetGroup>::default()));
        state.set_asset_layout_items(ModelRc::new(VecModel::default()));
        state.set_asset_layout_headers(ModelRc::new(VecModel::default()));
        state.set_asset_layout_height(1.0);
        state.set_asset_col_0(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_asset_col_1(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_asset_col_2(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_asset_col_3(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_asset_col_4(ModelRc::new(VecModel::<AssetItem>::default()));
    }
    if target_page != "generation" {
        cancel_gallery_previews(PreviewCollection::Generations);
        invalidate_virtual_gallery(PreviewCollection::Generations);
        CONVERSATION_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel);
        REFERENCE_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel);
        state.set_generation_visible_limit(GALLERY_PAGE_SIZE);
        state.set_generations(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_generation_groups(ModelRc::new(VecModel::<AssetGroup>::default()));
        state.set_generation_layout_items(ModelRc::new(VecModel::default()));
        state.set_generation_layout_headers(ModelRc::new(VecModel::default()));
        state.set_generation_layout_loaders(ModelRc::new(VecModel::default()));
        state.set_generation_layout_height(1.0);
        state.set_conversations(ModelRc::new(VecModel::<ConversationItem>::default()));
        state.set_references(ModelRc::new(VecModel::<ReferenceItem>::default()));
    }
    if target_page != "inspiration" {
        cancel_gallery_previews(PreviewCollection::Inspiration);
        invalidate_virtual_gallery(PreviewCollection::Inspiration);
        state.set_inspiration_visible_limit(GALLERY_PAGE_SIZE);
        state.set_inspiration(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_inspiration_layout_items(ModelRc::new(VecModel::default()));
        state.set_inspiration_layout_headers(ModelRc::new(VecModel::default()));
        state.set_inspiration_layout_height(1.0);
        state.set_inspiration_col_0(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_inspiration_col_1(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_inspiration_col_2(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_inspiration_col_3(ModelRc::new(VecModel::<AssetItem>::default()));
        state.set_inspiration_col_4(ModelRc::new(VecModel::<AssetItem>::default()));
    }
    if target_page != "canvas" {
        CANVAS_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel);
        state.set_canvas_notes(ModelRc::new(VecModel::<CanvasNote>::default()));
        state.set_canvas_links(ModelRc::new(VecModel::<CanvasLink>::default()));
    }
    if target_page != "toolbox-compress" {
        COMPRESSION_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel);
        state.set_compression_images(ModelRc::new(VecModel::from(
            strip_toolbox_previews(state.get_compression_images().iter().collect()),
        )));
    }
    if target_page != "toolbox-convert" {
        CONVERSION_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel);
        state.set_conversion_images(ModelRc::new(VecModel::from(
            strip_toolbox_previews(state.get_conversion_images().iter().collect()),
        )));
    }
    if target_page != "toolbox-enhance" {
        state.set_enhance_source_image(Image::default());
        state.set_enhance_result_image(Image::default());
    }
    if !matches!(target_page, "toolbox-watermark" | "toolbox-remove-black") {
        state.set_watermark_source_image(Image::default());
        state.set_watermark_result_image(Image::default());
    }
    if target_page != "toolbox-colorize" {
        state.set_colorize_source_image(Image::default());
        state.set_colorize_result_image(Image::default());
    }
    if target_page != "toolbox-crop" {
        state.set_crop_source_image(Image::default());
    }
}

fn strip_toolbox_previews(mut items: Vec<CompressionImageItem>) -> Vec<CompressionImageItem> {
    for item in &mut items {
        item.image = Image::default();
    }
    items
}

fn restore_page_previews(app: &AppWindow, page: &str) {
    let state = app.global::<AppState>();
    match page {
        "toolbox-compress" => {
            let items = state.get_compression_images().iter().collect::<Vec<_>>();
            let tasks = items
                .iter()
                .filter(|item| item.image.size().width == 0 && !item.source_path.is_empty())
                .map(|item| (item.id.to_string(), item.source_path.to_string()))
                .collect();
            state.set_compression_images(ModelRc::new(VecModel::from(items)));
            schedule_toolbox_batch_previews(app, false, tasks);
        }
        "toolbox-convert" => {
            let items = state.get_conversion_images().iter().collect::<Vec<_>>();
            let tasks = items
                .iter()
                .filter(|item| item.image.size().width == 0 && !item.source_path.is_empty())
                .map(|item| (item.id.to_string(), item.source_path.to_string()))
                .collect();
            state.set_conversion_images(ModelRc::new(VecModel::from(items)));
            schedule_toolbox_batch_previews(app, true, tasks);
        }
        "toolbox-enhance" => {
            restore_single_preview(
                &state.get_enhance_source_path().to_string(),
                |image| state.set_enhance_source_image(image),
            );
            restore_single_preview(
                &state.get_enhance_result_path().to_string(),
                |image| state.set_enhance_result_image(image),
            );
        }
        "toolbox-watermark" | "toolbox-remove-black" => {
            restore_single_preview(
                &state.get_watermark_source_path().to_string(),
                |image| state.set_watermark_source_image(image),
            );
            restore_single_preview(
                &state.get_watermark_result_path().to_string(),
                |image| state.set_watermark_result_image(image),
            );
        }
        "toolbox-colorize" => {
            restore_single_preview(
                &state.get_colorize_source_path().to_string(),
                |image| state.set_colorize_source_image(image),
            );
            restore_single_preview(
                &state.get_colorize_result_path().to_string(),
                |image| state.set_colorize_result_image(image),
            );
        }
        "toolbox-crop" => restore_single_preview(
            &state.get_crop_source_path().to_string(),
            |image| state.set_crop_source_image(image),
        ),
        _ => {}
    }
}

fn schedule_toolbox_batch_previews(
    app: &AppWindow,
    conversion: bool,
    tasks: Vec<(String, String)>,
) {
    if tasks.is_empty() {
        return;
    }
    let epoch_source = if conversion {
        &CONVERSION_PREVIEW_EPOCH
    } else {
        &COMPRESSION_PREVIEW_EPOCH
    };
    let preview_epoch = epoch_source.fetch_add(1, Ordering::AcqRel) + 1;
    let expected_page = if conversion {
        "toolbox-convert"
    } else {
        "toolbox-compress"
    };
    let weak = app.as_weak();
    let _ = std::thread::Builder::new()
        .name("toolbox-preview-loader".to_string())
        .spawn(move || {
            for (item_id, source_path) in tasks {
                let epoch_source = if conversion {
                    &CONVERSION_PREVIEW_EPOCH
                } else {
                    &COMPRESSION_PREVIEW_EPOCH
                };
                if epoch_source.load(Ordering::Acquire) != preview_epoch {
                    return;
                }
                let Ok(Some(prepared)) = prepare_preview_image_if(
                    Path::new(&source_path),
                    PreviewPurpose::Toolbox,
                    || epoch_source.load(Ordering::Acquire) == preview_epoch,
                )
                else {
                    continue;
                };
                let weak = weak.clone();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let epoch_source = if conversion {
                        &CONVERSION_PREVIEW_EPOCH
                    } else {
                        &COMPRESSION_PREVIEW_EPOCH
                    };
                    if epoch_source.load(Ordering::Acquire) != preview_epoch
                        || app.global::<AppState>().get_page().as_str() != expected_page
                    {
                        return;
                    }
                    let image = materialize_prepared_preview(prepared);
                    let state = app.global::<AppState>();
                    let items = if conversion {
                        state.get_conversion_images()
                    } else {
                        state.get_compression_images()
                    };
                    for row in 0..items.row_count() {
                        let Some(mut item) = items.row_data(row) else {
                            continue;
                        };
                        if item.id.as_str() == item_id
                            && item.source_path.as_str() == source_path
                        {
                            item.image = image.clone();
                            items.set_row_data(row, item);
                            break;
                        }
                    }
                });
            }
        });
}

fn restore_single_preview(path: &str, setter: impl FnOnce(Image)) {
    if path.trim().is_empty() {
        return;
    }
    setter(
        load_preview_image(Path::new(path), PreviewPurpose::Canvas).unwrap_or_default(),
    );
}

pub(super) fn push_all(app: &AppWindow, store: &Store) {
    push_model_groups(app, store);
    push_prompt_history(app, store);
    push_custom_prompts(app, store);
    push_notifications(app, store);
    match app.global::<AppState>().get_page().as_str() {
        "generation" => {
            push_conversations(app, store);
            push_generations(app, store);
            push_references(app, store);
        }
        "assets" => push_assets(app, store),
        "inspiration" => push_inspiration(app, store),
        "canvas" => {
            push_canvas_notes(app, store);
            push_canvas_references(app, store);
        }
        _ => {}
    }
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
        store
            .generations
            .iter()
            .map(|item| item.prompt.as_str())
            .filter(|prompt| !store.dismissed_prompt_history.contains(prompt.trim())),
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
    let category = current_workspace_category(app);
    let items = store
        .custom_prompts
        .iter()
        .map(|prompt| {
            let profile = store.custom_prompt_profiles.get(prompt);
            let preview = single_line_prompt_preview(prompt);
            let name = profile
                .map(|profile| profile.name.trim())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| preview.chars().take(48).collect());
            CustomPromptItem {
                name: name.into(),
                preview: preview.into(),
                content: prompt.clone().into(),
                selected: custom_prompt_selected_for_category(store, &category, prompt),
                prefix: "".into(),
                start_offset: -1,
                end_offset: -1,
                category: normalized_custom_prompt_category(
                    profile
                        .map(|profile| profile.category.as_str())
                        .unwrap_or("default"),
                )
                .into(),
                format: normalized_custom_prompt_format(
                    profile
                        .map(|profile| profile.format.as_str())
                        .unwrap_or("json"),
                )
                .into(),
                time: store
                    .custom_prompt_times
                    .get(prompt)
                    .cloned()
                    .unwrap_or_default()
                    .into(),
            }
        })
        .collect::<Vec<_>>();
    let replacements = selected_custom_prompt_replacements_for_category(store, &category);
    let mut prompt = state.get_prompt().to_string();
    let missing = replacements
        .iter()
        .filter(|(name, _)| !prompt.contains(&inline_custom_prompt_display_text(name)))
        .map(|(name, _)| inline_custom_prompt_display_text(name))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let mut migrated = missing.join(" ");
        if !prompt.is_empty() && prompt != "//" {
            migrated.push(' ');
            migrated.push_str(&prompt);
        }
        prompt = migrated;
        state.set_prompt(prompt.clone().into());
    }
    let selected_items = inline_custom_prompt_occurrences(&prompt, &replacements)
        .into_iter()
        .filter_map(|occurrence| {
            let mut item = items
                .iter()
                .find(|item| item.content.as_str() == occurrence.content)
                .cloned()?;
            item.name = occurrence.name.into();
            item.prefix = occurrence.prefix.into();
            item.start_offset = occurrence.start_offset;
            item.end_offset = occurrence.end_offset;
            Some(item)
        })
        .collect::<Vec<_>>();
    state.set_selected_custom_prompt_items(ModelRc::new(VecModel::from(selected_items)));
    state.set_custom_prompt_items(ModelRc::new(VecModel::from(items)));
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
    let preview_epoch = CANVAS_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    let linked_inputs = canvas_linked_inputs(store);
    let existing_previews = state
        .get_canvas_notes()
        .iter()
        .filter(|note| {
            note.preview_image.size().width > 0 && note.preview_image.size().height > 0
        })
        .map(|note| {
            (
                (note.id.to_string(), note.image_path.to_string()),
                note.preview_image,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut canvas_notes = store.canvas_notes.iter().collect::<Vec<_>>();
    canvas_notes.sort_by(|left, right| {
        let left_group = left.kind == "group";
        let right_group = right.kind == "group";
        left_group
            .cmp(&right_group)
            .reverse()
            .then_with(|| {
                if left_group && right_group {
                    group_depth(&store.canvas_notes, &left.id)
                        .cmp(&group_depth(&store.canvas_notes, &right.id))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| left.z_index.cmp(&right.z_index))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut preview_tasks = Vec::new();
    let canvas_views = canvas_notes
        .into_iter()
        .map(|note| {
            let preview_key = (note.id.clone(), note.image_path.clone());
            let preview_image = existing_previews
                .get(&preview_key)
                .cloned()
                .unwrap_or_default();
            if !note.image_path.is_empty() && !existing_previews.contains_key(&preview_key) {
                preview_tasks.push((note.id.clone(), note.image_path.clone()));
            }
            CanvasNote {
                id: note.id.clone().into(),
                kind: note.kind.clone().into(),
                content: note.content.clone().into(),
                linked_input: linked_inputs
                    .get(&note.id)
                    .cloned()
                    .unwrap_or_default()
                    .into(),
                x: note.x,
                y: note.y,
                width: note.width,
                height: note.height,
                parent_group_id: note.parent_group_id.clone().into(),
                z_index: note.z_index,
                image_path: note.image_path.clone().into(),
                preview_image,
                font_size: note.font_size,
                selected: note.selected,
            }
        })
        .collect::<Vec<_>>();
    state.set_canvas_notes(ModelRc::new(VecModel::from(canvas_views)));
    schedule_canvas_previews(app, preview_epoch, preview_tasks);
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
                    flow_reversed: link.flow_reversed,
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

fn schedule_canvas_previews(
    app: &AppWindow,
    preview_epoch: u64,
    tasks: Vec<(String, String)>,
) {
    if tasks.is_empty() {
        return;
    }
    let weak = app.as_weak();
    let _ = std::thread::Builder::new()
        .name("canvas-preview-loader".to_string())
        .spawn(move || {
            for (note_id, source_path) in tasks {
                if CANVAS_PREVIEW_EPOCH.load(Ordering::Acquire) != preview_epoch {
                    return;
                }
                let Ok(Some(prepared)) = prepare_original_image_if(
                    Path::new(&source_path),
                    || CANVAS_PREVIEW_EPOCH.load(Ordering::Acquire) == preview_epoch,
                )
                else {
                    continue;
                };
                let weak = weak.clone();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    if CANVAS_PREVIEW_EPOCH.load(Ordering::Acquire) != preview_epoch
                        || app.global::<AppState>().get_page().as_str() != "canvas"
                    {
                        return;
                    }
                    let image = materialize_prepared_preview(prepared);
                    let notes = app.global::<AppState>().get_canvas_notes();
                    for row in 0..notes.row_count() {
                        let Some(mut note) = notes.row_data(row) else {
                            continue;
                        };
                        if note.id.as_str() == note_id
                            && note.image_path.as_str() == source_path
                        {
                            note.preview_image = image.clone();
                            notes.set_row_data(row, note);
                            break;
                        }
                    }
                });
            }
        });
}

fn canvas_linked_inputs(store: &Store) -> BTreeMap<String, String> {
    let notes_by_id = store
        .canvas_notes
        .iter()
        .map(|note| (note.id.clone(), note))
        .collect::<BTreeMap<_, _>>();
    let mut upstream_by_target = BTreeMap::<String, Vec<String>>::new();
    for link in &store.canvas_links {
        upstream_by_target
            .entry(link.target_id.clone())
            .or_default()
            .push(link.source_id.clone());
    }
    let mut memo = BTreeMap::<String, Option<String>>::new();
    let mut result = BTreeMap::new();
    for target in &store.canvas_notes {
        let mut visiting = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let linked = upstream_by_target
            .get(&target.id)
            .into_iter()
            .flatten()
            .filter_map(|source_id| {
                resolved_canvas_content(
                    &notes_by_id,
                    &upstream_by_target,
                    source_id,
                    &mut visiting,
                    &mut memo,
                )
            })
            .filter(|content| seen.insert(content.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        if !linked.is_empty() {
            result.insert(target.id.clone(), linked);
        }
    }
    result
}

fn resolved_canvas_content(
    notes_by_id: &BTreeMap<String, &CanvasNoteData>,
    upstream_by_target: &BTreeMap<String, Vec<String>>,
    node_id: &str,
    visiting: &mut BTreeSet<String>,
    memo: &mut BTreeMap<String, Option<String>>,
) -> Option<String> {
    if let Some(cached) = memo.get(node_id) {
        return cached.clone();
    }
    if !visiting.insert(node_id.to_string()) {
        return None;
    }
    let Some(note) = notes_by_id.get(node_id).copied() else {
        visiting.remove(node_id);
        return None;
    };
    let mut seen = BTreeSet::new();
    let mut parts = upstream_by_target
        .get(node_id)
        .into_iter()
        .flatten()
        .filter_map(|source_id| {
            resolved_canvas_content(
                notes_by_id,
                upstream_by_target,
                source_id,
                visiting,
                memo,
            )
        })
        .filter(|content| seen.insert(content.clone()))
        .collect::<Vec<_>>();
    let own = meaningful_canvas_content(note);
    if !own.is_empty() && seen.insert(own.to_string()) {
        parts.push(own.to_string());
    }
    visiting.remove(node_id);
    let resolved = parts.join("\n");
    let resolved = (!resolved.is_empty()).then_some(resolved);
    memo.insert(node_id.to_string(), resolved.clone());
    resolved
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
    let preview_epoch = CONVERSATION_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    let existing_previews = state
        .get_conversations()
        .iter()
        .filter(|item| item.image.size().width > 0 && item.image.size().height > 0)
        .map(|item| (item.id.to_string(), item.image))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut conversations = Vec::new();
    let mut preview_tasks = Vec::new();
    for item in store
        .generations
        .iter()
        .filter(|item| item.source_path != "failed" && !item.conversation_id.trim().is_empty())
    {
        if !seen.insert(item.conversation_id.clone()) {
            continue;
        }
        let image = existing_previews
            .get(&item.conversation_id)
            .cloned()
            .unwrap_or_default();
        if image.size().width == 0 {
            preview_tasks.push((item.conversation_id.clone(), item.source_path.clone()));
        }
        conversations.push(ConversationItem {
            id: item.conversation_id.clone().into(),
            title: short_text(&item.title, 10).into(),
            image,
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
    schedule_conversation_previews(app, preview_epoch, preview_tasks);
}

fn schedule_conversation_previews(
    app: &AppWindow,
    preview_epoch: u64,
    tasks: Vec<(String, String)>,
) {
    if tasks.is_empty() {
        return;
    }
    let weak = app.as_weak();
    let _ = std::thread::Builder::new()
        .name("conversation-preview-loader".to_string())
        .spawn(move || {
            for (conversation_id, source_path) in tasks {
                if CONVERSATION_PREVIEW_EPOCH.load(Ordering::Acquire) != preview_epoch {
                    return;
                }
                let Ok(Some(prepared)) = prepare_preview_image_if(
                    Path::new(&source_path),
                    PreviewPurpose::Reference,
                    || CONVERSATION_PREVIEW_EPOCH.load(Ordering::Acquire) == preview_epoch,
                )
                else {
                    continue;
                };
                let weak = weak.clone();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    if CONVERSATION_PREVIEW_EPOCH.load(Ordering::Acquire) != preview_epoch
                        || app.global::<AppState>().get_page().as_str() != "generation"
                    {
                        return;
                    }
                    let image = materialize_prepared_preview(prepared);
                    let conversations = app.global::<AppState>().get_conversations();
                    for row in 0..conversations.row_count() {
                        let Some(mut item) = conversations.row_data(row) else {
                            continue;
                        };
                        if item.id.as_str() == conversation_id {
                            item.image = image.clone();
                            conversations.set_row_data(row, item);
                            break;
                        }
                    }
                });
            }
        });
}

fn gallery_collection_items(store: &Store, collection: PreviewCollection) -> &[AssetData] {
    match collection {
        PreviewCollection::Assets => &store.assets,
        PreviewCollection::Generations => &store.generations,
        PreviewCollection::Inspiration => &store.inspiration,
    }
}

fn gallery_filter_category(state: &AppState, collection: PreviewCollection) -> String {
    match collection {
        PreviewCollection::Assets => state.get_asset_category_filter().to_string(),
        PreviewCollection::Generations => {
            resolve_category(&state.get_asset_type().to_string(), "")
        }
        PreviewCollection::Inspiration => state.get_inspiration_category_filter().to_string(),
    }
}

fn gallery_filtered_indices(
    store: &Store,
    collection: PreviewCollection,
    category: &str,
) -> Vec<usize> {
    gallery_collection_items(store, collection)
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let included = match collection {
                PreviewCollection::Generations => item.category == category,
                PreviewCollection::Assets | PreviewCollection::Inspiration => {
                    include_gallery_item(item, "all", category)
                }
            };
            included.then_some(index)
        })
        .collect()
}

fn invalidate_virtual_gallery(collection: PreviewCollection) {
    GALLERY_VIRTUAL_STATE.with(|state| {
        state.borrow_mut().slot_mut(collection).cache = None;
    });
}

fn refresh_virtual_gallery(app: &AppWindow, store: &Store, collection: PreviewCollection) {
    let (viewport, layout_mode) = GALLERY_VIRTUAL_STATE.with(|virtual_state| {
        let mut virtual_state = virtual_state.borrow_mut();
        let slot = virtual_state.slot_mut(collection);
        let layout_mode = if slot.layout_mode.is_empty() {
            let state = app.global::<AppState>();
            match collection {
                PreviewCollection::Assets => state.get_asset_gallery_layout().to_string(),
                PreviewCollection::Generations => {
                    state.get_generation_gallery_layout().to_string()
                }
                PreviewCollection::Inspiration => {
                    state.get_inspiration_gallery_layout().to_string()
                }
            }
        } else {
            slot.layout_mode.clone()
        };
        (slot.viewport, layout_mode)
    });
    update_virtual_gallery(
        app,
        store,
        collection,
        viewport,
        normalize_gallery_layout(&layout_mode),
    );
}

pub(super) fn update_gallery_viewport(
    app: &AppWindow,
    store: &Store,
    source: &str,
    top: f32,
    viewport_height: f32,
    viewport_width: f32,
    layout_mode: &str,
    card_width: f32,
    loading_count: i32,
) {
    let collection = match source {
        "asset" => PreviewCollection::Assets,
        "generation" => PreviewCollection::Generations,
        "inspiration" => PreviewCollection::Inspiration,
        _ => return,
    };
    let viewport = GalleryViewportState {
        top: top.max(0.0),
        height: viewport_height.max(1.0),
        width: viewport_width.max(1.0),
        card_width: card_width.max(1.0),
        loading_count: loading_count.clamp(0, 4),
    };
    let layout_mode = normalize_gallery_layout(layout_mode).to_string();
    GALLERY_VIRTUAL_STATE.with(|virtual_state| {
        let mut virtual_state = virtual_state.borrow_mut();
        let slot = virtual_state.slot_mut(collection);
        slot.viewport = viewport;
        slot.layout_mode = layout_mode.clone();
    });
    update_virtual_gallery(app, store, collection, viewport, &layout_mode);
}

fn update_virtual_gallery(
    app: &AppWindow,
    store: &Store,
    collection: PreviewCollection,
    viewport: GalleryViewportState,
    layout_mode: &str,
) {
    let state = app.global::<AppState>();
    let category = gallery_filter_category(&state, collection);
    let language = state.get_language().to_string();
    let key = GalleryLayoutKey {
        width_px: viewport.width.round().max(1.0) as i32,
        card_width_px: viewport.card_width.round().max(1.0) as i32,
        layout_mode: normalize_gallery_layout(layout_mode).to_string(),
        category: category.clone(),
        language: language.clone(),
        loading_count: if collection == PreviewCollection::Generations {
            viewport.loading_count
        } else {
            0
        },
    };

    let publication = GALLERY_VIRTUAL_STATE.with(|virtual_state| {
        let mut virtual_state = virtual_state.borrow_mut();
        let slot = virtual_state.slot_mut(collection);
        let rebuild = slot
            .cache
            .as_ref()
            .is_none_or(|cache| cache.key != key);
        if rebuild {
            let filtered = gallery_filtered_indices(store, collection, &category);
            slot.cache = Some(build_gallery_layout(
                store,
                collection,
                &filtered,
                key.clone(),
            ));
        }
        let cache = slot.cache.as_mut().expect("gallery layout cache");
        let window = select_gallery_window(cache, viewport.top, viewport.height);
        if cache.published_window.as_ref() == Some(&window) {
            return None;
        }
        cache.published_window = Some(window.clone());
        let selected_rows = window
            .rows
            .iter()
            .filter_map(|index| cache.rows.get(*index).cloned())
            .collect::<Vec<_>>();
        let selected_headers = window
            .headers
            .iter()
            .filter_map(|index| cache.headers.get(*index).cloned())
            .collect::<Vec<_>>();
        let selected_loaders = window
            .loaders
            .iter()
            .filter_map(|index| cache.loaders.get(*index).cloned())
            .collect::<Vec<_>>();
        Some((
            selected_rows,
            selected_headers,
            selected_loaders,
            cache.content_height,
        ))
    });
    let Some((layout_rows, header_rows, loader_rows, content_height)) = publication else {
        return;
    };

    let existing_previews = match collection {
        PreviewCollection::Assets => gallery_preview_images(&state.get_assets()),
        PreviewCollection::Generations => gallery_preview_images(&state.get_generations()),
        PreviewCollection::Inspiration => gallery_preview_images(&state.get_inspiration()),
    };
    let source_items = gallery_collection_items(store, collection);
    let mut visible_assets = Vec::with_capacity(layout_rows.len());
    let mut placements = Vec::with_capacity(layout_rows.len());
    let mut preview_items = Vec::with_capacity(layout_rows.len());
    for layout in layout_rows {
        let Some(asset) = source_items.get(layout.source_index) else {
            continue;
        };
        let item_index = visible_assets.len() as i32;
        visible_assets.push(to_asset_view_with_previews(asset, &existing_previews));
        preview_items.push(asset);
        placements.push(GalleryPlacement {
            item_index,
            x: layout.x,
            y: layout.y,
            width: layout.width,
            gap: layout.gap,
            masonry: layout.masonry,
        });
    }
    let headers = header_rows
        .into_iter()
        .map(|header| GalleryHeader {
            title: header.title.into(),
            y: header.y,
        })
        .collect::<Vec<_>>();
    let loaders = loader_rows
        .into_iter()
        .map(|loader| GalleryLoadingPlacement {
            sequence_index: loader.sequence_index,
            x: loader.x,
            y: loader.y,
            width: loader.width,
            gap: loader.gap,
        })
        .collect::<Vec<_>>();

    cancel_gallery_previews(collection);
    let item_model = ModelRc::new(VecModel::from(visible_assets));
    let placement_model = ModelRc::new(VecModel::from(placements));
    let header_model = ModelRc::new(VecModel::from(headers));
    match collection {
        PreviewCollection::Assets => {
            state.set_assets(item_model);
            state.set_asset_layout_items(placement_model);
            state.set_asset_layout_headers(header_model);
            state.set_asset_layout_height(content_height);
            state.set_asset_groups(ModelRc::new(VecModel::default()));
        }
        PreviewCollection::Generations => {
            state.set_generations(item_model);
            state.set_generation_layout_items(placement_model);
            state.set_generation_layout_headers(header_model);
            state.set_generation_layout_loaders(ModelRc::new(VecModel::from(loaders)));
            state.set_generation_layout_height(content_height);
            state.set_generation_groups(ModelRc::new(VecModel::default()));
        }
        PreviewCollection::Inspiration => {
            state.set_inspiration(item_model);
            state.set_inspiration_layout_items(placement_model);
            state.set_inspiration_layout_headers(header_model);
            state.set_inspiration_layout_height(content_height);
        }
    }
    schedule_gallery_previews(app, &preview_items, collection, &existing_previews);
}

fn select_gallery_window(
    cache: &GalleryLayoutCache,
    viewport_top: f32,
    viewport_height: f32,
) -> GalleryWindowKey {
    let visible_top = viewport_top.max(0.0);
    let visible_bottom = visible_top + viewport_height.max(1.0);
    let overscan = viewport_height.max(1.0) * GALLERY_OVERSCAN_SCREENS;
    let overscan_top = (visible_top - overscan).max(0.0);
    let overscan_bottom = visible_bottom + overscan;

    let visible_range = gallery_rows_in_y_range(cache, visible_top, visible_bottom);
    let mut visible = cache.rows_by_y[visible_range]
        .iter()
        .copied()
        .filter(|index| {
            let row = &cache.rows[*index];
            row.y + row.height >= visible_top && row.y <= visible_bottom
        })
        .collect::<Vec<_>>();
    let mut selected = visible.iter().copied().collect::<BTreeSet<_>>();
    if selected.len() < MAX_GALLERY_WINDOW_ITEMS {
        let overscan_range = gallery_rows_in_y_range(cache, overscan_top, overscan_bottom);
        for index in cache.rows_by_y[overscan_range].iter().copied() {
            let row = &cache.rows[index];
            if row.y + row.height < overscan_top || row.y > overscan_bottom {
                continue;
            }
            selected.insert(index);
            if selected.len() >= MAX_GALLERY_WINDOW_ITEMS {
                break;
            }
        }
    }
    visible = selected.into_iter().collect();
    visible.sort_by(|left, right| {
        cache.rows[*left]
            .y
            .total_cmp(&cache.rows[*right].y)
            .then_with(|| cache.rows[*left].x.total_cmp(&cache.rows[*right].x))
    });
    let headers = cache
        .headers
        .iter()
        .enumerate()
        .filter_map(|(index, header)| {
            (header.y + 32.0 >= overscan_top && header.y <= overscan_bottom).then_some(index)
        })
        .collect();
    let loaders = cache
        .loaders
        .iter()
        .enumerate()
        .filter_map(|(index, loader)| {
            (loader.y + loader.height >= overscan_top && loader.y <= overscan_bottom)
                .then_some(index)
        })
        .collect();
    GalleryWindowKey {
        rows: visible,
        headers,
        loaders,
    }
}

fn gallery_rows_in_y_range(
    cache: &GalleryLayoutCache,
    range_top: f32,
    range_bottom: f32,
) -> std::ops::Range<usize> {
    let earliest_y = (range_top - cache.max_row_height).max(0.0);
    let start = cache
        .rows_by_y
        .partition_point(|index| cache.rows[*index].y < earliest_y);
    let end = cache
        .rows_by_y
        .partition_point(|index| cache.rows[*index].y <= range_bottom);
    start..end
}

fn build_gallery_layout(
    store: &Store,
    collection: PreviewCollection,
    filtered: &[usize],
    key: GalleryLayoutKey,
) -> GalleryLayoutCache {
    let source_items = gallery_collection_items(store, collection);
    let width = key.width_px.max(1) as f32;
    let requested_card_width = key.card_width_px.max(1) as f32;
    let waterfall = key.layout_mode == "waterfall";
    let mut rows = Vec::with_capacity(filtered.len());
    let mut headers = Vec::new();
    let mut loaders = Vec::new();

    if collection == PreviewCollection::Inspiration {
        layout_gallery_section(
            source_items,
            filtered,
            &mut rows,
            &mut loaders,
            0,
            0.0,
            width,
            requested_card_width,
            waterfall,
            collection,
        );
    } else {
        let mut groups: Vec<(String, Vec<usize>, i32)> = Vec::new();
        for source_index in filtered {
            let Some(item) = source_items.get(*source_index) else {
                continue;
            };
            let title = time_group_label(&item.time, &key.language);
            if groups.last().map(|group| group.0.as_str()) != Some(title.as_str()) {
                groups.push((title.clone(), Vec::new(), 0));
            }
            if let Some(group) = groups.last_mut() {
                group.1.push(*source_index);
            }
        }
        if collection == PreviewCollection::Generations && key.loading_count > 0 {
            let today = if key.language == "en" { "Today" } else { "今天" };
            if groups.first().is_some_and(|group| group.0 == today) {
                groups[0].2 = key.loading_count;
            } else {
                groups.insert(0, (today.to_string(), Vec::new(), key.loading_count));
            }
        }

        let mut cursor_y = 0.0_f32;
        for (title, indices, loading_count) in groups {
            if indices.is_empty() && loading_count == 0 {
                continue;
            }
            headers.push(GalleryHeaderRow { title, y: cursor_y });
            let grid_y = cursor_y + 40.0;
            let section_height = layout_gallery_section(
                source_items,
                &indices,
                &mut rows,
                &mut loaders,
                loading_count,
                grid_y,
                width,
                requested_card_width,
                waterfall,
                collection,
            );
            cursor_y = section_height + 26.0;
        }
    }

    let mut rows_by_y = (0..rows.len()).collect::<Vec<_>>();
    rows_by_y.sort_by(|left, right| {
        rows[*left]
            .y
            .total_cmp(&rows[*right].y)
            .then_with(|| rows[*left].x.total_cmp(&rows[*right].x))
    });
    let content_height = rows
        .iter()
        .map(|row| row.y + row.height)
        .chain(loaders.iter().map(|row| row.y + row.height))
        .chain(headers.iter().map(|header| header.y + 32.0))
        .fold(1.0_f32, f32::max)
        + if collection == PreviewCollection::Inspiration {
            0.0
        } else {
            26.0
        };
    let max_row_height = rows
        .iter()
        .map(|row| row.height)
        .fold(1.0_f32, f32::max);
    GalleryLayoutCache {
        key,
        rows,
        rows_by_y,
        headers,
        loaders,
        content_height,
        max_row_height,
        published_window: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_gallery_section(
    source_items: &[AssetData],
    source_indices: &[usize],
    rows: &mut Vec<GalleryLayoutRow>,
    loaders: &mut Vec<GalleryLoaderRow>,
    loading_count: i32,
    start_y: f32,
    width: f32,
    requested_card_width: f32,
    waterfall: bool,
    collection: PreviewCollection,
) -> f32 {
    let loading_count = loading_count.clamp(0, 4) as usize;
    let item_count = source_indices.len() + loading_count;
    if item_count == 0 {
        return start_y;
    }
    let gap = if waterfall {
        if collection == PreviewCollection::Generations {
            10.0
        } else {
            18.0
        }
    } else {
        8.0
    };
    let fit_columns = (((width + gap) / (requested_card_width + gap)).floor() as usize)
        .clamp(1, 8);
    let column_count = if waterfall {
        fit_columns
    } else {
        fit_columns.min(item_count).max(1)
    };
    let minimum_width = if collection == PreviewCollection::Generations {
        91.0
    } else if collection == PreviewCollection::Inspiration {
        118.0
    } else {
        118.0
    };
    let available_width = (width - gap * column_count.saturating_sub(1) as f32).max(1.0);
    let item_width = if waterfall {
        available_width / column_count as f32
    } else {
        requested_card_width.min((available_width / column_count as f32).max(minimum_width))
    };

    if !waterfall {
        for sequence in 0..loading_count {
            let column = sequence % column_count;
            let row = sequence / column_count;
            loaders.push(GalleryLoaderRow {
                sequence_index: sequence as i32,
                x: column as f32 * (item_width + gap),
                y: start_y + row as f32 * (item_width + gap),
                width: item_width,
                height: item_width,
                gap: 0.0,
            });
        }
        for (offset, source_index) in source_indices.iter().enumerate() {
            let position = loading_count + offset;
            let column = position % column_count;
            let row = position / column_count;
            rows.push(GalleryLayoutRow {
                source_index: *source_index,
                x: column as f32 * (item_width + gap),
                y: start_y + row as f32 * (item_width + gap),
                width: item_width,
                height: item_width,
                gap: 0.0,
                masonry: false,
            });
        }
        let row_count = item_count.div_ceil(column_count);
        return start_y + row_count as f32 * (item_width + gap);
    }

    let card_gap = gap;
    let mut column_bottoms = vec![start_y; column_count];
    for sequence in 0..loading_count {
        let column = shortest_gallery_column(&column_bottoms);
        let y = column_bottoms[column];
        loaders.push(GalleryLoaderRow {
            sequence_index: sequence as i32,
            x: column as f32 * (item_width + card_gap),
            y,
            width: item_width,
            height: item_width + card_gap,
            gap: card_gap,
        });
        column_bottoms[column] = y + item_width + card_gap;
    }
    for source_index in source_indices {
        let Some(asset) = source_items.get(*source_index) else {
            continue;
        };
        let column = shortest_gallery_column(&column_bottoms);
        let y = column_bottoms[column];
        let image_height = masonry_image_height(asset, item_width);
        rows.push(GalleryLayoutRow {
            source_index: *source_index,
            x: column as f32 * (item_width + card_gap),
            y,
            width: item_width,
            height: image_height + card_gap,
            gap: card_gap,
            masonry: true,
        });
        column_bottoms[column] = y + image_height + card_gap;
    }
    column_bottoms.into_iter().fold(start_y, f32::max)
}

fn shortest_gallery_column(column_bottoms: &[f32]) -> usize {
    column_bottoms
        .iter()
        .enumerate()
        .min_by(|left, right| left.1.total_cmp(right.1))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn masonry_image_height(asset: &AssetData, item_width: f32) -> f32 {
    if asset.width <= 0 || asset.height <= 0 {
        return item_width;
    }
    (item_width * asset.height as f32 / asset.width as f32)
        .clamp(item_width * 0.5, item_width * 2.4)
}

#[cfg(test)]
mod virtual_gallery_tests {
    use super::*;

    fn gallery_asset(index: usize) -> AssetData {
        AssetData {
            id: format!("asset-{index}"),
            conversation_id: String::new(),
            title: format!("Asset {index}"),
            category: "other".to_string(),
            kind: "game".to_string(),
            time: "2026-08-12 10:00".to_string(),
            prompt: String::new(),
            ratio: "1:1".to_string(),
            quality: "1k".to_string(),
            model: String::new(),
            origin: String::new(),
            width: if index % 3 == 0 { 1600 } else { 1024 },
            height: if index % 3 == 1 { 1600 } else { 1024 },
            source_path: format!("/tmp/{index}.png"),
            reference_paths: Vec::new(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable: false,
            delivery_downloading: false,
        }
    }

    #[test]
    fn ten_thousand_gallery_items_publish_a_bounded_viewport_window() {
        let store = Store {
            assets: (0..10_000).map(gallery_asset).collect(),
            ..Store::default()
        };
        let filtered = (0..store.assets.len()).collect::<Vec<_>>();
        let cache = build_gallery_layout(
            &store,
            PreviewCollection::Assets,
            &filtered,
            GalleryLayoutKey {
                width_px: 1200,
                card_width_px: 200,
                layout_mode: "waterfall".to_string(),
                category: "all".to_string(),
                language: "zh".to_string(),
                loading_count: 0,
            },
        );
        assert_eq!(cache.rows.len(), 10_000);
        let top = select_gallery_window(&cache, 0.0, 800.0);
        let middle = select_gallery_window(&cache, cache.content_height / 2.0, 800.0);
        let middle_candidates = gallery_rows_in_y_range(
            &cache,
            cache.content_height / 2.0 - 800.0,
            cache.content_height / 2.0 + 1600.0,
        );
        assert!(!top.rows.is_empty());
        assert!(!middle.rows.is_empty());
        assert!(top.rows.len() <= MAX_GALLERY_WINDOW_ITEMS);
        assert!(middle.rows.len() <= MAX_GALLERY_WINDOW_ITEMS);
        assert!(middle_candidates.len() < 200);
        assert_ne!(top.rows, middle.rows);
    }

    #[test]
    fn generation_loader_cards_share_the_virtualized_layout() {
        let store = Store {
            generations: (0..20).map(gallery_asset).collect(),
            ..Store::default()
        };
        let filtered = (0..store.generations.len()).collect::<Vec<_>>();
        let cache = build_gallery_layout(
            &store,
            PreviewCollection::Generations,
            &filtered,
            GalleryLayoutKey {
                width_px: 680,
                card_width_px: 160,
                layout_mode: "waterfall".to_string(),
                category: "other".to_string(),
                language: "zh".to_string(),
                loading_count: 4,
            },
        );
        assert_eq!(cache.loaders.len(), 4);
        assert_eq!(cache.rows.len(), 20);
        assert!(cache.rows.iter().all(|row| row.y >= 40.0));
        assert!(cache.content_height > 160.0);
    }
}

pub(super) fn push_assets(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    state.set_asset_character_count(count_assets(store, "character"));
    state.set_asset_scene_count(count_assets(store, "scene"));
    state.set_asset_ui_count(count_assets(store, "ui"));
    state.set_asset_effect_count(count_assets(store, "effect"));
    state.set_asset_other_count(count_assets(store, "other"));
    state.set_asset_all_count(store.assets.len() as i32);
    state.set_asset_has_more(false);
    state.set_asset_visible_limit(store.assets.len().min(i32::MAX as usize) as i32);
    invalidate_virtual_gallery(PreviewCollection::Assets);
    refresh_virtual_gallery(app, store, PreviewCollection::Assets);
}

pub(super) fn push_generations(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    let count = store
        .generations
        .iter()
        .filter(|item| item.category == category)
        .count();
    state.set_generation_has_more(false);
    state.set_generation_visible_limit(count.min(i32::MAX as usize) as i32);
    invalidate_virtual_gallery(PreviewCollection::Generations);
    refresh_virtual_gallery(app, store, PreviewCollection::Generations);
}

pub(super) fn push_inspiration(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let category = state.get_inspiration_category_filter().to_string();
    let count = store
        .inspiration
        .iter()
        .filter(|item| include_gallery_item(item, "all", &category))
        .count();
    state.set_inspiration_has_more(false);
    state.set_inspiration_visible_limit(count.min(i32::MAX as usize) as i32);
    invalidate_virtual_gallery(PreviewCollection::Inspiration);
    refresh_virtual_gallery(app, store, PreviewCollection::Inspiration);
}

pub(super) fn reset_asset_gallery_page(app: &AppWindow) {
    cancel_gallery_previews(PreviewCollection::Assets);
    invalidate_virtual_gallery(PreviewCollection::Assets);
    app.global::<AppState>()
        .set_asset_visible_limit(GALLERY_PAGE_SIZE);
}

pub(super) fn reset_generation_gallery_page(app: &AppWindow) {
    cancel_gallery_previews(PreviewCollection::Generations);
    invalidate_virtual_gallery(PreviewCollection::Generations);
    app.global::<AppState>()
        .set_generation_visible_limit(GALLERY_PAGE_SIZE);
}

pub(super) fn reset_inspiration_gallery_page(app: &AppWindow) {
    cancel_gallery_previews(PreviewCollection::Inspiration);
    invalidate_virtual_gallery(PreviewCollection::Inspiration);
    app.global::<AppState>()
        .set_inspiration_visible_limit(GALLERY_PAGE_SIZE);
}

pub(super) fn load_more_asset_gallery(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    if !state.get_asset_has_more() {
        return;
    }
    let kind = "all".to_string();
    let category = state.get_asset_category_filter().to_string();
    let filtered = store
        .assets
        .iter()
        .filter(|item| include_gallery_item(item, &kind, &category))
        .collect::<Vec<_>>();
    let flat = state.get_assets();
    let groups = state.get_asset_groups();
    append_gallery_page(
        app,
        &filtered,
        &flat,
        Some(&groups),
        state.get_language().as_str(),
        PreviewCollection::Assets,
        |visible, has_more| {
            state.set_asset_visible_limit(visible);
            state.set_asset_has_more(has_more);
        },
        || push_assets(app, store),
    );
}

pub(super) fn load_more_generation_gallery(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    if !state.get_generation_has_more() {
        return;
    }
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    let filtered = store
        .generations
        .iter()
        .filter(|item| item.category == category)
        .collect::<Vec<_>>();
    let flat = state.get_generations();
    let groups = state.get_generation_groups();
    append_gallery_page(
        app,
        &filtered,
        &flat,
        Some(&groups),
        state.get_language().as_str(),
        PreviewCollection::Generations,
        |visible, has_more| {
            state.set_generation_visible_limit(visible);
            state.set_generation_has_more(has_more);
        },
        || push_generations(app, store),
    );
}

pub(super) fn load_more_inspiration_gallery(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    if !state.get_inspiration_has_more() {
        return;
    }
    let kind = "all".to_string();
    let category = state.get_inspiration_category_filter().to_string();
    let filtered = store
        .inspiration
        .iter()
        .filter(|item| include_gallery_item(item, &kind, &category))
        .collect::<Vec<_>>();
    let flat = state.get_inspiration();
    append_gallery_page(
        app,
        &filtered,
        &flat,
        None,
        state.get_language().as_str(),
        PreviewCollection::Inspiration,
        |visible, has_more| {
            state.set_inspiration_visible_limit(visible);
            state.set_inspiration_has_more(has_more);
        },
        || push_inspiration(app, store),
    );
}

fn append_gallery_page(
    app: &AppWindow,
    filtered: &[&AssetData],
    flat: &ModelRc<AssetItem>,
    groups: Option<&ModelRc<AssetGroup>>,
    language: &str,
    collection: PreviewCollection,
    update_state: impl FnOnce(i32, bool),
    fallback: impl FnOnce(),
) {
    let current = flat.row_count();
    let end = current
        .saturating_add(GALLERY_PAGE_SIZE as usize)
        .min(filtered.len());
    let visible = end.min(i32::MAX as usize) as i32;
    if current > filtered.len()
        || !try_append_gallery_models(
            flat,
            groups,
            &filtered[..current.min(filtered.len())],
            &filtered[current.min(filtered.len())..end],
            language,
        )
    {
        update_state(visible, end < filtered.len());
        fallback();
        return;
    }

    update_state(visible, end < filtered.len());
    schedule_gallery_previews(
        app,
        &filtered[current..end],
        collection,
        &BTreeMap::new(),
    );
}

fn try_append_gallery_models(
    flat: &ModelRc<AssetItem>,
    groups: Option<&ModelRc<AssetGroup>>,
    expected_existing: &[&AssetData],
    appended: &[&AssetData],
    language: &str,
) -> bool {
    if !gallery_models_match_prefix(flat, groups, expected_existing, language) {
        return false;
    }
    if appended.is_empty() {
        return true;
    }

    let Some(flat_model) = flat.as_any().downcast_ref::<VecModel<AssetItem>>() else {
        return false;
    };
    let appended_views = appended
        .iter()
        .map(|item| to_asset_view_metadata(item))
        .collect::<Vec<_>>();

    if let Some(groups) = groups {
        let Some(group_model) = groups.as_any().downcast_ref::<VecModel<AssetGroup>>() else {
            return false;
        };
        let mut appended_groups = group_asset_views_with_previews(
            appended,
            language,
            &BTreeMap::new(),
        );
        let merge_tail = groups
            .row_count()
            .checked_sub(1)
            .and_then(|row| groups.row_data(row))
            .filter(|tail| {
                appended_groups
                    .first()
                    .is_some_and(|first| first.title == tail.title)
            });
        let tail_items = if let Some(tail) = merge_tail.as_ref() {
            let Some(model) = tail.items.as_any().downcast_ref::<VecModel<AssetItem>>() else {
                return false;
            };
            Some(model)
        } else {
            None
        };

        if let Some(tail_items) = tail_items {
            let first = appended_groups.remove(0);
            tail_items.extend(first.items.iter());
        }
        group_model.extend(appended_groups);
    }

    flat_model.extend(appended_views);
    true
}

fn gallery_models_match_prefix(
    flat: &ModelRc<AssetItem>,
    groups: Option<&ModelRc<AssetGroup>>,
    expected: &[&AssetData],
    language: &str,
) -> bool {
    if flat.row_count() != expected.len()
        || expected.iter().enumerate().any(|(row, item)| {
            flat.row_data(row)
                .is_none_or(|view| !gallery_view_matches_data(&view, item))
        })
    {
        return false;
    }
    let Some(groups) = groups else {
        return true;
    };
    let expected_groups = grouped_gallery_data(expected, language);
    if groups.row_count() != expected_groups.len() {
        return false;
    }
    expected_groups
        .iter()
        .enumerate()
        .all(|(group_row, (title, items))| {
            groups.row_data(group_row).is_some_and(|group| {
                group.title.as_str() == title
                    && group.items.row_count() == items.len()
                    && items.iter().enumerate().all(|(item_row, item)| {
                        group
                            .items
                            .row_data(item_row)
                            .is_some_and(|view| gallery_view_matches_data(&view, item))
                    })
            })
        })
}

fn grouped_gallery_data<'a>(
    items: &[&'a AssetData],
    language: &str,
) -> Vec<(String, Vec<&'a AssetData>)> {
    let mut groups = Vec::<(String, Vec<&AssetData>)>::new();
    for item in items {
        let title = time_group_label(&item.time, language);
        if groups.last().map(|(last, _)| last.as_str()) != Some(title.as_str()) {
            groups.push((title, Vec::new()));
        }
        groups.last_mut().expect("group was just created").1.push(item);
    }
    groups
}

fn gallery_view_matches_data(view: &AssetItem, data: &AssetData) -> bool {
    view.id.as_str() == data.id
        && view.source_path.as_str() == data.source_path
        && view.time.as_str() == data.time
        && view.width == data.width
        && view.height == data.height
}

#[cfg(test)]
mod gallery_paging_tests {
    use super::*;

    fn asset(id: &str, time: &str) -> AssetData {
        AssetData {
            id: id.to_string(),
            conversation_id: String::new(),
            title: format!("Asset {id}"),
            category: "character".to_string(),
            kind: "game".to_string(),
            time: time.to_string(),
            prompt: String::new(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model: "test".to_string(),
            origin: "generation".to_string(),
            width: 1024,
            height: 1024,
            source_path: format!("/tmp/{id}.png"),
            reference_paths: Vec::new(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable: false,
            delivery_downloading: false,
        }
    }

    fn models(items: &[&AssetData]) -> (ModelRc<AssetItem>, ModelRc<AssetGroup>) {
        let flat = ModelRc::new(VecModel::from(
            items
                .iter()
                .map(|item| to_asset_view_metadata(item))
                .collect::<Vec<_>>(),
        ));
        let groups = ModelRc::new(VecModel::from(group_asset_views_with_previews(
            items,
            "zh",
            &BTreeMap::new(),
        )));
        (flat, groups)
    }

    #[test]
    fn incremental_gallery_append_preserves_images_and_extends_date_groups() {
        let first = asset("first", "2020-01-01 10:00:00");
        let second = asset("second", "2020-01-01 11:00:00");
        let third = asset("third", "2020-01-01 12:00:00");
        let fourth = asset("fourth", "2020-01-02 09:00:00");
        let existing = [&first, &second];
        let appended = [&third, &fourth];
        let (flat, groups) = models(&existing);
        let mut first_view = flat.row_data(0).expect("first row");
        first_view.image = Image::from_rgba8(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &[1, 2, 3, 255],
                1,
                1,
            ),
        );
        flat.set_row_data(0, first_view);

        assert!(try_append_gallery_models(
            &flat,
            Some(&groups),
            &existing,
            &appended,
            "zh",
        ));

        assert_eq!(flat.row_count(), 4);
        assert_eq!(flat.row_data(0).expect("first row").image.size().width, 1);
        assert_eq!(flat.row_data(2).expect("third row").id.as_str(), "third");
        assert_eq!(groups.row_count(), 2);
        assert_eq!(
            groups.row_data(0).expect("first group").items.row_count(),
            3
        );
        assert_eq!(
            groups.row_data(1).expect("second group").items.row_count(),
            1
        );
    }

    #[test]
    fn incremental_gallery_append_rejects_stale_prefix_without_partial_changes() {
        let first = asset("first", "2020-01-01 10:00:00");
        let second = asset("second", "2020-01-01 11:00:00");
        let appended = asset("third", "2020-01-01 12:00:00");
        let existing = [&first, &second];
        let (flat, groups) = models(&existing);
        let mut changed_second = second.clone();
        changed_second.source_path = "/tmp/replaced.png".to_string();
        let changed = [&first, &changed_second];

        assert!(!try_append_gallery_models(
            &flat,
            Some(&groups),
            &changed,
            &[&appended],
            "zh",
        ));
        assert_eq!(flat.row_count(), 2);
        assert_eq!(groups.row_count(), 1);
        assert_eq!(groups.row_data(0).expect("group").items.row_count(), 2);
    }

    #[test]
    fn incremental_inspiration_append_updates_only_the_flat_model() {
        let first = asset("first", "官方示例");
        let second = asset("second", "官方示例");
        let existing = [&first];
        let flat = ModelRc::new(VecModel::from(vec![to_asset_view_metadata(&first)]));

        assert!(try_append_gallery_models(
            &flat,
            None,
            &existing,
            &[&second],
            "zh",
        ));
        assert_eq!(flat.row_count(), 2);
        assert_eq!(flat.row_data(1).expect("second row").id.as_str(), "second");
    }

    #[test]
    fn preview_row_hints_update_flat_and_group_models_without_scanning() {
        let first = asset("first", "2020-01-01 10:00:00");
        let second = asset("second", "2020-01-01 11:00:00");
        let items = [&first, &second];
        let (flat, groups) = models(&items);
        let image = Image::from_rgba8(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &[9, 8, 7, 255],
                1,
                1,
            ),
        );
        let target = PreviewTarget {
            collection: PreviewCollection::Generations,
            asset_id: second.id.clone(),
            source_path: second.source_path.clone(),
            flat_row: Some(1),
            group_row: Some(0),
            group_item_row: Some(1),
        };

        assert!(update_asset_preview_row_at(
            &flat,
            target.flat_row,
            &target.asset_id,
            &target.source_path,
            &image,
        ));
        assert!(update_asset_preview_group_at(&groups, &target, &image));
        assert_eq!(flat.row_data(1).expect("flat row").image.size().width, 1);
        assert_eq!(
            groups
                .row_data(0)
                .expect("group")
                .items
                .row_data(1)
                .expect("group row")
                .image
                .size()
                .width,
            1
        );
    }
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

fn group_asset_views_with_previews(
    items: &[&AssetData],
    language: &str,
    previews: &BTreeMap<(String, String), Image>,
) -> Vec<AssetGroup> {
    let mut groups: Vec<(String, Vec<AssetItem>)> = Vec::new();
    for asset in items {
        let title = time_group_label(&asset.time, language);
        if groups.last().map(|(last_title, _)| last_title.as_str()) != Some(title.as_str()) {
            groups.push((title.clone(), Vec::new()));
        }
        if let Some((_, group_items)) = groups.last_mut() {
            group_items.push(to_asset_view_with_previews(asset, previews));
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

fn gallery_preview_images(items: &ModelRc<AssetItem>) -> BTreeMap<(String, String), Image> {
    items
        .iter()
        .filter(|item| item.image.size().width > 0 && item.image.size().height > 0)
        .map(|item| {
            (
                (item.id.to_string(), item.source_path.to_string()),
                item.image,
            )
        })
        .collect()
}

fn to_asset_view_with_previews(
    asset: &AssetData,
    previews: &BTreeMap<(String, String), Image>,
) -> AssetItem {
    let mut item = to_asset_view_metadata(asset);
    if let Some(image) = previews.get(&(asset.id.clone(), asset.source_path.clone())) {
        item.image = image.clone();
    }
    item
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
    push_reference_items(
        app,
        references_for_category(&store.references, &category),
        max_references,
    );
}

pub(super) fn push_canvas_references(app: &AppWindow, store: &Store) {
    push_reference_items(app, &store.canvas_references, MAX_REFERENCE_IMAGES);
}

fn push_reference_items(
    app: &AppWindow,
    source: &[ReferenceData],
    max_references: usize,
) {
    let state = app.global::<AppState>();
    let preview_epoch = REFERENCE_PREVIEW_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    let existing_previews = state
        .get_references()
        .iter()
        .filter(|item| item.image.size().width > 0 && item.image.size().height > 0)
        .map(|item| {
            (
                (item.id.to_string(), item.source_path.to_string()),
                item.image,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut preview_tasks = Vec::new();
    let references = source
        .iter()
        .take(max_references)
        .map(|item| {
            let key = (item.id.clone(), item.source_path.clone());
            let image = existing_previews.get(&key).cloned().unwrap_or_default();
            if image.size().width == 0 && !item.source_path.trim().is_empty() {
                preview_tasks.push((item.id.clone(), item.source_path.clone()));
            }
            ReferenceItem {
                id: item.id.clone().into(),
                image,
                source_path: item.source_path.clone().into(),
            }
        })
        .collect::<Vec<_>>();
    state.set_references(ModelRc::new(VecModel::from(references)));
    schedule_reference_previews(app, preview_epoch, preview_tasks);
}

fn schedule_reference_previews(
    app: &AppWindow,
    preview_epoch: u64,
    tasks: Vec<(String, String)>,
) {
    if tasks.is_empty() {
        return;
    }
    let weak = app.as_weak();
    let _ = std::thread::Builder::new()
        .name("reference-preview-loader".to_string())
        .spawn(move || {
            for (reference_id, source_path) in tasks {
                let Ok(Some(prepared)) = prepare_preview_image_if(
                    Path::new(&source_path),
                    PreviewPurpose::Reference,
                    || REFERENCE_PREVIEW_EPOCH.load(Ordering::Acquire) == preview_epoch,
                ) else {
                    continue;
                };
                let weak = weak.clone();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    if REFERENCE_PREVIEW_EPOCH.load(Ordering::Acquire) != preview_epoch
                        || !reference_preview_page_is_visible(
                            app.global::<AppState>().get_page().as_str(),
                        )
                    {
                        return;
                    }
                    let image = materialize_prepared_preview(prepared);
                    let references = app.global::<AppState>().get_references();
                    for row in 0..references.row_count() {
                        let Some(mut item) = references.row_data(row) else {
                            continue;
                        };
                        if item.id.as_str() == reference_id
                            && item.source_path.as_str() == source_path
                        {
                            item.image = image.clone();
                            references.set_row_data(row, item);
                            break;
                        }
                    }
                });
            }
        });
}

fn reference_preview_page_is_visible(page: &str) -> bool {
    matches!(page, "generation" | "canvas")
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

#[cfg(test)]
mod reference_preview_page_tests {
    use super::reference_preview_page_is_visible;

    #[test]
    fn canvas_reference_previews_remain_visible_on_canvas_pages() {
        assert!(reference_preview_page_is_visible("generation"));
        assert!(reference_preview_page_is_visible("canvas"));
        assert!(!reference_preview_page_is_visible("settings"));
    }
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

pub(super) fn to_asset_view_metadata(asset: &AssetData) -> AssetItem {
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
        image: Image::default(),
        source_path: asset.source_path.clone().into(),
        drag_uri: file_uri_for_path(&asset.source_path).into(),
        cutout_done: asset.cutout_done,
        remove_black_done: asset.remove_black_done,
        upscale_done: asset.upscale_done,
        is_new: asset.is_new,
        delivery_recoverable: asset.delivery_recoverable,
        delivery_downloading: asset.delivery_downloading,
    }
}

fn schedule_gallery_previews(
    app: &AppWindow,
    items: &[&AssetData],
    collection: PreviewCollection,
    existing_previews: &BTreeMap<(String, String), Image>,
) {
    let positions = gallery_preview_positions(app, collection);
    for item in items {
        if item.source_path == "failed" || item.source_path.trim().is_empty() {
            continue;
        }
        if existing_previews.contains_key(&(item.id.clone(), item.source_path.clone())) {
            continue;
        }
        let position = positions
            .get(&(item.id.clone(), item.source_path.clone()))
            .copied()
            .unwrap_or((None, None, None));
        request_gallery_preview(
            app,
            Path::new(&item.source_path),
            PreviewPurpose::Gallery,
            PreviewTarget {
                collection,
                asset_id: item.id.clone(),
                source_path: item.source_path.clone(),
                flat_row: position.0,
                group_row: position.1,
                group_item_row: position.2,
            },
        );
    }
}

type GalleryPreviewPosition = (Option<usize>, Option<usize>, Option<usize>);

fn gallery_preview_positions(
    app: &AppWindow,
    collection: PreviewCollection,
) -> BTreeMap<(String, String), GalleryPreviewPosition> {
    let state = app.global::<AppState>();
    let flat = match collection {
        PreviewCollection::Assets => state.get_assets(),
        PreviewCollection::Generations => state.get_generations(),
        PreviewCollection::Inspiration => state.get_inspiration(),
    };
    let mut positions = BTreeMap::new();
    for row in 0..flat.row_count() {
        let Some(item) = flat.row_data(row) else {
            continue;
        };
        positions.insert(
            (item.id.to_string(), item.source_path.to_string()),
            (Some(row), None, None),
        );
    }
    let groups = match collection {
        PreviewCollection::Assets => Some(state.get_asset_groups()),
        PreviewCollection::Generations => Some(state.get_generation_groups()),
        PreviewCollection::Inspiration => None,
    };
    if let Some(groups) = groups {
        for group_row in 0..groups.row_count() {
            let Some(group) = groups.row_data(group_row) else {
                continue;
            };
            for item_row in 0..group.items.row_count() {
                let Some(item) = group.items.row_data(item_row) else {
                    continue;
                };
                if let Some(position) = positions
                    .get_mut(&(item.id.to_string(), item.source_path.to_string()))
                {
                    position.1 = Some(group_row);
                    position.2 = Some(item_row);
                }
            }
        }
    }
    positions
}

pub(super) fn apply_gallery_preview(app: &AppWindow, target: &PreviewTarget, image: Image) {
    let state = app.global::<AppState>();
    match target.collection {
        PreviewCollection::Assets => {
            let items = state.get_assets();
            if !update_asset_preview_row_at(
                &items,
                target.flat_row,
                &target.asset_id,
                &target.source_path,
                &image,
            ) {
                update_asset_preview_rows(
                    &items,
                    &target.asset_id,
                    &target.source_path,
                    &image,
                );
            }
            let groups = state.get_asset_groups();
            if !update_asset_preview_group_at(&groups, target, &image) {
                update_asset_preview_groups(
                    &groups,
                    &target.asset_id,
                    &target.source_path,
                    &image,
                );
            }
        }
        PreviewCollection::Generations => {
            let items = state.get_generations();
            if !update_asset_preview_row_at(
                &items,
                target.flat_row,
                &target.asset_id,
                &target.source_path,
                &image,
            ) {
                update_asset_preview_rows(
                    &items,
                    &target.asset_id,
                    &target.source_path,
                    &image,
                );
            }
            let groups = state.get_generation_groups();
            if !update_asset_preview_group_at(&groups, target, &image) {
                update_asset_preview_groups(
                    &groups,
                    &target.asset_id,
                    &target.source_path,
                    &image,
                );
            }
        }
        PreviewCollection::Inspiration => {
            let items = state.get_inspiration();
            if !update_asset_preview_row_at(
                &items,
                target.flat_row,
                &target.asset_id,
                &target.source_path,
                &image,
            ) {
                update_asset_preview_rows(
                    &items,
                    &target.asset_id,
                    &target.source_path,
                    &image,
                );
            }
        }
    }
}

fn update_asset_preview_row_at(
    items: &ModelRc<AssetItem>,
    row: Option<usize>,
    asset_id: &str,
    source_path: &str,
    image: &Image,
) -> bool {
    let Some(row) = row else {
        return false;
    };
    let Some(mut item) = items.row_data(row) else {
        return false;
    };
    if item.id.as_str() != asset_id || item.source_path.as_str() != source_path {
        return false;
    }
    item.image = image.clone();
    items.set_row_data(row, item);
    true
}

fn update_asset_preview_group_at(
    groups: &ModelRc<AssetGroup>,
    target: &PreviewTarget,
    image: &Image,
) -> bool {
    let Some(group_row) = target.group_row else {
        return false;
    };
    let Some(group) = groups.row_data(group_row) else {
        return false;
    };
    update_asset_preview_row_at(
        &group.items,
        target.group_item_row,
        &target.asset_id,
        &target.source_path,
        image,
    )
}

fn update_asset_preview_rows(
    items: &ModelRc<AssetItem>,
    asset_id: &str,
    source_path: &str,
    image: &Image,
) -> bool {
    for row in 0..items.row_count() {
        let Some(mut item) = items.row_data(row) else {
            continue;
        };
        if item.id.as_str() == asset_id && item.source_path.as_str() == source_path {
            item.image = image.clone();
            items.set_row_data(row, item);
            return true;
        }
    }
    false
}

fn update_asset_preview_groups(
    groups: &ModelRc<AssetGroup>,
    asset_id: &str,
    source_path: &str,
    image: &Image,
) {
    for row in 0..groups.row_count() {
        let Some(group) = groups.row_data(row) else {
            continue;
        };
        if update_asset_preview_rows(&group.items, asset_id, source_path, image) {
            return;
        }
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
                    flow_reversed: false,
                },
                CanvasLinkData {
                    id: "two".to_string(),
                    source_id: "style".to_string(),
                    target_id: "image".to_string(),
                    flow_reversed: false,
                },
            ],
            ..Store::default()
        };

        assert_eq!(
            canvas_linked_inputs(&store)
                .remove("image")
                .unwrap_or_default(),
            "雨夜城市\n电影感霓虹灯"
        );
        assert_eq!(meaningful_canvas_content(&store.canvas_notes[2]), "");
    }
}
