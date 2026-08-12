use super::*;

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
}
