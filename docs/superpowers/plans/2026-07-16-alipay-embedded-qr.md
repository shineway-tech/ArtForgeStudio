# Alipay Embedded QR Payment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make membership purchase, renewal, upgrade, and credit recharge display an Alipay-hosted QR payment page inside the macOS and Windows client while keeping the backend authoritative for payment and entitlement completion.

**Architecture:** Keep the existing `alipay.trade.page.pay` order flow and `checkout_url` API contract, but generate the URL with Alipay embedded-QR parameters and no synchronous return URL. Validate that URL at the client boundary, materialize it in a Wry child WebView on macOS and Windows, and keep the existing three-second order synchronization, recovery, fulfillment, and notification refresh paths.

**Tech Stack:** Node.js 24, CommonJS, `alipay-sdk`, Node test runner, ESLint, Rust 2021, Slint 1.16.1, Wry 0.55.1, `reqwest::Url`, macOS WKWebView, Windows WebView2.

## Global Constraints

- Work from `/Users/fanxiao/workstation/ai/ArtForgeStudio`; the active client is `ArtForgeStudio/native-client` and the backend is `server/artforge-api`.
- Do not modify archived client crates under `ArtForgeStudio/crates/`.
- Do not switch to `alipay.trade.precreate`, generate a local QR image, scrape Alipay HTML, or add a payment database migration.
- Use `alipay.trade.page.pay`, `product_code: "FAST_INSTANT_TRADE_PAY"`, `qr_pay_mode: "4"`, and `qrcode_width: 220` for every non-zero membership and credit payment.
- Preserve `notify_url`; do not send `return_url` for the embedded QR flow.
- Preserve the API field `payment.checkout_url`, payment channel `website`, server-side notification verification, active querying, expiration, closing, fulfillment, and idempotency behavior.
- Accept initial checkout URLs only when they use HTTPS and the exact host `openapi.alipay.com` or `openapi-sandbox.dl.alipaydev.com`.
- Permit subsequent HTTPS navigation only to the initial host or the exact/suffix domains `alipay.com`, `alipayobjects.com`, and `alipaydev.com`; reject downloads and new windows.
- Never include a full signed `checkout_url` in logs, UI errors, or test failure messages.
- Use the Chinese copy `支付宝扫码支付`, `请使用支付宝扫一扫完成支付，支付结果以服务端确认为准。`, `生成支付二维码`, and `正在等待支付宝付款结果...`; remove user-facing “打开支付宝”, “打开收银台”, “支付宝收银台”, “关闭收银台”, and “系统浏览器启动失败”.
- Closing the WebView hides the QR only; it must not mark an order paid, cancel it, remove its recovery record, or stop status polling.
- Do not execute a real Alipay payment. Use unit tests and the dev Mock API only.
- Do not change Redis configuration, callback/HTTPS deployment configuration, or production secrets.
- Do not create Git commits; each task ends with a test/checkpoint instead.

---

## Execution Status (2026-07-16)

- [x] Task 1: backend embedded-QR request parameters and trusted checkout URL validation.
- [x] Tasks 2–3: exact Alipay URL policy and common macOS/Windows Wry child WebView; macOS `cargo check` and focused tests passed.
- [x] Task 4: QR copy, close behavior, safe load error, three-second polling, success refresh preservation, and pending-order reopen behavior.
- [x] Task 5: credit and membership Mock API checkout contracts and repeated-sync tests.
- [x] Task 6: backend payment guide and frontend/backend execution record updated.
- [x] Task 7 automated gates: backend 44/44 tests, ESLint, config check, Mock membership/payment integration, client 47/47 active tests, two enabled payment cross-stack tests, and `cargo check` passed.
- [x] Startup smoke: the macOS client connected to the local Mock API, refreshed its session, and loaded account, membership, credits, models, and generation tasks before clean shutdown.
- [ ] Manual visual check: click one credit pack and one membership plan on macOS to inspect the child WebView surface; do not scan or pay. Windows WebView2 release-runner and real Alipay payment remain formal-release checks.
- [x] No Git commit, real payment, database migration, Redis change, or production callback/HTTPS change was made.

---

## File Map

