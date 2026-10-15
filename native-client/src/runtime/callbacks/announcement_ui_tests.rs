use super::*;
use i_slint_backend_testing::ElementHandle;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::ComponentHandle;

fn announcement_test_app() -> AppWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    let state = app.global::<AppState>();
    state.set_contact_popup_open(false);
    state.set_page("notifications".into());
    state.set_announcement_title("服务恢复与创作任务说明".into());
    state.set_announcement_time("2026-10-15 12:30".into());
    app.window()
        .set_size(slint::LogicalSize::new(1180.0, 760.0));
    app.show().unwrap();
    app.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    app
}

fn settle_announcement_ui() {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
}

fn announcement_element(app: &AppWindow, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(app, &format!("AnnouncementDialog::{id}"))
        .next()
        .unwrap_or_else(|| panic!("visible announcement element: {id}"))
}

fn open_test_announcement(app: &AppWindow) {
    app.global::<AppState>().set_announcement_open(false);
    assert!(
        ElementHandle::find_by_element_id(app, "AnnouncementDialog::card")
            .next()
            .is_none()
    );
    app.global::<AppState>().set_announcement_open(true);
    // Traverse the conditional dialog before advancing its initial focus timer.
    announcement_element(app, "card");
    settle_announcement_ui();
}

fn assert_inside(child: &ElementHandle, container: &ElementHandle) {
    let child_position = child.absolute_position();
    let child_size = child.size();
    let container_position = container.absolute_position();
    let container_size = container.size();
    assert!(child_size.width > 0.0 && child_size.height > 0.0);
    assert!(child_position.x >= container_position.x - 1.0);
    assert!(child_position.y >= container_position.y - 1.0);
    assert!(
        child_position.x + child_size.width <= container_position.x + container_size.width + 1.0
    );
    assert!(
        child_position.y + child_size.height <= container_position.y + container_size.height + 1.0
    );
}

