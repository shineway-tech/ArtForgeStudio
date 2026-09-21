use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
use slint::ComponentHandle;

slint::slint! {
    import { AppState } from "../ui/app-state.slint";
    import { InfiniteCanvasPage } from "../ui/pages/infinite-canvas-page.slint";
    export { AppState }

    export component SideScrollMapCanvasTestWindow inherits Window {
        InfiniteCanvasPage { width: parent.width; height: parent.height; }
    }
}

#[test]
fn side_scroll_map_uses_the_shared_infinite_canvas_layout() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        renderer_name: Some("software".into()),
        ..Default::default()
    })))
    .unwrap();
    let app = SideScrollMapCanvasTestWindow::new().unwrap();
    let state = app.global::<AppState>();
    state.set_logged_in(true);
    state.set_page("canvas".into());
    state.set_canvas_workflow_id("side-scroll-map".into());
    state.set_canvas_workflow_title("横版地图生成器".into());
    state.set_canvas_workflow_template("横版无缝地图规范：".into());
    state.set_canvas_workflow_hint("上传风格参考图，生成前中后景分层、首尾无缝衔接的横版地图".into());
    state.set_ratio("21:9".into());
    state.set_count(1);
    app.show().unwrap();

    assert_eq!(state.get_page().as_str(), "canvas");
    assert_eq!(state.get_canvas_workflow_id().as_str(), "side-scroll-map");
    assert_eq!(state.get_ratio().as_str(), "21:9");
    assert_eq!(state.get_count(), 1);

    for (width, height, artifact) in [
        (960.0, 692.0, "side-scroll-map-canvas-minimum.png"),
        (1220.0, 832.0, "side-scroll-map-canvas-preferred.png"),
    ] {
        app.window().set_size(slint::LogicalSize::new(width, height));

        let pixels = app.window().take_snapshot().unwrap();
        assert_eq!(pixels.width(), width as u32);
        assert_eq!(pixels.height(), height as u32);
        assert!(pixels.as_bytes().chunks_exact(4).any(|pixel| pixel[3] != 0));
        if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            image::save_buffer(
                std::path::PathBuf::from(directory).join(artifact),
                pixels.as_bytes(),
                pixels.width(),
                pixels.height(),
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
}