- Create `server/artforge-api/src/services/alipay_page_pay.js`: pure builder for embedded-QR page-pay parameters.
- Create `server/artforge-api/test/alipay_page_pay.test.js`: verifies the complete page-pay parameter contract without initializing the real SDK.
- Modify `server/artforge-api/src/services/alipay.js`: delegate request construction to the pure builder and stop passing `returnUrl`.
- Modify `server/artforge-api/bin/dev-mock-api-server`: return a syntactically trusted mock checkout URL.
- Modify `server/artforge-api/docs/MEMBERSHIP_PAYMENT_API.md`: document embedded QR behavior and server-authoritative completion.
- Modify `ArtForgeStudio/native-client/Cargo.toml`: enable Wry for macOS as well as Windows.
- Modify `ArtForgeStudio/native-client/src/runtime/payment_window.rs`: centralize URL policy, WebView layout, and the common macOS/Windows child WebView.
- Modify `ArtForgeStudio/native-client/src/runtime/callbacks/payment.rs`: expose QR-specific messages, preserve polling on WebView close/failure, and reopen valid recovered orders.
- Modify `ArtForgeStudio/native-client/ui/dialogs/ali-pay-qr-dialog.slint`: replace checkout/open wording with embedded-QR wording.
- Modify `ArtForgeStudio/native-client/ui/dialogs/membership-dialog.slint`: close the child WebView when the membership dialog is hidden and show QR semantics while payment is active.
- Modify `ArtForgeStudio/native-client/ui/components/top-bar.slint`: rename the active-payment action to “关闭支付码”.
- Modify `ArtForgeStudio/native-client/src/runtime/tests.rs`: protect the payment copy contract.
- Modify `ArtForgeStudio/native-client/src/runtime/api/cross_stack_tests.rs`: verify membership and credit responses both expose trusted checkout URLs.
- Modify `ArtForgeStudio/docs/FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md`: update the implemented payment behavior and remaining real-environment checks.

---

### Task 1: Build and use the backend embedded-QR request contract

**Files:**
- Create: `server/artforge-api/src/services/alipay_page_pay.js`
- Create: `server/artforge-api/test/alipay_page_pay.test.js`
- Modify: `server/artforge-api/src/services/alipay.js`

**Interfaces:**
- Consumes: `centsToYuan(value)` from `server/artforge-api/src/utils/money.js` and current Alipay config values.
- Produces: `buildPagePayParams({ orderNo, amountCents, subject, sellerId, timeoutMinutes, notifyUrl }): { bizContent: object, notifyUrl?: string }`.
- Produces: `validatePagePayCheckoutUrl(checkoutUrl: string): string`, accepting only HTTPS URLs on the two approved Alipay gateway hosts.
- Preserves: `pagePay({ orderNo, amountCents, subject }): { checkoutUrl: string, outTradeNo: string }`.

- [ ] **Step 1: Write the failing pure-contract test**

Create `server/artforge-api/test/alipay_page_pay.test.js` with this content:

```js
const assert = require('node:assert/strict');
const test = require('node:test');
const {
  buildPagePayParams,
  validatePagePayCheckoutUrl,
} = require('../src/services/alipay_page_pay');

test('page pay parameters request Alipay embedded QR mode without a return URL', () => {
  const params = buildPagePayParams({
    orderNo: 'AF202607160001',
    amountCents: '12345',
    subject: 'ArtForge Studio 积分充值',
    sellerId: '2088000000000000',
    timeoutMinutes: 5,
    notifyUrl: 'https://artforge-api.example/v1/payments/alipay/notify',
  });

  assert.deepEqual(params, {
    bizContent: {
      out_trade_no: 'AF202607160001',
      product_code: 'FAST_INSTANT_TRADE_PAY',
      total_amount: '123.45',
      subject: 'ArtForge Studio 积分充值',
      seller_id: '2088000000000000',
      timeout_express: '5m',
      qr_pay_mode: '4',
      qrcode_width: 220,
    },
    notifyUrl: 'https://artforge-api.example/v1/payments/alipay/notify',
  });
  assert.equal(Object.hasOwn(params, 'returnUrl'), false);
});

test('page pay parameters omit optional notify URL when dev callback is unset', () => {
  const params = buildPagePayParams({
    orderNo: 'AF202607160002',
    amountCents: '1',
    subject: 'ArtForge Studio 月度会员',
    sellerId: '2088000000000000',
    timeoutMinutes: 5,
    notifyUrl: '',
  });

  assert.equal(params.bizContent.total_amount, '0.01');
  assert.equal(Object.hasOwn(params, 'notifyUrl'), false);
  assert.equal(Object.hasOwn(params, 'returnUrl'), false);
});

test('page pay checkout URL must be an approved Alipay HTTPS gateway', () => {
  for (const checkoutUrl of [
    'https://openapi.alipay.com/gateway.do?sign=redacted',
    'https://openapi-sandbox.dl.alipaydev.com/gateway.do?sign=redacted',
  ]) {
    assert.equal(validatePagePayCheckoutUrl(checkoutUrl), checkoutUrl);
  }
  for (const checkoutUrl of [
    'http://openapi.alipay.com/gateway.do',
    'https://openapi.alipay.com.attacker.example/gateway.do',
    'https://evil.example/gateway.do',
    '',
  ]) {
    assert.throws(() => validatePagePayCheckoutUrl(checkoutUrl), /trusted Alipay HTTPS gateway/);
  }
});
```

