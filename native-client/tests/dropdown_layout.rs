use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
use slint::{
    platform::{PointerEventButton, WindowEvent},
    ComponentHandle, LogicalPosition,
};

// Exercise the production chips in a small window, without network or user data.
slint::slint! {
    import { CreationModeChip } from "../ui/components/creation-mode-chip.slint";
    import { StyleModeChip } from "../ui/components/style-mode-chip.slint";
    import { AppState } from "../ui/app-state.slint";
    export { AppState }
    export component DropdownTestWindow inherits Window {
        width: 600px;
        height: 600px;
        CreationModeChip { x: 20px; y: 500px; }
        StyleModeChip { x: 220px; y: 500px; }
    }
}

fn test_window() -> DropdownTestWindow {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        renderer_name: Some("software".into()),
        ..Default::default()
    })))
    .unwrap();
    let app = DropdownTestWindow::new().unwrap();
    app.show().unwrap();
    app
}

fn select_last_option(app: &DropdownTestWindow, component: &str, count: usize) {
    let chip = ElementHandle::find_by_element_type_name(app, component)
        .next()
        .unwrap();
    chip.mock_single_click(PointerEventButton::Left);
    let popup = chip
        .query_descendants()
        .match_type_name("VerticalLayout")
        .find_first()
        .expect("open dropdown layout");
    let options = popup
        .query_descendants()
        .match_type_name("TextOption")
        .find_all();
    assert_eq!(options.len(), count);
    let bounds = popup.absolute_position();
    let size = popup.size();
    for option in &options {
        let pos = option.absolute_position();
        assert!(
            pos.x >= bounds.x && pos.x + option.size().width <= bounds.x + size.width,
            "{component} option must fit horizontally"
        );
        assert!(
            pos.y >= bounds.y + 5.0 && pos.y + option.size().height <= bounds.y + size.height - 5.0,
            "{component}: row bottom {} exceeds popup bottom {} including padding",
            pos.y + option.size().height,
            bounds.y + size.height - 5.0
        );
    }
    // This backend's snapshot buffer does not follow simulated DPI changes.
    // Verify scaled bounds/clicks, but capture the reference images at 100%.
    if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR")
        .filter(|_| app.window().scale_factor() == 1.0)
    {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let state = app.global::<AppState>();
        let pixels = app.window().take_snapshot().unwrap();
        let name = format!(
            "{component}-{}-{}-{}.png",
            state.get_asset_type(),
            state.get_language(),
            app.window().scale_factor(),
        );
        image::save_buffer(
            directory.join(name),
            pixels.as_bytes(),
            pixels.width(),
            pixels.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
    let last = options.last().unwrap();
    // Testing-backend popup coordinates are local to the popup window.
    let position = LogicalPosition::new(
        chip.absolute_position().x + last.absolute_position().x + last.size().width / 2.0,
        chip.absolute_position().y - size.height - 8.0
            + last.absolute_position().y
            + last.size().height / 2.0,
    );
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    assert!(
        ElementHandle::find_by_element_type_name(app, "TextOption")
            .next()
            .is_none(),
        "selecting the last option must close the menu"
    );
}

#[test]
fn creation_menu_contains_and_selects_its_last_option_in_every_category() {
    let app = test_window();
    let state = app.global::<AppState>();
    for (language, scale_factor) in [("zh", 1.0), ("zh", 1.5), ("en", 2.0)] {
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
        app.window().set_size(slint::PhysicalSize::new(
            (600.0 * scale_factor) as u32,
            (600.0 * scale_factor) as u32,
        ));
        state.set_language(language.into());
        for (category, count, last) in [
            ("scene", 9, "building-kit"),
            ("character", 9, "character-poster"),
            ("ui", 9, "ui-popup"),
            ("effect", 8, "fx-weapon-trail"),
        ] {
            state.set_asset_type(category.into());
            state.set_creation_mode("free".into());
            select_last_option(&app, "CreationModeChip", count);
            assert_eq!(state.get_creation_mode(), last);
        }
    }
}

#[test]
fn style_menu_contains_and_selects_its_last_option() {
    let app = test_window();
    let state = app.global::<AppState>();
    for (language, scale_factor) in [("zh", 1.0), ("zh", 1.5), ("en", 2.0)] {
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
        app.window().set_size(slint::PhysicalSize::new(
            (600.0 * scale_factor) as u32,
            (600.0 * scale_factor) as u32,
        ));
        state.set_language(language.into());
        state.set_style_mode("free".into());
        select_last_option(&app, "StyleModeChip", 10);
        assert_eq!(state.get_style_mode(), "ghibli");
    }
}
