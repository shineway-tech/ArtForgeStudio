use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
use slint::{
    platform::{PointerEventButton, WindowEvent},
    ComponentHandle, LogicalPosition,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

slint::slint! {
    import { Sidebar } from "../ui/components/sidebar.slint";
    import { TopBar } from "../ui/components/top-bar.slint";
    import { AppState } from "../ui/app-state.slint";
    import { AppTheme } from "../ui/theme.slint";
    export { AppState, AppTheme }
    export component SidebarTestWindow inherits Window {
        width: 1180px;
        height: 728px;
        sidebar := Sidebar { x: 0px; y: 0px; height: parent.height; }
        TopBar { x: sidebar.width; y: 0px; width: parent.width - sidebar.width; }
    }
}

fn click_at(app: &SidebarTestWindow, position: LogicalPosition) {
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn menu_click(app: &SidebarTestWindow, avatar: &ElementHandle, id: &str) {
    let panel = ElementHandle::find_by_element_id(app, "SidebarAccountButton::account-popup-panel")
        .next()
        .unwrap();
    let row = ElementHandle::find_by_element_id(app, &format!("SidebarAccountButton::{id}"))
        .next()
        .unwrap();
    click_at(
        app,
        LogicalPosition::new(
            avatar.absolute_position().x
                + avatar.size().width
                + 12.0
                + row.absolute_position().x
                + row.size().width / 2.0,
            avatar.absolute_position().y + avatar.size().height - panel.size().height
                + row.absolute_position().y
                + row.size().height / 2.0,
        ),
    );
    assert!(
        ElementHandle::find_by_element_id(app, "SidebarAccountButton::account-popup-panel")
            .next()
            .is_none(),
        "menu must close after choosing an action"
    );
}

#[test]
fn bottom_account_controls_keep_routes_and_menu_fits_collapsed_and_expanded_sidebar() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        renderer_name: Some("software".into()),
        ..Default::default()
    })))
    .unwrap();
    let app = SidebarTestWindow::new().unwrap();
    let state = app.global::<AppState>();
    let theme = app.global::<AppTheme>();
    theme.set_panel(slint::Color::from_rgb_u8(24, 24, 24));
    theme.set_panel_soft(slint::Color::from_rgb_u8(36, 40, 52));
    theme.set_bg(slint::Color::from_rgb_u8(12, 14, 22));
    theme.set_border(slint::Color::from_rgb_u8(42, 42, 44));
    theme.set_text(slint::Color::from_rgb_u8(244, 244, 246));
    theme.set_muted(slint::Color::from_rgb_u8(170, 174, 182));
    theme.set_accent(slint::Color::from_rgb_u8(14, 165, 233));
    theme.set_accent_dark(slint::Color::from_rgb_u8(14, 165, 233));
    state.set_logged_in(true);
    state.set_page("generation".into());
    state.set_nickname("Very long display name for layout".into());
    state.set_account_amount_label("197950 积分".into());
    state.set_has_unread(true);
    state.set_update_available(true);
    let destination = Rc::new(RefCell::new(String::new()));
    let target = destination.clone();
    state.on_navigate(move |page| *target.borrow_mut() = page.to_string());
    let logout_count = Rc::new(Cell::new(0));
    let counter = logout_count.clone();
    state.on_logout(move || counter.set(counter.get() + 1));
    app.show().unwrap();
    for (language, collapsed) in [("zh", false), ("zh", true), ("en", false), ("en", true)] {
        state.set_language(language.into());
        state.set_sidebar_collapsed(collapsed);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
        let avatar = ElementHandle::find_by_element_type_name(&app, "SidebarAccountButton")
            .next()
            .expect("account entry must move from top bar to sidebar bottom");
        let credits = ElementHandle::find_by_element_type_name(&app, "SidebarCreditsButton")
            .next()
            .unwrap();
        let notification =
            ElementHandle::find_by_element_id(&app, "Sidebar::sidebar-notifications")
                .next()
                .unwrap();
        assert!(
            credits.size().width <= if collapsed { 56.0 } else { 80.0 },
            "credits entry must stay compact instead of filling the sidebar"
        );
        assert!(credits.absolute_position().y > 520.0);
        assert!(
            credits.absolute_position().y + credits.size().height
                <= notification.absolute_position().y
        );
        assert!(
            notification.absolute_position().y + notification.size().height
                <= avatar.absolute_position().y
        );
        assert!(avatar.absolute_position().y + avatar.size().height <= 728.0);
        assert!(
            avatar.absolute_position().x + avatar.size().width
                <= if collapsed { 72.0 } else { 204.0 }
        );
        credits.mock_single_click(PointerEventButton::Left);
        assert_eq!(&*destination.borrow(), "credits");
        notification.mock_single_click(PointerEventButton::Left);
        assert_eq!(&*destination.borrow(), "notifications");
        state.set_profile_open(false);
        avatar.mock_single_click(PointerEventButton::Left);
        assert!(
            !state.get_profile_open(),
            "avatar opens menu first, not the profile dialog"
        );
        let panel =
            ElementHandle::find_by_element_id(&app, "SidebarAccountButton::account-popup-panel")
                .next()
                .unwrap();
        assert!(avatar.absolute_position().y + avatar.size().height - panel.size().height >= 0.0);
        assert!(
            avatar.absolute_position().x + avatar.size().width + 12.0 + panel.size().width
                <= 1180.0
        );
        let rows =
            ElementHandle::find_by_element_type_name(&app, "AccountMenuRow").collect::<Vec<_>>();
        assert_eq!(rows.len(), 3);
        assert!(ElementHandle::find_by_element_id(
            &app,
            "SidebarAccountButton::account-theme"
        )
        .next()
        .is_none());
        assert!(ElementHandle::find_by_element_id(
            &app,
            "SidebarAccountButton::account-about"
        )
        .next()
        .is_none());
        for row in &rows {
            assert!(row.absolute_position().y + row.size().height <= panel.size().height - 10.0);
        }
        if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            let pixels = app.window().take_snapshot().unwrap();
            image::save_buffer(
                std::path::PathBuf::from(directory)
                    .join(format!("sidebar-{language}-{collapsed}.png")),
                pixels.as_bytes(),
                pixels.width(),
                pixels.height(),
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
        click_at(&app, LogicalPosition::new(1100.0, 500.0));
        assert!(ElementHandle::find_by_element_id(
            &app,
            "SidebarAccountButton::account-popup-panel"
        )
        .next()
        .is_none());
        assert!(!state.get_profile_open());
        avatar.mock_single_click(PointerEventButton::Left);
        menu_click(&app, &avatar, "account-profile");
        assert!(state.get_profile_open());
        assert_eq!(
            state.get_profile_name(),
            "Very long display name for layout"
        );
        state.set_profile_open(false);
        avatar.mock_single_click(PointerEventButton::Left);
        menu_click(&app, &avatar, "account-settings");
        assert_eq!(&*destination.borrow(), "settings");
        assert_eq!(state.get_settings_section(), "basic");
        avatar.mock_single_click(PointerEventButton::Left);
        menu_click(&app, &avatar, "account-membership");
        assert_eq!(&*destination.borrow(), "credits");
        assert_eq!(state.get_credits_tab(), "membership");
        avatar.mock_single_click(PointerEventButton::Left);
        menu_click(&app, &avatar, "account-logout");
    }
    assert_eq!(logout_count.get(), 4);
    state.set_logged_in(false);
    assert!(
        ElementHandle::find_by_element_type_name(&app, "SidebarCreditsButton")
            .next()
            .is_none()
    );
    let avatar = ElementHandle::find_by_element_type_name(&app, "SidebarAccountButton")
        .next()
        .unwrap();
    avatar.mock_single_click(PointerEventButton::Left);
    menu_click(&app, &avatar, "account-settings");
    assert_eq!(&*destination.borrow(), "settings");
}