- [ ] **Step 2: Run the new test and verify the missing module failure**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
node --test test/alipay_page_pay.test.js
```

Expected: FAIL with `Cannot find module '../src/services/alipay_page_pay'`.

- [ ] **Step 3: Add the minimal pure parameter builder**

Create `server/artforge-api/src/services/alipay_page_pay.js` with this content:

```js
const { centsToYuan } = require('../utils/money');

function buildPagePayParams({
  orderNo,
  amountCents,
  subject,
  sellerId,
  timeoutMinutes,
  notifyUrl,
}) {
  const params = {
    bizContent: {
      out_trade_no: orderNo,
      product_code: 'FAST_INSTANT_TRADE_PAY',
      total_amount: centsToYuan(amountCents),
      subject,
      seller_id: sellerId,
      timeout_express: `${timeoutMinutes}m`,
      qr_pay_mode: '4',
      qrcode_width: 220,
    },
  };
  if (notifyUrl) params.notifyUrl = notifyUrl;
  return params;
}

const ALIPAY_CHECKOUT_HOSTS = new Set([
  'openapi.alipay.com',
  'openapi-sandbox.dl.alipaydev.com',
]);

function validatePagePayCheckoutUrl(checkoutUrl) {
  let parsed;
  try {
    parsed = new URL(checkoutUrl);
  } catch {
    throw new Error('Alipay did not generate a trusted Alipay HTTPS gateway URL');
  }
  if (parsed.protocol !== 'https:' || !ALIPAY_CHECKOUT_HOSTS.has(parsed.hostname.toLowerCase())) {
    throw new Error('Alipay did not generate a trusted Alipay HTTPS gateway URL');
  }
  return checkoutUrl;
}

module.exports = { buildPagePayParams, validatePagePayCheckoutUrl };
```

- [ ] **Step 4: Make the SDK adapter consume the builder**

In `server/artforge-api/src/services/alipay.js`, replace the `centsToYuan` import with:

```js
const {
  buildPagePayParams,
  validatePagePayCheckoutUrl,
} = require('./alipay_page_pay');
```

Replace the request-construction body in `pagePay` with:

```js
    const params = buildPagePayParams({
      orderNo,
      amountCents,
      subject,
      sellerId: config.alipay.merchant_id,
      timeoutMinutes: config.alipay.payment_expire_minutes,
      notifyUrl: config.alipay.notify_url,
    });
    const checkoutUrl = sdk.pageExecute('alipay.trade.page.pay', 'GET', params);
    return {
      checkoutUrl: validatePagePayCheckoutUrl(checkoutUrl),
      outTradeNo: orderNo,
    };
```

This replacement removes the former `params.returnUrl = config.alipay.return_url` branch while leaving SDK initialization and error wrapping unchanged.

- [ ] **Step 5: Run backend contract and order-view tests**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
node --test test/alipay_page_pay.test.js test/state_views.test.js
npx eslint src/services/alipay.js src/services/alipay_page_pay.js test/alipay_page_pay.test.js
```

Expected: both test files PASS and ESLint exits with code 0.

- [ ] **Step 6: Review the Task 1 diff without committing**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio
git diff --check -- server/artforge-api/src/services/alipay.js server/artforge-api/src/services/alipay_page_pay.js server/artforge-api/test/alipay_page_pay.test.js
```

Expected: no output and exit code 0. Do not run `git commit`.

---

### Task 2: Enforce the client checkout URL security boundary

**Files:**
- Modify: `ArtForgeStudio/native-client/src/runtime/payment_window.rs`

**Interfaces:**
- Consumes: a backend `payment.checkout_url` string.
- Produces: `validated_checkout_url(checkout_url: &str) -> Result<reqwest::Url>`.
- Produces: `payment_navigation_allowed(candidate: &str, checkout_host: &str) -> bool`.
- Produces: `PaymentWebViewConfig { checkout: reqwest::Url, checkout_host: String, x: u32, y: u32, width: u32, height: u32 }` through `payment_webview_config(checkout_url, window_width, window_height) -> Result<PaymentWebViewConfig>`.

- [ ] **Step 1: Replace the current navigation test with initial-URL and redirect-policy tests**

Inside the existing `#[cfg(test)] mod tests` in `payment_window.rs`, use these tests:

```rust
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
fn payment_navigation_is_https_and_alipay_limited() {
    let checkout_host = "openapi.alipay.com";
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
fn webview_config_keeps_the_qr_surface_centered_and_redacts_nothing_into_errors() {
    let config = payment_webview_config(
        "https://openapi.alipay.com/gateway.do?sign=redacted",
        1440,
        900,
    )
    .expect("trusted checkout");
    assert_eq!(config.checkout_host, "openapi.alipay.com");
    assert_eq!((config.x, config.y, config.width, config.height), (540, 284, 360, 380));
}
```

