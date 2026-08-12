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
        state.on_copy_contact_detail(move |value| {
            let value = value.trim();
            if value.is_empty() {
                return;
            }
            let Ok(mut clipboard) = arboard::Clipboard::new() else {
                return;
            };
            if clipboard.set_text(value.to_owned()).is_ok() {
                if let Some(app) = app_weak.upgrade() {
                    let state = app.global::<AppState>();
                    let current_sequence = state.get_contact_copy_sequence();
                    let sequence = if current_sequence == i32::MAX {
                        1
                    } else {
                        current_sequence + 1
                    };
                    state.set_contact_copy_sequence(sequence);
                    state.set_contact_copy_toast_visible(true);
                    let app_weak = app.as_weak();
                    slint::Timer::single_shot(Duration::from_millis(1400), move || {
                        let Some(app) = app_weak.upgrade() else {
                            return;
                        };
                        let state = app.global::<AppState>();
                        if state.get_contact_copy_sequence() == sequence {
                            state.set_contact_copy_toast_visible(false);
                        }
                    });
                }
            }
        });
    }

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
            navigate_to_with_store(&app, &store.borrow(), "settings");
            refresh_storage_usage_async(&app);
        });
    }
}
