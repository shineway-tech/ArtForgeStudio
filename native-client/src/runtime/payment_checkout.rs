use super::*;

const PAYMENT_CHECKOUT_PATH: &str = "/v1/payments/alipay/checkout";

fn origin_matches(candidate: &reqwest::Url, trusted: &reqwest::Url) -> bool {
    candidate.scheme() == trusted.scheme()
        && candidate.host_str() == trusted.host_str()
        && candidate.port_or_known_default() == trusted.port_or_known_default()
}

fn checkout_scheme_allowed(checkout: &reqwest::Url) -> bool {
    if checkout.scheme() == "https" {
        return true;
    }
    #[cfg(debug_assertions)]
    {
        checkout.scheme() == "http"
            && matches!(checkout.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

fn checkout_fragment_has_session(fragment:&str)->bool {
    let Ok(parsed)=reqwest::Url::parse(&format!("https://checkout-fragment.invalid/?{fragment}"))else{return false;};
    let orders=parsed.query_pairs().filter(|(key,_)|key=="order_id").map(|(_,value)|value.into_owned()).collect::<Vec<_>>();
    let tokens=parsed.query_pairs().filter(|(key,_)|key=="token").map(|(_,value)|value.into_owned()).collect::<Vec<_>>();
    orders.len()==1 && !orders[0].is_empty() && tokens.len()==1 && !tokens[0].is_empty()
}
fn checkout_matches_order(checkout:&reqwest::Url,expected_order_id:&str)->bool {
    if expected_order_id.is_empty(){return false;}
    let Some(fragment)=checkout.fragment()else{return false;};
    let Ok(parsed)=reqwest::Url::parse(&format!("https://checkout-fragment.invalid/?{fragment}"))else{return false;};
    let orders=parsed.query_pairs().filter(|(key,_)|key=="order_id").map(|(_,value)|value.into_owned()).collect::<Vec<_>>();
    orders.len()==1 && orders[0]==expected_order_id
}

fn validated_checkout_url(
    checkout_url: &str,
    trusted_api_base: &reqwest::Url,
) -> Result<reqwest::Url> {
    let checkout = reqwest::Url::parse(checkout_url).context("支付地址无效")?;
    if !checkout_scheme_allowed(&checkout) {
        return Err(anyhow!("支付中转页必须使用 HTTPS"));
    }
    if !origin_matches(&checkout, trusted_api_base)
        || checkout.path() != PAYMENT_CHECKOUT_PATH
        || checkout.query().is_some()
        || !checkout
            .fragment()
            .is_some_and(checkout_fragment_has_session)
        || !checkout.username().is_empty()
        || checkout.password().is_some()
    {
        return Err(anyhow!("支付地址不是受信任的服务端中转页"));
    }
    Ok(checkout)
}

pub(super) fn open_payment_checkout(
    app:&AppWindow, checkout_url:&str,trusted_api_base:&reqwest::Url,persistence:&PrivatePersistence,expected_order_id:&str,
)->Result<()> {
    let checkout=validated_checkout_url(checkout_url,trusted_api_base)?;
    anyhow::ensure!(checkout_matches_order(&checkout,expected_order_id),"支付地址不属于原始订单");
    let(activity,effect)=persistence.begin_effect()?;
    if activity.is_quiescing() || persistence.upgrade_latch().is_tripped() {
        return Err(anyhow!("当前用户操作已关闭"));
    }
    // Counted effect surrounds native WebView creation, but no global short
    // latch is held. Trip waits for this admitted effect to drain.
    let mut embedded = checkout.clone();
    embedded.set_path(PAYMENT_QR_PATH);
    let result=open_embedded_checkout(app,embedded.as_str());
    drop(effect);drop(activity);
    result
}

fn open_embedded_checkout(app:&AppWindow,checkout_url:&str)->Result<()> {
    #[cfg(test)]
    if let Some(result)=PAYMENT_CHECKOUT_TEST_LAUNCH.with(|slot|slot.borrow_mut().as_mut().map(|launch|launch(checkout_url))) {
        return result;
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    { desktop_payment_webview::open(app, checkout_url) }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { let _ = (app, checkout_url); Err(anyhow!("当前平台不支持应用内扫码支付")) }
}
#[cfg(test)]
thread_local! {
    static PAYMENT_CHECKOUT_TEST_LAUNCH: RefCell<Option<Box<dyn FnMut(&str)->Result<()>>>> = RefCell::new(None);
}
#[cfg(test)]
pub(super) fn with_payment_checkout_test_launcher<R>(
    launch:impl FnMut(&str)->Result<()>+'static, body:impl FnOnce()->R,
)->R {
    struct Reset;
    impl Drop for Reset { fn drop(&mut self){PAYMENT_CHECKOUT_TEST_LAUNCH.with(|slot|{slot.borrow_mut().take();});} }
    PAYMENT_CHECKOUT_TEST_LAUNCH.with(|slot|{
        assert!(slot.borrow().is_none(),"checkout fixture launcher already installed");
        *slot.borrow_mut()=Some(Box::new(launch));
    });
    let _reset=Reset;
    body()
}

const PAYMENT_QR_PATH: &str = "/v1/payments/alipay/qr";
const ALIPAY_PAGE_HOSTS: &[&str] = &[
    "openapi.alipay.com", "unitradeprod.alipay.com", "cashier.alipay.com", "excashier.alipay.com",
    "qr.alipay.com", "render.alipay.com",
    "openapi-sandbox.dl.alipaydev.com", "cashier-sandbox.dl.alipaydev.com",
];

fn payment_navigation_allowed(candidate: &str, bridge: &reqwest::Url) -> bool {
    if candidate == "about:blank" { return true; }
    let Ok(url) = reqwest::Url::parse(candidate) else { return false; };
    if !url.username().is_empty() || url.password().is_some() { return false; }
    if origin_matches(&url, bridge) {
        return checkout_scheme_allowed(&url) && url.path() == PAYMENT_QR_PATH && url.query().is_none();
    }
    url.scheme() == "https" && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|host| ALIPAY_PAGE_HOSTS.contains(&host))
}

pub(super) fn close_payment_checkout() {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    desktop_payment_webview::close();
}

pub(super) fn payment_checkout_is_open() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    { desktop_payment_webview::is_open() }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { false }
}

pub(super) fn resize_payment_checkout(app: &AppWindow, x: f32, y: f32, width: f32, height: f32) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    desktop_payment_webview::resize(app.window().scale_factor(), x, y, width, height);
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod desktop_payment_webview {
    use super::*;
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder};
    use wry::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};

    struct Surface { id: Uuid, view: WebView }
    thread_local! { static SURFACE: RefCell<Option<Surface>> = const { RefCell::new(None) }; }

    fn is_current(id: Uuid) -> bool { SURFACE.with(|slot| slot.borrow().as_ref().is_some_and(|surface| surface.id == id)) }
    pub(super) fn is_open() -> bool { SURFACE.with(|slot| slot.borrow().is_some()) }
    pub(super) fn close() {
        SURFACE.with(|slot| {
            if let Some(surface) = slot.borrow_mut().take() {
                let _ = surface.view.set_visible(false);
            }
        });
    }

    fn show_ready_surface(id: Uuid) -> Result<()> {
        SURFACE.with(|slot| {
            let slot = slot.borrow();
            let surface = slot.as_ref().filter(|surface| surface.id == id)
                .context("支付二维码窗口已关闭")?;
            surface.view.set_visible(true).context("支付二维码显示失败")
        })
    }
    pub(super) fn resize(scale: f32, x: f32, y: f32, width: f32, height: f32) {
        if ![scale,x,y,width,height].iter().all(|v| v.is_finite()) || scale <= 0.0 || width <= 0.0 || height <= 0.0 { return; }
        SURFACE.with(|slot| { if let Some(surface) = slot.borrow().as_ref() {
            let _ = surface.view.set_bounds(Rect {
                position: PhysicalPosition::new((x * scale).round() as i32, (y * scale).round() as i32).into(),
                size: PhysicalSize::new((width * scale).round() as u32, (height * scale).round() as u32).into(),
            });
        }});
    }
    pub(super) fn open(app: &AppWindow, checkout: &str) -> Result<()> {
        close();
        app.global::<AppState>().set_payment_qr_loaded(false);
        let id = Uuid::new_v4();
        let bridge = reqwest::Url::parse(checkout)?;
        let ipc_bridge = bridge.clone();
        let weak = app.as_weak();
        let size = app.window().size().to_logical(app.window().scale_factor());
        let handle = app.window().window_handle();
        let view = WebViewBuilder::new()
            // Keep Slint's white loading placeholder visible through every
            // redirect. Only reveal the native surface after QR content is ready.
            .with_visible(false)
            .with_background_color((255, 255, 255, 255))
            .with_url(checkout)
            .with_bounds(Rect {
                position: LogicalPosition::new((size.width - 240.0) / 2.0, (size.height - 660.0) / 2.0 + 222.0).into(),
                size: LogicalSize::new(240.0, 240.0).into(),
            })
            .with_devtools(false).with_clipboard(false).with_incognito(true)
            .with_initialization_script(include_str!("payment_qr_ready.js"))
            .with_navigation_handler(move |candidate| payment_navigation_allowed(&candidate, &bridge))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_ipc_handler(move |request| {
                let source = reqwest::Url::parse(&request.uri().to_string()).ok();
                let bridge_error = request.body() == "checkout-unavailable" && source.as_ref().is_some_and(|url|
                    origin_matches(url, &ipc_bridge) && url.path() == PAYMENT_QR_PATH);
                let qr_ready = request.body() == "qr-content-ready" && source.as_ref().is_some_and(|url|
                    url.scheme() == "https" && url.port_or_known_default() == Some(443)
                        && url.host_str().is_some_and(|host| ALIPAY_PAGE_HOSTS.contains(&host)));
                if !bridge_error && !qr_ready { return; }
                let weak = weak.clone();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if !is_current(id) { return; }
                    if let Some(app) = weak.upgrade() {
                        let state = app.global::<AppState>();
                        if qr_ready && show_ready_surface(id).is_ok() {
                            state.set_payment_qr_loaded(true);
                        }
                        else {
                            close();state.set_payment_dialog_mode("error".into());
                            state.set_payment_status_message("二维码加载失败，请刷新后重试".into());
                        }
                    }
                });
            })
            .build_as_child(&handle).context("应用内支付二维码初始化失败")?;
        SURFACE.with(|slot| *slot.borrow_mut() = Some(Surface { id, view }));
        let weak = app.as_weak();
        slint::Timer::single_shot(Duration::from_secs(25), move || {
            if !is_current(id) { return; }
            if let Some(app) = weak.upgrade() {
                let state = app.global::<AppState>();
                if !state.get_payment_qr_loaded() {
                    close();
                    state.set_payment_dialog_mode("error".into());
                    state.set_payment_status_message("二维码加载超时，请检查网络后刷新".into());
                }
            } else { close(); }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payment_navigation_allows_the_real_alipay_qr_redirect_chain() {
        let bridge = reqwest::Url::parse("https://artforge-api.honeykid.cn/v1/payments/alipay/qr").unwrap();
        for candidate in [
            "https://openapi.alipay.com/gateway.do?fixture=1",
            "https://unitradeprod.alipay.com/appAssign.htm?fixture=1",
            "https://excashier.alipay.com/standard/auth.htm?fixture=1",
        ] {
            assert!(payment_navigation_allowed(candidate, &bridge), "{candidate}");
        }
        for candidate in [
            "https://unitradeprod.alipay.com.evil.example/appAssign.htm",
            "http://unitradeprod.alipay.com/appAssign.htm",
            "https://unitradeprod.alipay.com:8443/appAssign.htm",
            "https://user@unitradeprod.alipay.com/appAssign.htm",
        ] {
            assert!(!payment_navigation_allowed(candidate, &bridge), "{candidate}");
        }
    }

    #[test]
    fn payment_navigation_rejects_untrusted_ports_credentials_and_external_schemes() {
        let bridge = reqwest::Url::parse("https://artforge-api.honeykid.cn/v1/payments/alipay/qr").unwrap();
        for candidate in [bridge.as_str(), "https://openapi.alipay.com/gateway.do?fixture=1", "https://cashier.alipay.com/qr"] {
            assert!(payment_navigation_allowed(candidate, &bridge), "{candidate}");
        }
        for candidate in ["https://openapi.alipay.com.evil.example/", "http://cashier.alipay.com/qr",
            "https://cashier.alipay.com:8443/qr", "https://user@cashier.alipay.com/qr",
            "alipays://startapp", "file:///tmp/payment.html", "javascript:alert(1)",
            "https://artforge-api.honeykid.cn/v1/other", "https://artforge-api.honeykid.cn/v1/payments/alipay/qr?token=leaked"] {
            assert!(!payment_navigation_allowed(candidate, &bridge), "{candidate}");
        }
    }

    #[test]
    fn core_required_upgrade_denies_actual_payment_browser_front_door() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let latch = UpgradeLatch::default();
        latch.trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        let writer=client_state::tests::Fixture::new(false,false);
        let lease=writer.lease("11111111-1111-4111-8111-111111111111",1,1);
        writer.activate(lease.clone()).unwrap();
        let activity=UserActivityGate::default();activity.activate(lease.clone()).unwrap();
        let persistence=PrivatePersistence::for_test((*writer).clone(),lease,activity,latch);
        assert!(open_payment_checkout(&app,checkout_url(), &api_base(), &persistence,"11111111-1111-4111-8111-111111111111").is_err());
    }

    fn api_base() -> reqwest::Url {
        reqwest::Url::parse("https://artforge-api.honeykid.cn/").unwrap()
    }

    fn checkout_url() -> &'static str {
        "https://artforge-api.honeykid.cn/v1/payments/alipay/checkout#order_id=11111111-1111-4111-8111-111111111111&token=signed-token"
    }

    #[test]
    fn checkout_url_requires_the_exact_api_hosted_redirect() {
        assert!(validated_checkout_url(checkout_url(), &api_base()).is_ok());
        for url in [
            "https://artforge-api.honeykid.cn/v1/payments/alipay/checkout",
            "https://artforge-api.honeykid.cn/v1/payments/alipay/checkout#order_id=one",
            "https://artforge-api.honeykid.cn/v1/payments/alipay/checkout?token=leaked#order_id=one&token=two",
            "https://artforge-api.honeykid.cn/v1/payments/alipay/other#order_id=one&token=two",
            "https://openapi.alipay.com/gateway.do?sign=redacted",
            "http://openapi.alipay.com/gateway.do",
            "https://evil.example/gateway.do",
            "not-a-url",
        ] {
            assert!(validated_checkout_url(url, &api_base()).is_err());
        }
    }

    #[test]
    fn core_checkout_exact_saved_order_and_unique_fragment_precede_final_os_launch() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let writer=client_state::tests::Fixture::new(false,false);
        let lease=writer.lease("11111111-1111-4111-8111-111111111111",1,1);
        writer.activate(lease.clone()).unwrap();
        let activity=UserActivityGate::default();activity.activate(lease.clone()).unwrap();
        let persistence=PrivatePersistence::for_test((*writer).clone(),lease,activity,UpgradeLatch::default());
        let launches=Rc::new(std::cell::Cell::new(0));let observed=launches.clone();
        let expected="11111111-1111-4111-8111-111111111111";
        with_payment_checkout_test_launcher(move |_|{observed.set(observed.get()+1);Ok(())},||{
            for fragment in [
                "order_id=22222222-2222-4222-8222-222222222222&token=fixture",
                "order_id=11111111-1111-4111-8111-111111111111&order_id=11111111-1111-4111-8111-111111111111&token=fixture",
                "order_id=11111111-1111-4111-8111-111111111111&token=fixture&token=fixture",
                "order_id=&token=fixture",
            ] {
                let url=format!("https://artforge-api.honeykid.cn/v1/payments/alipay/checkout#{fragment}");
                assert!(open_payment_checkout(&app,&url,&api_base(),&persistence,expected).is_err());
            }
            assert_eq!(launches.get(),0);
            open_payment_checkout(&app,checkout_url(),&api_base(),&persistence,expected).unwrap();
        });
        assert_eq!(launches.get(),1);
    }
    #[test]
    fn core_checkout_retired_private_persistence_cannot_reach_final_os_launch() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let writer=client_state::tests::Fixture::new(false,false);
        let lease=writer.lease("11111111-1111-4111-8111-111111111111",1,1);
        writer.activate(lease.clone()).unwrap();
        let activity=UserActivityGate::default();activity.activate(lease.clone()).unwrap();
        let persistence=PrivatePersistence::for_test((*writer).clone(),lease.clone(),activity.clone(),UpgradeLatch::default());
        activity.begin_quiesce(&lease).unwrap().retire();
        with_payment_checkout_test_launcher(|_|panic!("retired payment must not launch an OS effect"),||{
            assert!(open_payment_checkout(&app,checkout_url(),&api_base(),&persistence,"11111111-1111-4111-8111-111111111111").is_err());
        });
    }
}