- [ ] **Step 2: Run the focused test and verify it fails on missing validation/config functions**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
cargo test -p artforge-studio-native payment_window -- --nocapture
```

Expected: compilation FAIL because `validated_checkout_url` and `payment_webview_config` do not exist.

- [ ] **Step 3: Add exact-host validation and a pure WebView configuration constructor**

Replace the constants and validation helpers above `open_payment_window` with:

```rust
const ALIPAY_CHECKOUT_HOSTS: &[&str] = &[
    "openapi.alipay.com",
    "openapi-sandbox.dl.alipaydev.com",
];
const ALIPAY_NAVIGATION_SUFFIXES: &[&str] =
    &["alipay.com", "alipayobjects.com", "alipaydev.com"];

#[derive(Debug)]
struct PaymentWebViewConfig {
    checkout: reqwest::Url,
    checkout_host: String,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
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

fn payment_navigation_allowed(candidate: &str, checkout_host: &str) -> bool {
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

fn payment_webview_config(
    checkout_url: &str,
    window_width: u32,
    window_height: u32,
) -> Result<PaymentWebViewConfig> {
    let checkout = validated_checkout_url(checkout_url)?;
    let checkout_host = checkout
        .host_str()
        .ok_or_else(|| anyhow!("支付地址缺少主机名"))?
        .to_ascii_lowercase();
    let width = window_width.saturating_sub(80).min(360);
    let height = window_height.saturating_sub(120).min(380);
    let x = window_width.saturating_sub(width) / 2;
    let y = window_height.saturating_sub(height) / 2 + 24;
    Ok(PaymentWebViewConfig { checkout, checkout_host, x, y, width, height })
}
```

Delete the older `ALIPAY_HOST_SUFFIXES` and duplicate `payment_navigation_allowed` implementation.

- [ ] **Step 4: Run the focused policy tests**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
cargo test -p artforge-studio-native payment_window -- --nocapture
```

Expected: all three `payment_window` tests PASS.

- [ ] **Step 5: Review the Task 2 diff without committing**

Run:

```bash
git diff --check -- native-client/src/runtime/payment_window.rs
```

Expected: no output and exit code 0. Do not run `git commit`.

---

### Task 3: Render the QR page in a common macOS/Windows Wry child WebView

**Files:**
- Modify: `ArtForgeStudio/native-client/Cargo.toml`
- Modify: `ArtForgeStudio/native-client/src/runtime/payment_window.rs`

**Interfaces:**
- Consumes: `payment_webview_config(checkout_url, window_width, window_height)` from Task 2.
- Preserves: `open_payment_window(app: &AppWindow, checkout_url: &str) -> Result<()>` and `close_payment_window()` for callback callers.
- Produces: one thread-local `desktop_payment_webview` child view on macOS/Windows; a second open replaces the prior child view.

- [ ] **Step 1: Add a compile-time behavior test for the desktop surface**

Add this test to the `payment_window.rs` test module:

```rust
#[test]
fn supported_desktop_builds_use_an_embedded_payment_surface() {
    assert_eq!(payment_surface_kind(), PaymentSurfaceKind::Embedded);
}
```

- [ ] **Step 2: Run it and verify the missing enum/function failure**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
cargo test -p artforge-studio-native supported_desktop_builds_use_an_embedded_payment_surface -- --nocapture
```

Expected: compilation FAIL because `PaymentSurfaceKind` and `payment_surface_kind` do not exist.

- [ ] **Step 3: Enable Wry on both supported desktop targets**

In `native-client/Cargo.toml`, add Wry to the macOS target block and remove it from the Windows-only block so the relevant sections read:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6.4"
objc2-app-kit = { version = "0.3.2", default-features = false, features = ["std", "NSApplication", "NSImage", "NSResponder"] }
objc2-foundation = { version = "0.3.2", default-features = false, features = ["std", "NSData"] }
wry = "0.55.1"

[target.'cfg(windows)'.dependencies]
wry = "0.55.1"
```

Keep all existing `windows`, `windows-core`, and `windows-sys` entries below the Windows Wry line unchanged.

- [ ] **Step 4: Replace the platform split with a shared embedded implementation**

In `payment_window.rs`, add the surface marker and replace `open_payment_window`, `close_payment_window`, and `windows_payment_window` with:

```rust
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
    let config = payment_webview_config(checkout_url, window_size.width, window_size.height)?;
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
        let webview = WebViewBuilder::new()
            .with_url(config.checkout.to_string())
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
            .build_as_child(window)
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
```

Delete the macOS `open_external_url` fallback and the old `windows_payment_window` module. The generic browser helper may remain for non-payment application links elsewhere, but payment code must not call it.

- [ ] **Step 5: Run policy tests and compile the macOS Wry path**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
cargo test -p artforge-studio-native payment_window -- --nocapture
cargo check -p artforge-studio-native
```

Expected: tests PASS and `cargo check` finishes successfully, proving WKWebView/Wry compiles on the current macOS machine.

- [ ] **Step 6: Verify conditional compilation has no browser fallback**

Run:

```bash
rg -n "open_external_url|系统浏览器启动失败|windows_payment_window" native-client/src/runtime/payment_window.rs
```

Expected: no output.

- [ ] **Step 7: Review the Task 3 diff without committing**

Run:

```bash
git diff --check -- native-client/Cargo.toml native-client/Cargo.lock native-client/src/runtime/payment_window.rs
```

Expected: no output and exit code 0. Do not run `git commit`.

---

### Task 4: Align payment copy, close behavior, status messages, and recovery

**Files:**
- Modify: `ArtForgeStudio/native-client/ui/dialogs/ali-pay-qr-dialog.slint`
- Modify: `ArtForgeStudio/native-client/ui/dialogs/membership-dialog.slint`
- Modify: `ArtForgeStudio/native-client/ui/components/top-bar.slint`
- Modify: `ArtForgeStudio/native-client/src/runtime/callbacks/payment.rs`
- Modify: `ArtForgeStudio/native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: existing Slint callbacks `AppState.recharge-credits`, `AppState.purchase-membership`, and `AppState.close-payment-window`.
- Preserves: three-second `poll_payment_order`, backend-authoritative `PaymentOrderPhase`, pending-order persistence, and success refresh calls.
- Changes: recovered pending orders call `continue_payment_order(..., true)` so an unexpired trusted QR page reappears.

- [ ] **Step 1: Add a failing UI copy contract test**

Append this test to `native-client/src/runtime/tests.rs`:

```rust
#[test]
fn payment_ui_uses_embedded_alipay_qr_copy() {
    let credit = include_str!("../../ui/dialogs/ali-pay-qr-dialog.slint");
    let membership = include_str!("../../ui/dialogs/membership-dialog.slint");
    let top_bar = include_str!("../../ui/components/top-bar.slint");

    assert!(credit.contains("支付宝扫码支付"));
    assert!(credit.contains("请使用支付宝扫一扫完成支付，支付结果以服务端确认为准。"));
    assert!(credit.contains("生成支付二维码"));
    assert!(membership.contains("支付宝扫码支付"));
    assert!(membership.contains("AppState.close-payment-window();"));
    assert!(top_bar.contains("关闭支付码"));

    let combined = format!("{credit}\n{membership}\n{top_bar}");
    for removed in ["支付宝收银台", "打开支付宝支付", "关闭收银台"] {
        assert!(!combined.contains(removed), "obsolete payment copy: {removed}");
    }
}

#[test]
fn recovered_pending_payment_reopens_the_embedded_surface() {
    let callbacks = include_str!("callbacks/payment.rs");
    assert!(!callbacks.contains(
        "continue_payment_order(&app, context, backend, started, false);"
    ));
}
```

- [ ] **Step 2: Run the copy test and verify it fails on current checkout wording**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
cargo test -p artforge-studio-native payment_ui_uses_embedded_alipay_qr_copy -- --nocapture
cargo test -p artforge-studio-native recovered_pending_payment_reopens_the_embedded_surface -- --nocapture
```

Expected: both tests FAIL because the required QR strings/close callback are absent, old checkout strings remain, and recovery still passes `false`.

- [ ] **Step 3: Replace the credit dialog wording**

In `ali-pay-qr-dialog.slint`, make these exact replacements:

```slint
SectionTitle { text: AppState.en ? "Alipay QR payment" : "支付宝扫码支付"; }
```

```slint
text: AppState.en ? "Scan with Alipay to complete payment. The server-confirmed result is authoritative." : "请使用支付宝扫一扫完成支付，支付结果以服务端确认为准。";
```

Use that second block inside the existing 90px explanatory rectangle, replacing the sentence about opening a secure checkout. Replace the primary button expression with:

```slint
text: AppState.credit-payment-busy ? (AppState.en ? "Waiting..." : "等待支付...") : (AppState.en ? "Generate payment QR" : "生成支付二维码");
```

- [ ] **Step 4: Make membership and top-bar controls close the embedded view with QR wording**

In `membership-dialog.slint`, replace the close button with:

```slint
DialogCloseButton {
    x: parent.width - 42px;
    y: 12px;
    clicked => {
        AppState.close-payment-window();
        AppState.membership-open = false;
    }
}
```

Replace the existing single-line `membership-payment-message` text with this same-height two-line payment status:

```slint
Text {
    height: 44px;
    text: AppState.membership-payment-busy ? ((AppState.en ? "Alipay QR payment" : "支付宝扫码支付") + "\n" + AppState.membership-payment-message) : AppState.membership-payment-message;
    color: AppState.membership-payment-busy ? AppTheme.accent : AppTheme.text;
    font-size: 13px;
    horizontal-alignment: center;
    vertical-alignment: center;
    wrap: word-wrap;
}
```

In `top-bar.slint`, replace the active-payment label with:

```slint
text: AppState.en ? "Hide payment QR" : "关闭支付码";
```

- [ ] **Step 5: Preserve QR load errors and keep backend polling authoritative**

In `continue_payment_order` in `payment.rs`, replace the current `if open_checkout` block and both pending message assignments with:

```rust
    let checkout_error = if open_checkout {
        started
            .order
            .payment
            .as_ref()
            .and_then(|payment| payment.checkout_url.as_deref())
            .and_then(|url| open_payment_window(app, url).err())
            .map(|_| "支付二维码加载失败，请关闭后重试。")
    } else {
        None
    };
    state.set_credit_payment_busy(true);
    state.set_membership_payment_busy(true);
    let credit_message = checkout_error.unwrap_or(if started.order.status == "paid" {
        "付款已确认，正在等待权益到账..."
    } else {
        "正在等待支付宝付款结果..."
    });
    let membership_message = checkout_error.unwrap_or(if started.order.status == "paid" {
        "付款已确认，正在等待会员权益生效..."
    } else {
        "正在等待支付宝付款结果..."
    });
    state.set_credit_payment_message(credit_message.into());
    state.set_membership_payment_message(membership_message.into());
```

This deliberately discards the low-level WebView error text from the UI, so no signed URL or platform detail can leak. Leave the following `poll_payment_order(...)` call in place even when `checkout_error` is set.

- [ ] **Step 6: Reopen valid checkout pages during startup recovery**

In `poll_recovered_order`, replace:

```rust
continue_payment_order(&app, context, backend, started, false);
```

with:

```rust
continue_payment_order(&app, context, backend, started, true);
```

Do not alter `remove_pending_order`: it must still run only for fulfilled or closed/expired orders.

- [ ] **Step 7: Run focused UI and payment-state tests**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
cargo test -p artforge-studio-native payment_ui_uses_embedded_alipay_qr_copy -- --nocapture
cargo test -p artforge-studio-native recovered_pending_payment_reopens_the_embedded_surface -- --nocapture
cargo test -p artforge-studio-native runtime::callbacks::payment::tests -- --nocapture
rg -n "打开支付宝|打开收银台|支付宝收银台|关闭收银台|系统浏览器启动失败" native-client/src native-client/ui
```

Expected: both test commands PASS and `rg` prints no user-facing matches.

- [ ] **Step 8: Review the Task 4 diff without committing**

Run:

```bash
git diff --check -- native-client/ui/dialogs/ali-pay-qr-dialog.slint native-client/ui/dialogs/membership-dialog.slint native-client/ui/components/top-bar.slint native-client/src/runtime/callbacks/payment.rs native-client/src/runtime/tests.rs
```

Expected: no output and exit code 0. Do not run `git commit`.

---

### Task 5: Prove both product families receive a trusted Mock checkout URL

**Files:**
- Modify: `server/artforge-api/bin/dev-mock-api-server`
- Modify: `ArtForgeStudio/native-client/src/runtime/api/cross_stack_tests.rs`

**Interfaces:**
- Consumes: unchanged backend order response `OrderDetail.payment.checkout_url: Option<String>`.
- Produces: Mock URL `https://openapi.alipay.com/gateway.do?mock_order=<orderNo>`; it is a string fixture and is never loaded during cross-stack API tests.

- [ ] **Step 1: Add failing checkout assertions to the existing payment matrix**

Add this helper near the other assertion helpers in `cross_stack_tests.rs`:

```rust
fn assert_trusted_mock_checkout(order: &OrderDetail) {
    let checkout_url = order
        .payment
        .as_ref()
        .and_then(|payment| payment.checkout_url.as_deref())
        .expect("pending payment exposes checkout URL");
    assert!(checkout_url.starts_with("https://openapi.alipay.com/gateway.do?mock_order="));
}
```

In `cross_stack_payment_parameter_matrix_and_idempotency`, add this after the credit order status assertion:

```rust
assert_trusted_mock_checkout(&order);
```

Add this after the membership order status assertion:

```rust
assert_trusted_mock_checkout(&membership_order);
```

- [ ] **Step 2: Start the dev Mock API and verify the test fails on the old `.test` host**

In terminal A, run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
NODE_ENV=dev ARTFORGE_ENABLE_MOCK_API=1 ARTFORGE_MOCK_PORT=39091 npm run mock:api
```

Expected: the Mock API listens on `127.0.0.1:39091` and does not call Alipay.

In terminal B, run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
ARTFORGE_CROSS_STACK_BASE_URL=http://127.0.0.1:39091 ARTFORGE_MOCK_EMAIL_CODE=654321 cargo test -p artforge-studio-native cross_stack_payment_parameter_matrix_and_idempotency -- --ignored --nocapture
```

Expected: FAIL at `assert_trusted_mock_checkout` because the fixture currently uses `openapi.alipay.test`.

- [ ] **Step 3: Change only the Mock checkout fixture host**

In `server/artforge-api/bin/dev-mock-api-server`, replace the `alipay.pagePay` fixture with:

```js
alipay.pagePay = async ({ orderNo }) => ({
  checkoutUrl: `https://openapi.alipay.com/gateway.do?mock_order=${encodeURIComponent(orderNo)}`,
  outTradeNo: orderNo,
});
```

The test must never pass this fixture to Wry or make a network request to that URL.

- [ ] **Step 4: Rerun the payment matrix and repeated-sync test**

With terminal A still running, run in terminal B:

```bash
ARTFORGE_CROSS_STACK_BASE_URL=http://127.0.0.1:39091 ARTFORGE_MOCK_EMAIL_CODE=654321 cargo test -p artforge-studio-native cross_stack_payment_parameter_matrix_and_idempotency -- --ignored --nocapture
ARTFORGE_CROSS_STACK_BASE_URL=http://127.0.0.1:39091 ARTFORGE_MOCK_EMAIL_CODE=654321 cargo test -p artforge-studio-native cross_stack_payment_required_fields_exact_boundaries_and_repeated_sync -- --ignored --nocapture
```

Expected: both ignored cross-stack tests PASS; membership and credit orders remain `pending_payment`, expose the trusted Mock URL, preserve idempotent replay, and remain pending after repeated synchronization.

- [ ] **Step 5: Stop the Mock API and review the diff without committing**

Stop terminal A with Ctrl-C, then run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio
git diff --check -- server/artforge-api/bin/dev-mock-api-server ArtForgeStudio/native-client/src/runtime/api/cross_stack_tests.rs
```

