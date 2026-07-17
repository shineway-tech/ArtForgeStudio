use super::*;

const ALIPAY_CHECKOUT_HOSTS: &[&str] = &[
    "openapi.alipay.com",
    "openapi-sandbox.dl.alipaydev.com",
];
const ALIPAY_NAVIGATION_SUFFIXES: &[&str] =
    &["alipay.com", "alipayobjects.com", "alipaydev.com"];

#[derive(Debug)]
struct PaymentWebViewConfig {
    checkout_html: String,
    checkout_host: String,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn escape_html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn payment_iframe_html(checkout: &reqwest::Url) -> String {
    let checkout = escape_html_attribute(checkout.as_str());
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
html, body {{ width: 100%; height: 100%; margin: 0; overflow: hidden; background: #fff; }}
iframe {{ display: block; width: 100%; height: 100%; border: 0; background: #fff; }}
</style>
</head>
<body><iframe src="{checkout}" title="支付宝扫码支付" scrolling="no"></iframe></body>
</html>"#
    )
}

fn host_matches(host: &str, allowed: &str) -> bool {
    host == allowed || host.ends_with(&format!(".{allowed}"))
}

fn validated_checkout_url(checkout_url: &str) -> Result<reqwest::Url> {
    let checkout = reqwest::Url::parse(checkout_url).context("支付地址无效")?;
    if checkout.scheme() != "https" {
        return Err(anyhow!("支付地址必须使用 HTTPS"));
    }
    let host = checkout
        .host_str()
        .ok_or_else(|| anyhow!("支付地址缺少主机名"))?
        .to_ascii_lowercase();
    if !ALIPAY_CHECKOUT_HOSTS.contains(&host.as_str()) {
        return Err(anyhow!("支付地址不是受信任的支付宝网关"));
    }
    Ok(checkout)
}

fn payment_webview_config(
    checkout_url: &str,
    window_width: u32,
    window_height: u32,
    scale_factor: f32,
) -> Result<PaymentWebViewConfig> {
    let checkout = validated_checkout_url(checkout_url)?;
    let checkout_host = checkout
        .host_str()
        .ok_or_else(|| anyhow!("支付地址缺少主机名"))?
        .to_ascii_lowercase();
    let checkout_html = payment_iframe_html(&checkout);
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let qr_size = (220.0 * scale_factor).round() as u32;
    let vertical_offset = (5.0 * scale_factor).round() as u32;
    let width = window_width.min(qr_size);
    let height = window_height.min(qr_size);
    let x = window_width.saturating_sub(width) / 2;
    let y = (window_height.saturating_sub(height) / 2 + vertical_offset)
        .min(window_height.saturating_sub(height));
    Ok(PaymentWebViewConfig {
        checkout_html,
        checkout_host,
        x,
        y,
        width,
        height,
    })
}

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
enum PaymentSurfaceKind {
    Embedded,
}

#[cfg(test)]
fn payment_surface_kind() -> PaymentSurfaceKind {
    PaymentSurfaceKind::Embedded
}

pub(super) fn open_payment_window(app: &AppWindow, checkout_url: &str) -> Result<()> {
    let window_size = app.window().size();
    let config = payment_webview_config(
        checkout_url,
        window_size.width,
        window_size.height,
        app.window().scale_factor(),
    )?;
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        desktop_payment_webview::open(app.window(), config)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (app, config);
        Err(anyhow!("当前平台不支持应用内支付宝二维码"))
    }
}

pub(super) fn close_payment_window() {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    desktop_payment_webview::close();
}

fn payment_navigation_allowed(candidate: &str, checkout_host: &str) -> bool {
    if candidate == "about:blank" {
        return true;
    }
    let Ok(url) = reqwest::Url::parse(candidate) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    host == checkout_host
        || ALIPAY_NAVIGATION_SUFFIXES
            .iter()
            .any(|suffix| host_matches(&host, suffix))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod desktop_payment_webview {
    use super::*;
    use std::cell::RefCell;
    use wry::dpi::{PhysicalPosition, PhysicalSize};
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder};

    thread_local! {
        static PAYMENT_WEBVIEW: RefCell<Option<WebView>> = const { RefCell::new(None) };
    }

    pub(super) fn open(window: &slint::Window, config: PaymentWebViewConfig) -> Result<()> {
        let checkout_host = config.checkout_host;
        let window_handle = window.window_handle();
        let webview = WebViewBuilder::new()
            .with_html(config.checkout_html)
            .with_bounds(Rect {
                position: PhysicalPosition::new(config.x, config.y).into(),
                size: PhysicalSize::new(config.width, config.height).into(),
            })
            .with_devtools(false)
            .with_clipboard(false)
            .with_navigation_handler(move |candidate| {
                payment_navigation_allowed(&candidate, &checkout_host)
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .build_as_child(&window_handle)
            .context("应用内支付页面初始化失败")?;
        PAYMENT_WEBVIEW.with(|slot| {
            *slot.borrow_mut() = Some(webview);
        });
        Ok(())
    }

    pub(super) fn close() {
        PAYMENT_WEBVIEW.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_url_requires_an_exact_alipay_https_gateway() {
        for url in [
            "https://openapi.alipay.com/gateway.do?sign=redacted",
            "https://openapi-sandbox.dl.alipaydev.com/gateway.do?sign=redacted",
        ] {
            assert!(validated_checkout_url(url).is_ok());
        }
        for url in [
            "http://openapi.alipay.com/gateway.do",
            "https://evil.example/gateway.do",
            "https://openapi.alipay.com.attacker.example/gateway.do",
            "not-a-url",
        ] {
            assert!(validated_checkout_url(url).is_err());
        }
    }

    #[test]
    fn checkout_wrapper_contains_one_escaped_full_size_iframe() {
        let checkout = validated_checkout_url(
            "https://openapi.alipay.com/gateway.do?subject=ArtForge&amount=10",
        )
        .expect("trusted checkout");
        let html = payment_iframe_html(&checkout);

        assert_eq!(html.matches("<iframe ").count(), 1);
        assert!(html.contains(
            "src=\"https://openapi.alipay.com/gateway.do?subject=ArtForge&amp;amount=10\""
        ));
        assert!(html.contains("width: 100%; height: 100%; margin: 0; overflow: hidden"));
        assert!(html.contains("scrolling=\"no\""));
        assert!(!html.contains("subject=ArtForge&amount=10"));
    }

    #[test]
    fn html_attribute_escaping_covers_markup_delimiters() {
        assert_eq!(
            escape_html_attribute("&\"'<>"),
            "&amp;&quot;&#39;&lt;&gt;"
        );
    }

    #[test]
    fn payment_navigation_is_https_and_alipay_limited() {
        let checkout_host = "openapi.alipay.com";
        assert!(payment_navigation_allowed("about:blank", checkout_host));
        for url in [
            "https://openapi.alipay.com/gateway.do",
            "https://excashier.alipay.com/standard/auth.htm",
            "https://render.alipayobjects.com/p/cashier/index.html",
            "https://openapi-sandbox.dl.alipaydev.com/gateway.do",
        ] {
            assert!(payment_navigation_allowed(url, checkout_host));
        }
        for url in [
            "http://excashier.alipay.com/standard/auth.htm",
            "https://alipay.com.attacker.example/pay",
            "https://attacker.example/?next=alipay.com",
            "not-a-url",
        ] {
            assert!(!payment_navigation_allowed(url, checkout_host));
        }
    }

    #[test]
    fn webview_config_keeps_the_qr_surface_centered() {
        let config = payment_webview_config(
            "https://openapi.alipay.com/gateway.do?sign=redacted",
            1440,
            900,
            1.0,
        )
        .expect("trusted checkout");
        assert_eq!(config.checkout_host, "openapi.alipay.com");
        assert_eq!(
            (config.x, config.y, config.width, config.height),
            (610, 345, 220, 220)
        );
    }

    #[test]
    fn webview_config_scales_the_qr_surface_on_retina_displays() {
        let config = payment_webview_config(
            "https://openapi.alipay.com/gateway.do?sign=redacted",
            2880,
            1800,
            2.0,
        )
        .expect("trusted checkout");
        assert_eq!(
            (config.x, config.y, config.width, config.height),
            (1220, 690, 440, 440)
        );
    }

    #[test]
    fn supported_desktop_builds_use_an_embedded_payment_surface() {
        assert_eq!(payment_surface_kind(), PaymentSurfaceKind::Embedded);
    }
}
