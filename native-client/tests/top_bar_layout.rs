use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
use slint::{platform::PointerEventButton, ComponentHandle};
use std::{cell::RefCell, rc::Rc};

slint::slint! {
    import { TopBar } from "../ui/components/top-bar.slint";
    import { AppState } from "../ui/app-state.slint";
    export { AppState }
    export component TopBarTestWindow inherits Window {
        width: 1180px;
        height: 180px;
        TopBar { width: parent.width; }
    }
}

#[test]
fn invitation_pill_fits_beside_model_pickers_and_keeps_navigation() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        renderer_name: Some("software".into()),
        ..Default::default()
    })))
    .unwrap();
    let app = TopBarTestWindow::new().unwrap();
    let state = app.global::<AppState>();
    state.set_page("generation".into());
    state.set_logged_in(true);
    state.set_image_model("image-code".into());
    state.set_image_model_name("gpt-image-2".into());
    state.set_reasoning_model("prompt-code".into());
    state.set_reasoning_model_name("GPT-5.6 Sol".into());
    state.set_video_model("video-code".into());
    state.set_video_model_name("Video Model".into());
    let destination = Rc::new(RefCell::new(String::new()));
    let selected = destination.clone();
    state.on_navigate(move |page| *selected.borrow_mut() = page.to_string());
    app.show().unwrap();
    for language in ["zh", "en"] {
        state.set_language(language.into());
        for payment in [false, true] {
            state.set_payment_active(payment);
            let gift = ElementHandle::find_by_element_type_name(&app, "InvitationGiftButton")
                .next()
                .unwrap();
            assert!(
                gift.size().width >= 96.0,
                "invitation text needs a full pill, not the old icon-only slot"
            );
            let pickers = ElementHandle::find_by_element_type_name(&app, "ModelPicker").collect::<Vec<_>>();
            assert_eq!(pickers.len(), 3, "image, reasoning and video selectors must be visible");
            for picker in pickers {
                assert!(
                    picker.size().width <= 280.0,
                    "generation model controls must use the compact width"
                );
                assert!(
                    picker.absolute_position().x + picker.size().width + 8.0
                        <= gift.absolute_position().x,
                    "model controls must not overlap invitation actions"
                );
            }
            assert!(
                ElementHandle::find_by_element_type_name(&app, "ThemeMenuButton")
                    .next()
                    .is_none(),
                "theme selection belongs in the avatar menu, not the top bar"
            );
            gift.mock_single_click(PointerEventButton::Left);
            assert_eq!(&*destination.borrow(), "invitation-gift");
            if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                let pixels = app.window().take_snapshot().unwrap();
                image::save_buffer(
                    std::path::PathBuf::from(directory)
                        .join(format!("top-bar-{language}-{payment}.png")),
                    pixels.as_bytes(),
                    pixels.width(),
                    pixels.height(),
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
}