Expected: no output and exit code 0. Do not run `git commit`.

---

### Task 6: Update payment documentation to match implemented behavior

**Files:**
- Modify: `server/artforge-api/docs/MEMBERSHIP_PAYMENT_API.md`
- Modify: `ArtForgeStudio/docs/FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md`

**Interfaces:**
- Documents the unchanged API response shape and the new rendering/security behavior from Tasks 1–5.
- Does not document private keys, signed URLs, Redis credentials, or any production secret.

- [ ] **Step 1: Replace the backend payment-mode explanation**

In `MEMBERSHIP_PAYMENT_API.md`, rename `## 电脑网站支付` to `## 应用内支付宝扫码支付` and replace the two paragraphs below the response example with:

```markdown
服务端使用 `alipay.trade.page.pay` 和 `FAST_INSTANT_TRADE_PAY` 生成签名支付地址，并固定发送 `qr_pay_mode=4`、`qrcode_width=220`，由支付宝返回适合嵌入页面的二维码。会员购买、续费、升级和积分充值共用该支付初始化逻辑。客户端只在 macOS WKWebView 或 Windows WebView2 子视图中加载 `checkout_url`，不打开系统浏览器，也不自行生成二维码。地址仅在订单待支付且未过期时返回。

嵌入二维码模式不发送 `return_url`。开发环境可以保持 `notify_url` 为空并依靠客户端主动同步；生产环境仍必须配置公网 HTTPS 异步通知。客户端每 3 秒同步一次订单，但支付成功和权益到账只认服务端返回的 `status` 与 `fulfillment_status`。
```

