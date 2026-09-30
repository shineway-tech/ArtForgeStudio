use super::*;


pub(super) fn prepare_activation_style_projection(models:Vec<CatalogModelView>,preferred:&str) -> PreparedUiProjection {
    let selection=select_style_analysis_model(models,preferred);
    let mut ui=PreparedUiProjection::default();
    ui.push(selection.available,|state,value|state.set_style_analysis_available(value));
    ui.push(selection.model_code.into(),|state,value|state.set_style_analysis_model_code(value));
    ui.push(selection.display_name.into(),|state,value|state.set_style_analysis_display_name(value));
    ui.push(selection.credit_cost.into(),|state,value|state.set_style_analysis_credit_cost(value));
    ui
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StyleAnalysisSelection {
    pub(super) available: bool,
    pub(super) model_code: String,
    pub(super) display_name: String,
    pub(super) credit_cost: String,
}

impl StyleAnalysisSelection {
    fn unavailable() -> Self {
        Self {
            available: false,
            model_code: String::new(),
            display_name: String::new(),
            credit_cost: String::new(),
        }
    }
}

fn select_style_analysis_model(
    models: impl IntoIterator<Item = CatalogModelView>,
    preferred: &str,
) -> StyleAnalysisSelection {
    let eligible = models
        .into_iter()
        .filter(|model| {
            model.purpose == "prompt_processing"
                && model.supports_style_analysis
                && !model.price_standard.trim().is_empty()
        })
        .collect::<Vec<_>>();
    let selected = eligible
        .iter()
        .find(|model| model.code.as_str() == preferred)
        .or_else(|| eligible.first());
    let Some(selected) = selected else {
        return StyleAnalysisSelection::unavailable();
    };
    StyleAnalysisSelection {
        available: true,
        model_code: selected.code.to_string(),
        display_name: selected.name.to_string(),
        credit_cost: selected.price_standard.to_string(),
    }
}

pub(super) fn resolve_style_analysis_selection(state: &AppState) -> StyleAnalysisSelection {
    select_style_analysis_model(
        state.get_catalog_models().iter(),
        state.get_reasoning_model().as_str(),
    )
}

pub(super) fn apply_style_analysis_selection(
    state: &AppState,
    selection: &StyleAnalysisSelection,
) {
    state.set_style_analysis_available(selection.available);
    state.set_style_analysis_model_code(selection.model_code.clone().into());
    state.set_style_analysis_display_name(selection.display_name.clone().into());
    state.set_style_analysis_credit_cost(selection.credit_cost.clone().into());
}

pub(super) fn sync_style_analysis_selection(state: &AppState) -> StyleAnalysisSelection {
    let selection = resolve_style_analysis_selection(state);
    apply_style_analysis_selection(state, &selection);
    selection
}

pub(super) fn wire_model_catalog_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_all_generations(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let selected = store
                .borrow()
                .generations
                .iter()
                .filter(|item| item.category == category)
                .map(|item| item.id.clone())
                .collect::<BTreeSet<_>>();
            publish_generation_selection(&state, selected);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_toggle_generation_selection(move |id| {
            let Some(app) = app_weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            let mut selected = generation_selection_ids(&state);
            if !selected.remove(id.as_str()) {
                selected.insert(id.to_string());
            }
            publish_generation_selection(&state, selected);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_clear_generation_selection(move || {
            let Some(app) = app_weak.upgrade() else { return; };
            publish_generation_selection(&app.global::<AppState>(), BTreeSet::new());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_update_gallery_viewport(
            move |source,
                  top,
                  viewport_height,
                  viewport_width,
                  layout_mode,
                  card_width,
                  loading_count| {
                if let Some(app) = app_weak.upgrade() {
                    update_gallery_viewport(
                        &app,
                        &store.borrow(),
                        source.as_str(),
                        top,
                        viewport_height,
                        viewport_width,
                        layout_mode.as_str(),
                        card_width,
                        loading_count,
                    );
                }
            },
        );
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_refresh_assets(move || {
            if let Some(app) = app_weak.upgrade() {
                reset_asset_gallery_page(&app);
                push_assets(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_refresh_inspiration(move || {
            if let Some(app) = app_weak.upgrade() {
                reset_inspiration_gallery_page(&app);
                push_inspiration(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_load_more_assets(move || {
            if let Some(app) = app_weak.upgrade() {
                load_more_asset_gallery(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_load_more_generations(move || {
            if let Some(app) = app_weak.upgrade() {
                load_more_generation_gallery(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_load_more_inspiration(move || {
            if let Some(app) = app_weak.upgrade() {
                load_more_inspiration_gallery(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_image_model(move |model| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let selected = state
                    .get_catalog_models()
                    .iter()
                    .find(|item| item.code == model && item.purpose == "image_generation");
                if let Some(selected) = selected {
                    state.set_image_model(selected.code);
                    state.set_image_model_name(selected.name);
                    state.set_image_price_1k(selected.price_1k);
                    state.set_image_price_2k(selected.price_2k);
                    state.set_image_price_4k(selected.price_4k);
                    save_local_store(&app, &store.borrow());
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_reasoning_model(move |model| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let selected = state
                    .get_catalog_models()
                    .iter()
                    .find(|item| item.code == model && item.purpose == "prompt_processing");
                if let Some(selected) = selected {
                    state.set_reasoning_model(selected.code);
                    state.set_reasoning_model_name(selected.name);
                    sync_style_analysis_selection(&state);
                    save_local_store(&app, &store.borrow());
                }
            }
        });
    }
}

fn publish_generation_selection(state: &AppState, selected: BTreeSet<String>) {
    let generations = state.get_generations();
    for row in 0..generations.row_count() {
        let Some(mut item) = generations.row_data(row) else { continue; };
        item.selected = selected.contains(item.id.as_str());
        generations.set_row_data(row, item);
    }
    let selected_count = selected.len().min(i32::MAX as usize) as i32;
    state.set_generation_selected_ids(ModelRc::new(VecModel::from(
        selected.into_iter().map(SharedString::from).collect::<Vec<_>>(),
    )));
    state.set_generation_selected_count(selected_count);
    state.set_generation_selection_mode(selected_count > 0);
    if selected_count == 0 {
        state.set_thumbnail_action_menu_id("".into());
        state.set_thumbnail_action_menu_source("".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_model(
        code: &str,
        purpose: &str,
        supports_style_analysis: bool,
        price_standard: &str,
    ) -> CatalogModelView {
        CatalogModelView {
            code: code.into(),
            name: format!("Model {code}").into(),
            purpose: purpose.into(),
            version: 1,
            capabilities: String::new().into(),
            pricing: String::new().into(),
            price_1k: 0,
            price_2k: 0,
            price_4k: 0,
            price_standard: price_standard.into(),
            video_price_480: String::new().into(),
            video_price_720: String::new().into(),
            video_price_1080: String::new().into(),
            supports_image_edit: false,
            supports_style_analysis,
        }
    }

    #[test]
    fn style_analysis_selector_prefers_supported_reasoning_model_and_server_price() {
        let selection = select_style_analysis_model(
            vec![
                catalog_model("fallback", "prompt_processing", true, "7"),
                catalog_model("preferred", "prompt_processing", true, "5"),
            ],
            "preferred",
        );

        assert!(selection.available);
        assert_eq!(selection.model_code, "preferred");
        assert_eq!(selection.display_name, "Model preferred");
        assert_eq!(selection.credit_cost, "5");
    }

    #[test]
    fn style_analysis_selector_rejects_missing_capability_or_standard_price() {
        let selection = select_style_analysis_model(
            vec![
                catalog_model("no-capability", "prompt_processing", false, "5"),
                catalog_model("no-price", "prompt_processing", true, ""),
                catalog_model("wrong-purpose", "image_generation", true, "5"),
            ],
            "no-capability",
        );

        assert!(!selection.available);
        assert!(selection.model_code.is_empty());
        assert!(selection.display_name.is_empty());
        assert!(selection.credit_cost.is_empty());
    }

    fn generation(id: &str, category: &str) -> AssetData {
        AssetData {
            id: id.into(),
            conversation_id: String::new(),
            title: id.into(),
            category: category.into(),
            kind: "generate".into(),
            time: "2026-09-24 00:00".into(),
            prompt: String::new(),
            ratio: "1:1".into(),
            quality: "1K".into(),
            model: "model".into(),
            origin: "backend".into(),
            width: 0,
            height: 0,
            source_path: "failed".into(),
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
    fn generation_batch_selection_selects_current_category_and_can_be_cleared() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let store = Rc::new(RefCell::new(Store::default()));
        store.borrow_mut().generations = vec![
            generation("scene-a", "scene"),
            generation("scene-b", "scene"),
            generation("character-a", "character"),
        ];
        let state = app.global::<AppState>();
        state.set_asset_type("scene".into());
        state.set_generations(ModelRc::new(VecModel::from(vec![
            to_asset_view_metadata(&store.borrow().generations[0]),
            to_asset_view_metadata(&store.borrow().generations[1]),
        ])));
        wire_model_catalog_callbacks(&app, store);

        state.invoke_select_all_generations();
        assert!(state.get_generation_selection_mode());
        assert_eq!(state.get_generation_selected_count(), 2);
        assert!(state.get_generations().iter().all(|item| item.selected));
        assert_eq!(
            generation_selection_ids(&state),
            BTreeSet::from(["scene-a".to_string(), "scene-b".to_string()])
        );

        state.invoke_toggle_generation_selection("scene-a".into());
        assert_eq!(state.get_generation_selected_count(), 1);
        assert!(!state.get_generations().row_data(0).unwrap().selected);
        assert!(state.get_generations().row_data(1).unwrap().selected);

        state.invoke_clear_generation_selection();
        assert!(!state.get_generation_selection_mode());
        assert_eq!(state.get_generation_selected_count(), 0);
        assert!(state.get_generations().iter().all(|item| !item.selected));
    }
}
