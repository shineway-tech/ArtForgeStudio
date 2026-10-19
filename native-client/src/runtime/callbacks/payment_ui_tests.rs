use super::*;
use i_slint_backend_testing::ElementHandle;
use slint::ComponentHandle;

fn element(app: &AppWindow, id: &str) -> Option<ElementHandle> {
    ElementHandle::find_by_element_id(app, &format!("AlipayPaymentDialog::{id}")).next()
}

#[test]
fn payment_entry_stays_visible_while_checkout_is_active() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { mock_time:true, renderer_name:Some("software".into()), ..Default::default() },
    ))).unwrap();
    let app=AppWindow::new().unwrap();let state=app.global::<AppState>();
    state.set_contact_popup_open(false);state.set_logged_in(true);state.set_page("credits".into());
    state.set_payment_active(true);state.set_payment_dialog_open(false);state.set_payment_amount("¥ 99.00".into());
    state.set_membership_plan_code("free".into());state.set_membership_plan_name("免费方案".into());
    state.set_membership_plans(ModelRc::new(VecModel::from(vec![MembershipPlanView {
        code:"plus".into(),name:"测试会员".into(),price:"¥ 99.00".into(),grant_credits:"100".into(),period_days:30,tier_rank:1,
    }])));
    let retries=Rc::new(Cell::new(0));let observed=retries.clone();
    state.on_retry_payment_browser(move||observed.set(observed.get()+1));
    app.show().unwrap();
    for theme in ["light","deep"] {
        apply_theme(&app,theme);
        for (width,height) in [(1180.0,760.0),(1440.0,900.0)] {
            app.window().set_size(slint::LogicalSize::new(width,height));
            for tab in ["recharge","membership"] {
                state.set_credits_tab(tab.into());state.set_payment_kind(if tab=="recharge" {"membership"}else{"credit"}.into());
                state.set_payment_dialog_mode("waiting".into());
                let card=ElementHandle::find_by_element_id(&app,"CreditsPage::payment-progress").next().unwrap();
                let action=ElementHandle::find_by_element_id(&app,"CreditsPage::payment-action").next().unwrap();
                let pos=card.absolute_position();let size=card.size();
                assert!(pos.x>=0.0 && pos.y>=0.0 && pos.x+size.width<=width && pos.y+size.height<=height);
                let before=retries.get();action.mock_single_click(slint::platform::PointerEventButton::Left);
                assert_eq!(retries.get(),before+1,"active checkout must offer a working entry");
                if let Some(dir)=std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
                    let dir=PathBuf::from(dir);fs::create_dir_all(&dir).unwrap();
                    let pixels=app.window().take_snapshot().unwrap();
                    image::save_buffer(dir.join(format!("payment-active-{tab}-{theme}-{}x{}.png",width as u32,height as u32)),
                        pixels.as_bytes(),pixels.width(),pixels.height(),image::ColorType::Rgba8).unwrap();
                }
            }
        }
    }
}

#[test]
fn payment_dialog_states_fit_and_remove_the_code_after_confirmation() {
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
    state.set_payment_amount("¥ 99.00".into());
    state.set_payment_payer("个人账号".into());
    state.set_payment_countdown("04:59".into());
    state.set_payment_status_message("等待扫码，支付结果将自动更新".into());
    state.set_payment_success_message("积分已到账".into());
    state.set_payment_success_detail("账户余额已更新，可以继续创作".into());
    let dismissals = Rc::new(Cell::new(0));
    let observed = dismissals.clone();
    state.on_dismiss_payment(move || observed.set(observed.get() + 1));
    let refreshes = Rc::new(Cell::new(0));
    let observed = refreshes.clone();
    state.on_retry_payment_browser(move || observed.set(observed.get() + 1));
    app.show().unwrap();
    for theme in ["light", "deep"] {
        apply_theme(&app, theme);
        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0)] {
            app.window()
                .set_size(slint::LogicalSize::new(width, height));
            for mode in [
                "waiting",
                "expired",
                "error",
                "closed",
                "fulfilling",
                "success",
            ] {
                state.set_payment_dialog_open(false);
                state.set_payment_dialog_mode(mode.into());
                state.set_payment_browser_ready(mode == "waiting");
                state.set_payment_countdown(if mode == "waiting" { "04:59" } else { "" }.into());
                state.set_payment_status_message(
                    match mode {
                        "expired" => "二维码已失效，正在确认原订单状态",
                        "error" => "二维码加载失败，请刷新后重试",
                        "closed" => "订单已关闭，请返回重新下单",
                        _ => "等待扫码，支付结果将自动更新",
                    }
                    .into(),
                );
                state.set_payment_dialog_open(true);
                let card = element(&app, "card").unwrap();
                i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
                slint::platform::update_timers_and_animations();
                let pos = card.absolute_position();
                let size = card.size();
                assert!(pos.x >= 0.0 && pos.y >= 0.0 && pos.y + size.height <= height);
                assert!((pos.x * 2.0 + size.width - width).abs() < 1.0);
                assert_eq!(
                    element(&app, "qr-stage").is_some(),
                    !matches!(mode, "fulfilling" | "success")
                );
                let (upper, lower) = if mode == "success" {
                    (
                        element(&app, "success-mark").unwrap(),
                        element(&app, "success-title").unwrap(),
                    )
                } else {
                    (
                        element(&app, "amount").unwrap(),
                        element(&app, "product-label").unwrap(),
                    )
                };
                assert!(
                    upper.absolute_position().y + upper.size().height + 4.0
                        <= lower.absolute_position().y,
                    "payment receipt content must not overlap in {mode}"
                );
                let action =
                    element(&app, if mode == "success" { "done" } else { "later" }).unwrap();
                assert!(action.absolute_position().y + action.size().height < pos.y + size.height);
                if mode == "expired" {
                    let before = refreshes.get();
                    element(&app, "refresh")
                        .unwrap()
                        .mock_single_click(slint::platform::PointerEventButton::Left);
                    assert_eq!(refreshes.get(), before + 1);
                }
                if mode == "waiting" {
                    let before = refreshes.get();
                    element(&app, "refresh-link").unwrap()
                        .mock_single_click(slint::platform::PointerEventButton::Left);
                    assert_eq!(refreshes.get(), before + 1);
                    let before = dismissals.get();
                    action.mock_single_click(slint::platform::PointerEventButton::Left);
                    assert_eq!(dismissals.get(), before + 1);
                }
                if let Some(dir) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
                    let dir = PathBuf::from(dir);
                    fs::create_dir_all(&dir).unwrap();
                    let pixels = app.window().take_snapshot().unwrap();
                    image::save_buffer(
                        dir.join(format!(
                            "payment-{mode}-{theme}-{}x{}.png",
                            width as u32, height as u32
                        )),
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
}