Remove the obsolete paragraph that says a future private `return_url` can be filled in.

- [ ] **Step 2: Update the client/backend execution record**

In `FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md`, replace the F10 row with:

```markdown
| F10 | 支付界面仍是本地二维码假流程 | 改为支付宝电脑网站支付嵌入式二维码，macOS 使用 WKWebView、Windows 使用 WebView2 |
```

Replace the completed phase-7 checkout item with:

```markdown
- [x] 会员与积分订单加载 `qr_pay_mode=4` 的支付宝签名页面，在应用内显示二维码，不打开系统浏览器。
```

Append this entry after the existing 2026-07-15 cross-stack verification section:

```markdown
### 2026-07-16 支付宝应用内二维码 dev 验收

- 后端会员购买、续费、升级和积分充值继续共用 `alipay.trade.page.pay`，并统一发送 `qr_pay_mode=4`、`qrcode_width=220`；嵌入模式不发送 `return_url`。
- macOS 使用 WKWebView、Windows 使用 WebView2，在应用主窗口内加载支付宝签名页面；初始地址只接受支付宝 HTTPS 网关，后续跳转限制为支付宝受信任域名，下载和新窗口被拒绝。
- dev Mock API 只验证会员与积分订单均返回受信任格式的 `checkout_url`、客户端 DTO 解析和订单轮询，不加载真实支付宝交易，也不代表真实付款验收。
- 真实小额支付、公网 HTTPS 异步回调和 Windows release runner 仍属于正式发布阶段，不是当前 dev 阻塞项。
```

