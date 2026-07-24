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
            && matches!(
                checkout.host_str(),
                Some("localhost" | "127.0.0.1" | "::1")
            )
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

fn checkout_fragment_has_session(fragment: &str) -> bool {
    let Ok(fragment_url) =
        reqwest::Url::parse(&format!("https://checkout-fragment.invalid/?{fragment}"))
    else {
        return false;
    };
    let mut has_order_id = false;
    let mut has_token = false;
    for (key, value) in fragment_url.query_pairs() {
        match key.as_ref() {
            "order_id" => has_order_id = !value.is_empty(),
            "token" => has_token = !value.is_empty(),
            _ => {}
        }
    }
    has_order_id && has_token
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
    checkout_url: &str,
    trusted_api_base: &reqwest::Url,
) -> Result<()> {
    let checkout = validated_checkout_url(checkout_url, trusted_api_base)?;
    open_external_checkout(checkout.as_str())
}

#[cfg(target_os = "macos")]
fn open_external_checkout(checkout_url: &str) -> Result<()> {
    Command::new("open")
        .arg(checkout_url)
        .spawn()
        .context("无法打开系统浏览器")?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_external_checkout(checkout_url: &str) -> Result<()> {
    Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(checkout_url)
        .spawn()
        .context("无法打开系统浏览器")?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn open_external_checkout(checkout_url: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(checkout_url)
        .spawn()
        .context("无法打开系统浏览器")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
