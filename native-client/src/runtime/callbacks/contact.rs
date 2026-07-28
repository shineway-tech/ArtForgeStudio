use super::*;

fn dismiss_contact_popup(app: &AppWindow, store: &Rc<RefCell<Store>>) {
    app.global::<AppState>().set_contact_popup_open(false);
    let mut store_mut = store.borrow_mut();
    if !store_mut.contact_popup_dismissed {
        store_mut.contact_popup_dismissed = true;
        save_local_store(app, &store_mut);
    }
}

pub(super) fn wire_contact_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_dismiss_contact_popup(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            dismiss_contact_popup(&app, &store);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_open_contact_settings(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            dismiss_contact_popup(&app, &store);
            let state = app.global::<AppState>();
            state.set_settings_section("contact".into());
            state.set_page("settings".into());
        });
    }
}