- [ ] **Step 3: Check documentation for contradictory checkout wording**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio
rg -n "系统浏览器|打开支付宝|打开收银台|return_url|二维码|WKWebView|WebView2" server/artforge-api/docs/MEMBERSHIP_PAYMENT_API.md ArtForgeStudio/docs/FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md
```

Expected: mentions of system-browser/checkout behavior appear only in historical context or explicit statements that the application does not use them; `return_url` appears only in the statement that embedded mode does not send it.

- [ ] **Step 4: Review the Task 6 diff without committing**

Run:

```bash
git diff --check -- server/artforge-api/docs/MEMBERSHIP_PAYMENT_API.md ArtForgeStudio/docs/FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md
```

Expected: no output and exit code 0. Do not run `git commit`.

---

### Task 7: Run full backend, client, and safety verification

**Files:**
- Verify all files listed in Tasks 1–6.
- Do not create or modify migrations, production config files, or secret files.

**Interfaces:**
- Verifies the complete feature boundary: parameter generation → order API → trusted URL policy → embedded view construction → polling/recovery → fulfillment refresh.

- [ ] **Step 1: Run the backend suite, lint, and configuration validation**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
npm test
npm run lint
npm run config:check
npm run membership:check
```

Expected: all commands exit 0. `membership:check` uses its Mock adapter to cover membership purchase, renewal, upgrade, and credit recharge; no real Alipay request is made.

