use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
use slint::{platform::{PointerEventButton, WindowEvent}, ComponentHandle, LogicalPosition, Model, ModelRc, VecModel};

slint::slint! {
    import { CanvasSplitGuides } from "../ui/components/canvas-split-guides.slint";
    export component SplitTestWindow inherits Window {
        width: 1000px;
        height: 600px;
        in property <float> zoom: 1;
        in-out property <[float]> rows;
        in-out property <[float]> columns;
        out property <int> background-presses: 0;
        TouchArea {
            pointer-event(event) => {
                if event.kind == PointerEventKind.down { root.background-presses += 1; }
            }
        }
        CanvasSplitGuides {
            x: 50px; y: 50px;
            width: 400px * root.zoom; height: 200px * root.zoom;
            scale-factor: root.zoom;
            row-positions <=> root.rows;
            column-positions <=> root.columns;
        }
    }
}

fn drag(app: &SplitTestWindow, from: (f32, f32), to: (f32, f32)) {
    let from = LogicalPosition::new(from.0, from.1);
    let to = LogicalPosition::new(to.0, to.1);
    app.window().dispatch_event(WindowEvent::PointerMoved { position: from });
    app.window().dispatch_event(WindowEvent::PointerPressed { position: from, button: PointerEventButton::Left });
    app.window().dispatch_event(WindowEvent::PointerMoved { position: to });
    app.window().dispatch_event(WindowEvent::PointerReleased { position: to, button: PointerEventButton::Left });
}

#[test]
fn guides_drag_independently_at_different_zoom_levels_without_moving_the_node() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true, renderer_name: Some("software".into()), ..Default::default()
    }))).unwrap();
    let app = SplitTestWindow::new().unwrap();
    app.show().unwrap();
    assert_eq!(app.get_rows().row_count(), 0);
    assert_eq!(app.get_columns().row_count(), 0);
    for zoom in [0.5, 1.0, 2.0] {
        app.set_zoom(zoom);
        app.set_rows(ModelRc::new(VecModel::from(vec![0.5])));
        app.set_columns(ModelRc::new(VecModel::from(vec![0.3, 0.7])));
        drag(&app, (50.0 + 120.0 * zoom, 50.0 + 30.0 * zoom), (50.0 + 180.0 * zoom, 50.0 + 30.0 * zoom));
        assert!((app.get_columns().row_data(0).unwrap() - 0.45).abs() < 0.001);
        assert_eq!(app.get_columns().row_data(1), Some(0.7));
        assert_eq!(app.get_rows().row_data(0), Some(0.5));
        drag(&app, (50.0 + 40.0 * zoom, 50.0 + 100.0 * zoom), (50.0 + 40.0 * zoom, 50.0 + 150.0 * zoom));
        assert!((app.get_rows().row_data(0).unwrap() - 0.75).abs() < 0.001);
        // Moving past the next guide must not reverse the cut order.
        drag(&app, (50.0 + 180.0 * zoom, 50.0 + 30.0 * zoom), (50.0 + 390.0 * zoom, 50.0 + 30.0 * zoom));
        assert!(app.get_columns().row_data(0).unwrap() < 0.7);
        assert_eq!(app.get_background_presses(), 0);
    }
}
