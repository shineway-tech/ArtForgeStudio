use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
use slint::{
    platform::{PointerEventButton, WindowEvent},
    ComponentHandle, LogicalPosition,
};
use std::{cell::RefCell, rc::Rc, sync::Once};

slint::slint! {
    import { ThumbnailCard } from "../ui/components/thumbnail-card.slint";
    import { AppState } from "../ui/app-state.slint";
    import { AssetItem } from "../ui/types.slint";
    export { AppState, AssetItem }

    export component ThumbnailTestWindow inherits Window {
        width: 240px;
        height: 240px;
        in property <AssetItem> item;

        ThumbnailCard {
            item: root.item;
            source: "generation";
            card-width: 200px;
            card-gap: 18px;
        }
    }
}

fn init_testing_backend() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        })))
        .unwrap();
    });
}

#[test]
fn clicking_anywhere_on_a_new_generation_card_dismisses_the_badge_and_opens_the_viewer() {
    init_testing_backend();

    let app = ThumbnailTestWindow::new().unwrap();
    app.set_item(AssetItem {
        id: "new-generation".into(),
        category: "character".into(),
        width: 1024,
        height: 1024,
        source_path: "new-generation.png".into(),
        is_new: true,
        ..Default::default()
    });

    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let state = app.global::<AppState>();
    let observed_calls = calls.clone();
    state.on_dismiss_new_generation(move |id| {
        observed_calls.borrow_mut().push(format!("dismiss:{id}"));
    });
    let observed_calls = calls.clone();
    state.on_open_viewer(move |id, source| {
        observed_calls
            .borrow_mut()
            .push(format!("open:{id}:{source}"));
    });

    app.show().unwrap();
    let card_center = LogicalPosition::new(100.0, 100.0);
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: card_center,
    });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: card_center,
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: card_center,
        button: PointerEventButton::Left,
    });

    assert_eq!(
        calls.borrow().as_slice(),
        ["dismiss:new-generation", "open:new-generation:generation"]
    );
}

#[test]
fn dragging_a_thumbnail_dispatches_preview_and_native_file_callbacks() {
    init_testing_backend();
    let app = ThumbnailTestWindow::new().unwrap();
    app.set_item(AssetItem {
        id: "drag-generation".into(),
        category: "scene".into(),
        width: 1024,
        height: 1024,
        source_path: "C:/legacy/output/drag-generation.png".into(),
        ..Default::default()
    });
    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let state = app.global::<AppState>();
    let observed = calls.clone();
    state.on_start_thumbnail_drag_preview(move |path| {
        observed.borrow_mut().push(format!("preview:{path}"));
        true
    });
    let observed = calls.clone();
    state.on_start_thumbnail_file_drag(move |path| {
        observed.borrow_mut().push(format!("file:{path}"));
        true
    });
    app.show().unwrap();

    let pressed = LogicalPosition::new(100.0, 100.0);
    app.window().dispatch_event(WindowEvent::PointerMoved { position: pressed });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: pressed,
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(125.0, 100.0),
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(125.0, 100.0),
        button: PointerEventButton::Left,
    });

    assert_eq!(
        calls.borrow().as_slice(),
        [
            "preview:C:/legacy/output/drag-generation.png",
            "file:C:/legacy/output/drag-generation.png",
        ]
    );
}