- [ ] **Step 2: Run the full active-client suite and macOS compile check**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
cargo test -p artforge-studio-native
cargo check -p artforge-studio-native
```

Expected: both commands exit 0. The current local toolchain does not provide `cargo fmt` or `cargo clippy`; `git diff --check` is the formatting gate for this dev task.

- [ ] **Step 3: Verify payment secrets and signed URLs were not introduced**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio
rg -n "LTAI|sk-[A-Za-z0-9_-]{12,}|access_secret|private_key.*BEGIN|checkout_url.*console|checkoutUrl.*logger" server/artforge-api ArtForgeStudio/native-client ArtForgeStudio/docs
```

Expected: no newly added credential or signed-checkout logging match. Existing configuration field names without literal secret values are acceptable and must be reviewed manually.

- [ ] **Step 4: Verify no database or Redis scope expansion occurred**

Run:

```bash
git status --short
git diff --name-only -- server/artforge-api/migrations server/artforge-api/database server/artforge-api/configs ArtForgeStudio/crates
```

Expected: the scoped diff contains no changes introduced by Tasks 1–6. The general status and scoped diff may show unrelated pre-existing user changes; leave them untouched and compare them with the File Map before proceeding.

- [ ] **Step 5: Perform the local Mock API/client smoke check without paying**

In terminal A, run the same Mock API used in Task 5:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
NODE_ENV=dev ARTFORGE_ENABLE_MOCK_API=1 ARTFORGE_MOCK_PORT=39091 npm run mock:api
```

In terminal B, run the client against that local backend:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
ARTFORGE_API_BASE_URL=http://127.0.0.1:39091 cargo run -p artforge-studio-native --bin ArtForgeStudio
```

Sign in with a dev email and code `654321`, open one credit pack and one membership plan, and stop after verifying each action creates a Mock order and displays the child WebView inside the main application. The fixture URL contains no valid Alipay signature and may render an Alipay error page; the smoke criterion is the embedded surface and lifecycle, not a QR payload. Confirm no system browser opens, the top bar can hide the surface, polling continues after hide, and restarting recovers the unexpired pending order. Do not scan or pay anything. Stop terminal A with Ctrl-C after the client exits.

Expected observations:

```text
积分：生成支付二维码 → 应用内支付宝扫码支付 → 可关闭支付码 → 订单仍待支付
会员：购买/续费/升级 → 应用内支付宝扫码支付 → 可关闭支付码 → 订单仍待支付
重启：有效待支付订单重新显示二维码；过期/关闭订单不再显示
```

- [ ] **Step 6: Run the final diff-quality checkpoint**

Run:

```bash
cd /Users/fanxiao/workstation/ai/ArtForgeStudio
git diff --check
git diff --stat
```

Expected: `git diff --check` exits 0. Review `git diff --stat` against the File Map and do not commit.
