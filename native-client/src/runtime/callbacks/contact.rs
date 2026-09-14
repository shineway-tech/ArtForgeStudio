use super::*;

fn dismiss_contact_popup(app: &AppWindow, store: &Rc<RefCell<Store>>) {
    app.global::<AppState>().set_contact_popup_open(false);
    let mut store_mut = store.borrow_mut();
    if !store_mut.contact_popup_dismissed {
        store_mut.contact_popup_dismissed = true;
        save_local_store(app, &store_mut);
    }
}

pub(super) fn wire_contact_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_copy_contact_detail(move |value| {
            let Some(backend) = context.backend.as_ref() else { return; };
            let Ok(_effect) = backend.api.upgrade_latch().begin_ordinary_blocking_effect() else { return; };
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
                    let upgrade = backend.api.upgrade_latch().clone();
                    slint::Timer::single_shot(Duration::from_millis(1400), move || {
                        let Ok(_effect) = upgrade.begin_ordinary_blocking_effect() else { return; };
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
        let context = context.clone();
        state.on_dismiss_contact_popup(move || {
            let Some(backend) = context.backend.as_ref() else { return; };
            let Ok(_effect) = backend.api.upgrade_latch().begin_ordinary_blocking_effect() else { return; };
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            dismiss_contact_popup(&app, &store);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_open_contact_settings(move || {
            let Some(backend) = context.backend.as_ref() else { return; };
            let Ok(_effect) = backend.api.upgrade_latch().begin_ordinary_blocking_effect() else { return; };
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

#[cfg(test)]
mod tests {
    use super::*;
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::PointerEventButton;

    #[test]
    fn sidebar_contact_opens_contact_page_below_notifications_in_both_widths() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true, renderer_name: Some("software".into()), ..Default::default()
            },
        ))).unwrap();
        let app = AppWindow::new().unwrap();
        let backend = Arc::new(BackendRuntime {
            api: ApiClient::new(ApiClientConfig {
                base_url: reqwest::Url::parse("https://example.invalid").unwrap(),
                app_version: "999.0.0".into(), timeout: Duration::from_secs(1),
            }, DeviceIdentity {
                id: "11111111-1111-4111-8111-111111111111".into(),
                name: "contact-test".into(), platform: "windows".into(),
            }, Arc::new(SessionManager::new(Arc::new(
                crate::runtime::test_support::MemoryRefreshTokenStore::default(),
            )))).unwrap(),
        });
        let context = AppContext { backend: Some(backend), ..Default::default() };
        context.store.borrow_mut().contact_popup_dismissed = true;
        wire_contact_callbacks(&app, context.clone());
        let state = app.global::<AppState>();
        let weak = app.as_weak();
        state.on_navigate(move |page| {
            if let Some(app) = weak.upgrade() { navigate_to(&app, &page); }
        });
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        apply_theme(&app, "light");
        app.window().set_size(slint::LogicalSize::new(1180.0, 760.0));
        app.show().unwrap();

        for collapsed in [false, true] {
            state.set_sidebar_collapsed(collapsed);
            state.set_page("notifications".into());
            state.set_settings_section("basic".into());
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(200));
            slint::platform::update_timers_and_animations();
            let contact = ElementHandle::find_by_element_id(&app, "Sidebar::sidebar-contact")
                .next().expect("contact entry below notifications");
            let sidebar = ElementHandle::find_by_element_type_name(&app, "Sidebar").next().unwrap();
            assert!((sidebar.size().width - if collapsed { 72.0 } else { 204.0 }).abs() < 1.0);
            let notifications = ElementHandle::find_by_element_id(&app, "Sidebar::sidebar-notifications")
                .next().unwrap();
            let account = ElementHandle::find_by_element_type_name(&app, "SidebarAccountButton")
                .next().unwrap();
            assert!(contact.absolute_position().y >= notifications.absolute_position().y + notifications.size().height);
            assert!(account.absolute_position().y >= contact.absolute_position().y + contact.size().height);
            assert!(account.absolute_position().y + account.size().height <= 760.0);
            contact.mock_single_click(PointerEventButton::Left);
            assert_eq!(state.get_page(), "settings");
            assert_eq!(state.get_settings_section(), "contact");
            assert!(!state.get_contact_popup_open());
            let texts: Vec<_> = ElementHandle::find_by_element_type_name(&app, "Text")
                .filter_map(|element| element.accessible_label()).collect();
            assert!(texts.iter().any(|text| text.contains("business@honeykid.cn")), "contact details must be rendered");
            if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
                let directory = PathBuf::from(directory);
                fs::create_dir_all(&directory).unwrap();
                let pixels = app.window().take_snapshot().unwrap();
                let name = if collapsed { "sidebar-contact-collapsed.png" } else { "sidebar-contact.png" };
                image::save_buffer(directory.join(name), pixels.as_bytes(), pixels.width(), pixels.height(), image::ColorType::Rgba8).unwrap();
            }
            notifications.mock_single_click(PointerEventButton::Left);
            assert_eq!(state.get_page(), "notifications", "existing navigation must still work");
        }
    }
}
