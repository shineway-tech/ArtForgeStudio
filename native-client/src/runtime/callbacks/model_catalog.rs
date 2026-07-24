use super::*;

pub(super) fn wire_model_catalog_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
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
                    save_local_store(&app, &store.borrow());
                }
            }
        });
    }
}
