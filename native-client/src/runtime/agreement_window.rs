use super::*;

const AGREEMENT_CONTENT_HOSTS: &[&str] = &["cdn.honeykid.cn"];

#[derive(Debug)]
struct AgreementWebViewConfig {
    content_url: String,
    content_origin: String,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn validated_agreement_url(content_url: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(content_url).context("协议地址无效")?;
    if url.scheme() != "https" {
        return Err(anyhow!("协议地址必须使用 HTTPS"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("协议地址缺少主机名"))?
        .to_ascii_lowercase();
    if !AGREEMENT_CONTENT_HOSTS.contains(&host.as_str()) {
        return Err(anyhow!("协议地址不是受信任的内容地址"));
    }
    Ok(url)
}

fn agreement_webview_config(
    content_url: &str,
    window_width: u32,
    window_height: u32,
    scale_factor: f32,
) -> Result<AgreementWebViewConfig> {
    let url = validated_agreement_url(content_url)?;
    let content_origin = url.origin().ascii_serialization();
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let target_width = (800.0 * scale_factor).round() as u32;
    let target_height = (560.0 * scale_factor).round() as u32;
    let side_margin = (60.0 * scale_factor).round() as u32;
    let vertical_margin = (120.0 * scale_factor).round() as u32;
    let width = target_width.min(window_width.saturating_sub(side_margin));
    let height = target_height.min(window_height.saturating_sub(vertical_margin));
    let x = window_width.saturating_sub(width) / 2;
    let panel_height = (680.0 * scale_factor).round() as u32;
    let panel_top = window_height.saturating_sub(panel_height) / 2;
    let y = (panel_top + (70.0 * scale_factor).round() as u32)
        .min(window_height.saturating_sub(height));
    Ok(AgreementWebViewConfig {
        content_url: url.to_string(),
        content_origin,
        x,
        y,
        width,
        height,
    })
}

fn agreement_navigation_allowed(candidate: &str, content_origin: &str) -> bool {
    if candidate == "about:blank" {
        return true;
    }
    let Ok(url) = reqwest::Url::parse(candidate) else {
        return false;
    };
    url.scheme() == "https" && url.origin().ascii_serialization() == content_origin
}

pub(super) fn open_agreement_window(app: &AppWindow, content_url: &str) -> Result<()> {
    let window_size = app.window().size();
    let config = agreement_webview_config(
        content_url,
        window_size.width,
        window_size.height,
        app.window().scale_factor(),
    )?;
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        desktop_agreement_webview::open(app.window(), config)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (app, config);
        Err(anyhow!("当前平台不支持应用内协议窗口"))
    }
}

pub(super) fn close_agreement_window() {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    desktop_agreement_webview::close();
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod desktop_agreement_webview {
    use super::*;
    use std::cell::RefCell;
    use wry::dpi::{PhysicalPosition, PhysicalSize};
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder};

    thread_local! {
        static AGREEMENT_WEBVIEW: RefCell<Option<WebView>> = const { RefCell::new(None) };
    }

    pub(super) fn open(window: &slint::Window, config: AgreementWebViewConfig) -> Result<()> {
        let content_origin = config.content_origin;
        let window_handle = window.window_handle();
        let webview = WebViewBuilder::new()
            .with_url(config.content_url)
            .with_bounds(Rect {
                position: PhysicalPosition::new(config.x, config.y).into(),
                size: PhysicalSize::new(config.width, config.height).into(),
            })
            .with_devtools(false)
            .with_clipboard(true)
            .with_navigation_handler(move |candidate| {
                agreement_navigation_allowed(&candidate, &content_origin)
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .build_as_child(&window_handle)
            .context("应用内协议页面初始化失败")?;
        AGREEMENT_WEBVIEW.with(|slot| {
            *slot.borrow_mut() = Some(webview);
        });
        Ok(())
    }

    pub(super) fn close() {
        AGREEMENT_WEBVIEW.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_url_requires_the_trusted_https_content_host() {
        assert!(validated_agreement_url(
            "https://cdn.honeykid.cn/public/art_forge/user-terms-v1.0.html"
        )
        .is_ok());
        for url in [
            "http://cdn.honeykid.cn/public/art_forge/user-terms-v1.0.html",
            "https://cdn.honeykid.cn.attacker.example/terms.html",
            "https://attacker.example/terms.html",
            "not-a-url",
        ] {
            assert!(validated_agreement_url(url).is_err());
        }
    }

    #[test]
    fn agreement_navigation_stays_inside_the_content_origin() {
        let origin = "https://cdn.honeykid.cn";
        assert!(agreement_navigation_allowed("about:blank", origin));
        assert!(agreement_navigation_allowed(
            "https://cdn.honeykid.cn/public/art_forge/privacy-policy-v1.0.html",
            origin
        ));
        for url in [
            "http://cdn.honeykid.cn/public/art_forge/privacy-policy-v1.0.html",
            "https://attacker.example/terms.html",
            "mailto:support@honeykid.cn",
            "not-a-url",
        ] {
            assert!(!agreement_navigation_allowed(url, origin));
        }
    }

    #[test]
    fn agreement_surface_matches_the_slint_dialog_content_area() {
        let config = agreement_webview_config(
            "https://cdn.honeykid.cn/public/art_forge/user-terms-v1.0.html",
            1440,
            900,
            1.0,
        )
        .expect("agreement config");
        assert_eq!((config.x, config.y, config.width, config.height), (320, 180, 800, 560));
    }
}
