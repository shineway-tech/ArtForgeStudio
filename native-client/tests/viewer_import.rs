use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
use slint::{platform::PointerEventButton, ComponentHandle};
use std::{cell::RefCell, rc::Rc};

slint::slint! {
    import { ViewerOverlay } from "../ui/dialogs/viewer-overlay.slint";
    import { AppState } from "../ui/app-state.slint";
    export { AppState }
    export component ViewerTestWindow inherits Window {
        width: 1180px;
        height: 760px;
        ViewerOverlay { width: parent.width; height: parent.height; }
    }
}

#[test]
fn import_waits_for_a_workflow_choice_and_cancel_preserves_the_viewer() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        renderer_name: Some("software".into()),
        ..Default::default()
    })))
    .unwrap();
    let app = ViewerTestWindow::new().unwrap();
    let state = app.global::<AppState>();
    state.set_viewer_open(true);
    state.set_viewer_source("asset".into());
    state.set_viewer_category("other".into());
    state.set_viewer_source_path("unchanged.png".into());
    state.set_page("assets".into());
    let imports = Rc::new(RefCell::new(Vec::<String>::new()));
    let calls = imports.clone();
    state.on_viewer_import_to_canvas(move || calls.borrow_mut().push("infinite-canvas".into()));
    let calls = imports.clone();
    state.on_viewer_open_creation_workflow(move |id, title, template, hint| {
        assert!(!title.is_empty());
        assert!(!template.is_empty());
        assert!(!hint.is_empty());
        calls.borrow_mut().push(id.to_string());
    });
    app.show().unwrap();
    let open_picker = || {
        ElementHandle::find_by_accessible_label(&app, "导入")
            .next()
            .or_else(|| ElementHandle::find_by_accessible_label(&app, "Import").next())
            .expect("import footer button")
            .mock_single_click(PointerEventButton::Left);
    };
    open_picker();
    assert!(
        imports.borrow().is_empty(),
        "opening the picker must not import immediately"
    );
    assert_eq!(
        ElementHandle::find_by_element_type_name(&app, "FreeCanvasCard").count(),
        8
    );
    for card in ElementHandle::find_by_element_type_name(&app, "FreeCanvasCard") {
        let pos = card.absolute_position();
        assert!(pos.x >= 24.0 && pos.y >= 24.0);
        assert!(pos.x + card.size().width <= 1156.0);
        assert!(pos.y + card.size().height <= 736.0);
    }
    if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        let pixels = app.window().take_snapshot().unwrap();
        image::save_buffer(
            std::path::PathBuf::from(directory).join("viewer-import.png"),
            pixels.as_bytes(),
            pixels.width(),
            pixels.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
    ElementHandle::find_by_element_type_name(&app, "DialogCloseButton")
        .next()
        .expect("cancel picker")
        .mock_single_click(PointerEventButton::Left);
    assert_eq!(
        ElementHandle::find_by_element_type_name(&app, "FreeCanvasCard").count(),
        0
    );
    assert_eq!(state.get_viewer_source_path(), "unchanged.png");
    assert_eq!(state.get_page(), "assets");
    assert!(state.get_viewer_open());
    for (index, expected) in [
        "plant-growth",
        "character-outfit",
        "monster-generator",
        "upgrade-evolution",
        "character-age",
        "character-body",
        "building-derivation",
        "infinite-canvas",
    ]
    .into_iter()
    .enumerate()
    {
        open_picker();
        ElementHandle::find_by_element_type_name(&app, "FreeCanvasCard")
            .nth(index)
            .expect("workflow card")
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(imports.borrow().last().unwrap(), expected);
        assert_eq!(imports.borrow().len(), index + 1);
        assert_eq!(
            ElementHandle::find_by_element_type_name(&app, "FreeCanvasCard").count(),
            0
        );
    }
    state.set_language("en".into());
    for factor in [1.0, 1.5, 2.0] {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: factor,
            });
        app.window().set_size(slint::PhysicalSize::new(
            (1180.0 * factor) as u32,
            (760.0 * factor) as u32,
        ));
        open_picker();
        for card in ElementHandle::find_by_element_type_name(&app, "FreeCanvasCard") {
            let pos = card.absolute_position();
            assert!(pos.x >= 24.0 && pos.y >= 24.0);
            assert!(pos.x + card.size().width <= 1156.0);
            assert!(pos.y + card.size().height <= 736.0);
        }
        ElementHandle::find_by_element_type_name(&app, "FreeCanvasCard")
            .last()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(imports.borrow().last().unwrap(), "infinite-canvas");
        assert_eq!(
            ElementHandle::find_by_element_type_name(&app, "FreeCanvasCard").count(),
            0
        );
    }
}