fn save_announcement_snapshot(app: &AppWindow, filename: &str) {
    if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).unwrap();
        let pixels = app.window().take_snapshot().unwrap();
        image::save_buffer(
            directory.join(filename),
            pixels.as_bytes(),
            pixels.width(),
            pixels.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

#[test]
fn announcement_long_body_scrolls_without_moving_footer_across_sizes_fonts_and_themes() {
    let app = announcement_test_app();
    let state = app.global::<AppState>();
    let body = format!(
        "{}\n正文结束：请在通知中心查看后续更新。",
        "服务已恢复。此前提交的创作任务会继续处理，已完成的作品可在素材库查看。\n\n".repeat(48)
    );
    state.set_announcement_body(body.clone().into());
    state.set_announcement_pending_count(2);

    for (theme, screenshot_theme) in [("light", "light"), ("deep", "dark")] {
        apply_theme(&app, theme);
        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0)] {
            app.window()
                .set_size(slint::LogicalSize::new(width, height));
            for (font_size, english) in [(14, false), (22, false), (24, true)] {
                state.set_announcement_open(false);
                state.set_settings_font_size(font_size);
                state.set_language(if english { "en" } else { "zh" }.into());
                state.set_announcement_important(font_size == 14);
                open_test_announcement(&app);
                let card = announcement_element(&app, "card");
                let scroll = announcement_element(&app, "body-scroll");
                let text = announcement_element(&app, "body-text");
                let primary = announcement_element(&app, "primary-button");
                let center = announcement_element(&app, "center-button");
                let close = announcement_element(&app, "close-button");
                let pending = announcement_element(&app, "pending-label");
                let position = card.absolute_position();
                let size = card.size();
                assert!(position.x >= 0.0 && position.y >= 0.0);
                assert!(position.x + size.width <= width && position.y + size.height <= height);
                assert!((position.x * 2.0 + size.width - width).abs() < 1.0);
                assert!((position.y * 2.0 + size.height - height).abs() < 1.0);
                for element in [&scroll, &primary, &center, &close, &pending] {
                    assert_inside(element, &card);
                }
                assert!(
                    scroll.absolute_position().y + scroll.size().height
                        <= pending.absolute_position().y
                );
                assert!(
                    pending.absolute_position().y + pending.size().height
                        <= primary.absolute_position().y
                );
                assert!(
                    center.absolute_position().x + center.size().width
                        < primary.absolute_position().x
                );
                assert_eq!(text.accessible_label().unwrap().as_str(), body);
                assert!(text.size().height > scroll.size().height,
                    "long announcement content must remain scrollable at {width}x{height}, font {font_size}, {theme}");

                let filename = if width == 1180.0 && font_size == 14 {
                    format!("announcement-{screenshot_theme}.png")
                } else {
                    format!(
                        "announcement-{screenshot_theme}-{}x{}-font{font_size}-{}.png",
                        width as u32,
                        height as u32,
                        if english { "en" } else { "zh" }
                    )
                };
                save_announcement_snapshot(&app, &filename);

                let text_top = text.absolute_position().y;
                let primary_position = primary.absolute_position();
                let center_position = center.absolute_position();
                scroll.scroll(0.0, -100_000.0);
                settle_announcement_ui();
                assert!(
                    text.absolute_position().y < text_top,
                    "scrolling must move the announcement body"
                );
                let text_bottom = text.absolute_position().y + text.size().height;
                assert!(text_bottom > scroll.absolute_position().y);
                assert!(
                    text_bottom <= scroll.absolute_position().y + scroll.size().height + 1.0,
                    "the final paragraph must be reachable"
                );
                assert!(
                    (primary.absolute_position().y - primary_position.y).abs() < 1.0,
                    "scrolling the body must not move the primary action"
                );
                assert!(
                    (center.absolute_position().y - center_position.y).abs() < 1.0,
                    "scrolling the body must not move notification-center navigation"
                );
            }
        }
    }

    apply_theme(&app, "light");
    state.set_language("zh".into());
    state.set_settings_font_size(14);
    state.set_announcement_title("服务维护通知".into());
    state.set_announcement_body(
        "我们将于今晚 23:00–23:30 进行服务升级。\n\n升级期间，部分创作任务可能需要稍作等待。已完成的作品会正常保留，请提前保存正在编辑的内容。\n\n服务恢复后，你可以继续创作。感谢理解与支持。".into(),
    );
    state.set_announcement_important(true);
    state.set_announcement_pending_count(0);
    app.window()
        .set_size(slint::LogicalSize::new(1180.0, 760.0));
    open_test_announcement(&app);
    save_announcement_snapshot(&app, "announcement-preview.png");
}

#[test]
fn announcement_backdrop_preserves_dialog_and_close_actions_dispatch_once() {
    let app = announcement_test_app();
    let state = app.global::<AppState>();
    state.set_announcement_body("公告正文保持可读，关闭后仍可在通知中心查看。".into());
    let closes = Rc::new(Cell::new(0));
    let centers = Rc::new(Cell::new(0));
    let close_count = closes.clone();
    let weak = app.as_weak();
    state.on_close_announcement(move || {
        close_count.set(close_count.get() + 1);
        weak.upgrade()
            .unwrap()
            .global::<AppState>()
            .set_announcement_open(false);
    });
    let center_count = centers.clone();
    let weak = app.as_weak();
    state.on_view_announcement_center(move || {
        center_count.set(center_count.get() + 1);
        let app = weak.upgrade().unwrap();
        app.global::<AppState>().set_announcement_open(false);
        app.global::<AppState>().set_page("notifications".into());
    });

    open_test_announcement(&app);
    let card = announcement_element(&app, "card");
    let backdrop_position = slint::LogicalPosition::new(
        card.absolute_position().x / 2.0,
        card.absolute_position().y / 2.0,
    );
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: backdrop_position,
    });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: backdrop_position,
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: backdrop_position,
        button: PointerEventButton::Left,
    });
    assert!(state.get_announcement_open());
    assert_eq!((closes.get(), centers.get()), (0, 0));

    announcement_element(&app, "close-button").mock_single_click(PointerEventButton::Left);
    assert!(!state.get_announcement_open());
    assert_eq!((closes.get(), centers.get()), (1, 0));

    open_test_announcement(&app);
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Escape.into(),
    });
    assert!(!state.get_announcement_open());
    assert_eq!((closes.get(), centers.get()), (2, 0));

    open_test_announcement(&app);
    announcement_element(&app, "primary-button").mock_single_click(PointerEventButton::Left);
    assert!(!state.get_announcement_open());
    assert_eq!((closes.get(), centers.get()), (3, 0));

    state.set_page("settings".into());
    open_test_announcement(&app);
    announcement_element(&app, "center-button").mock_single_click(PointerEventButton::Left);
    assert!(!state.get_announcement_open());
    assert_eq!(state.get_page(), "notifications");
    assert_eq!((closes.get(), centers.get()), (3, 1));
}

