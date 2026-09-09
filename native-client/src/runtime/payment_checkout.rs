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
    checkout_url:&str,trusted_api_base:&reqwest::Url,persistence:&PrivatePersistence,expected_order_id:&str,
)->Result<()> {
    let checkout=validated_checkout_url(checkout_url,trusted_api_base)?;
    anyhow::ensure!(checkout_matches_order(&checkout,expected_order_id),"支付地址不属于原始订单");
    let(activity,effect)=persistence.begin_effect()?;
    if activity.is_quiescing() || persistence.upgrade_latch().is_tripped() {
        return Err(anyhow!("当前用户操作已关闭"));
    }
    // Counted effect surrounds the final OS call, but no global short latch is
    // held during process creation. Trip waits for this admitted effect to drain.
    let result=open_external_checkout(checkout.as_str());
    drop(effect);drop(activity);
    result
}

fn open_external_checkout(checkout_url:&str)->Result<()> {
    #[cfg(test)]
    if let Some(result)=PAYMENT_CHECKOUT_TEST_LAUNCH.with(|slot|slot.borrow_mut().as_mut().map(|launch|launch(checkout_url))) {
        return result;
    }
    platform_open_external_checkout(checkout_url)
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

#[cfg(target_os = "macos")]
fn platform_open_external_checkout(checkout_url: &str) -> Result<()> {
    Command::new("open")
        .arg(checkout_url)
        .spawn()
        .context("无法打开系统浏览器")?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn platform_open_external_checkout(checkout_url: &str) -> Result<()> {
    Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(checkout_url)
        .spawn()
        .context("无法打开系统浏览器")?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_open_external_checkout(checkout_url: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(checkout_url)
        .spawn()
        .context("无法打开系统浏览器")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_required_upgrade_denies_actual_payment_browser_front_door() {
        let latch = UpgradeLatch::default();
        latch.trip(RequiredUpgrade { minimum_version: Some("99.0.0".into()) });
        let writer=client_state::tests::Fixture::new(false,false);
        let lease=writer.lease("11111111-1111-4111-8111-111111111111",1,1);
        writer.activate(lease.clone()).unwrap();
        let activity=UserActivityGate::default();activity.activate(lease.clone()).unwrap();
        let persistence=PrivatePersistence::for_test((*writer).clone(),lease,activity,latch);
        assert!(open_payment_checkout(checkout_url(), &api_base(), &persistence,"11111111-1111-4111-8111-111111111111").is_err());
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
                assert!(open_payment_checkout(&url,&api_base(),&persistence,expected).is_err());
            }
            assert_eq!(launches.get(),0);
            open_payment_checkout(checkout_url(),&api_base(),&persistence,expected).unwrap();
        });
        assert_eq!(launches.get(),1);
    }
    #[test]
    fn core_checkout_retired_private_persistence_cannot_reach_final_os_launch() {
        let writer=client_state::tests::Fixture::new(false,false);
        let lease=writer.lease("11111111-1111-4111-8111-111111111111",1,1);
        writer.activate(lease.clone()).unwrap();
        let activity=UserActivityGate::default();activity.activate(lease.clone()).unwrap();
        let persistence=PrivatePersistence::for_test((*writer).clone(),lease.clone(),activity.clone(),UpgradeLatch::default());
        activity.begin_quiesce(&lease).unwrap().retire();
        with_payment_checkout_test_launcher(|_|panic!("retired payment must not launch an OS effect"),||{
            assert!(open_payment_checkout(checkout_url(),&api_base(),&persistence,"11111111-1111-4111-8111-111111111111").is_err());
        });
    }
}