#[test]
fn announcement_notification_card_reopens_and_delete_keeps_its_own_action() {
    let app = announcement_test_app();
    let state = app.global::<AppState>();
    let id = "announcement-history-item";
    state.set_notifications(ModelRc::new(VecModel::from(vec![NotificationItem {
        id: id.into(),
        title: "服务恢复公告".into(),
        reason: "服务恢复后，之前的任务会继续处理。".into(),
        model: "must-not-be-rendered-for-system-announcements".into(),
        time: "2026-10-15".into(),
        announcement: true,
        success: false,
        read: true,
    }])));
    let opened = Rc::new(RefCell::new(Vec::<String>::new()));
    let opened_ids = opened.clone();
    state.on_open_announcement(move |id| opened_ids.borrow_mut().push(id.to_string()));
    let marked_read = Rc::new(Cell::new(0));
    let read_count = marked_read.clone();
    state.on_mark_notification_read(move |_| read_count.set(read_count.get() + 1));
    settle_announcement_ui();
    let labels: Vec<_> = ElementHandle::find_by_element_type_name(&app, "Text")
        .filter_map(|element| element.accessible_label())
        .collect();
    assert!(labels.iter().any(|label| label.as_str() == "系统公告"));
    assert!(labels
        .iter()
        .any(|label| label.as_str() == "服务恢复后，之前的任务会继续处理。"));
    assert!(!labels
        .iter()
        .any(|label| label.contains("must-not-be-rendered")));

    ElementHandle::find_by_element_id(&app, "NotificationsPage::notification-touch")
        .next()
        .unwrap()
        .mock_single_click(PointerEventButton::Left);
    assert_eq!(opened.borrow().as_slice(), [id]);
    assert_eq!(
        marked_read.get(),
        0,
        "announcement cards must use their detail action"
    );
    assert!(!state.get_delete_confirm_open());

    ElementHandle::find_by_element_id(&app, "NotificationsPage::delete-button")
        .next()
        .unwrap()
        .mock_single_click(PointerEventButton::Left);
    assert!(state.get_delete_confirm_open());
    assert_eq!(state.get_pending_delete_kind(), "notification");
    assert_eq!(state.get_pending_delete_id(), id);
    assert_eq!(
        opened.borrow().as_slice(),
        [id],
        "delete must not also reopen the announcement"
    );
    assert_eq!(marked_read.get(), 0);
}

#[test]
fn announcement_waits_for_an_open_dropdown_and_resumes_after_it_closes() {
    let app = announcement_test_app();
    let state = app.global::<AppState>();
    state.set_page("settings".into());
    state.set_settings_section("basic".into());
    settle_announcement_ui();
    assert!(
        announcement_ui_ready(&app),
        "idle settings must allow an announcement"
    );

    let font_picker = ElementHandle::find_by_element_type_name(&app, "FontPicker")
        .next()
        .expect("visible font dropdown");
    font_picker.mock_single_click(PointerEventButton::Left);
    assert!(
        !announcement_ui_ready(&app),
        "an open dropdown must keep announcement focus away"
    );

    let outside = slint::LogicalPosition::new(0.0, 0.0);
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: outside });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: outside,
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: outside,
        button: PointerEventButton::Left,
    });
    settle_announcement_ui();
    assert!(
        announcement_ui_ready(&app),
        "closing the dropdown must release the announcement gate"
    );
}
