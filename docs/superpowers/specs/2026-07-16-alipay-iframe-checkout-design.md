# Alipay iframe Checkout Design

**Date:** 2026-07-16

**Status:** Approved design, pending implementation

## Goal

Make the embedded Alipay payment surface match `market_tool`: the user sees a compact dialog containing only the title, a usable QR code, a close action, and a stable waiting status. The full Alipay website header and raw payment-query errors must not appear inside the dialog.

## Root Cause

The backend correctly creates an `alipay.trade.page.pay` checkout URL with `qr_pay_mode=4` and `qrcode_width=220`. `market_tool` loads that URL as a `220×220` iframe. ArtForgeStudio currently loads the checkout URL as the Wry WebView's top-level document, so Alipay returns/renders its complete website payment page. The fixed-size WebView then clips the page before the QR code.

ArtForgeStudio also copies every transient order-query error into `payment-qr-message`, which exposes internal codes such as `payment_query_failed` in the customer-facing payment dialog. `market_tool` keeps polling and does not replace the normal waiting copy for transient query failures.

## Selected Approach

Keep the existing backend payment API and signed checkout URL. Replace the Wry top-level navigation with a small local HTML wrapper that contains a single trusted Alipay iframe:

```html
<iframe src="TRUSTED_ALIPAY_CHECKOUT_URL" title="支付宝扫码支付"></iframe>
```

The wrapper and iframe have no margins, borders, scrollbars, or background decoration and occupy the complete logical `220×220` payment surface. The checkout URL is validated before HTML construction and HTML-attribute escaped before insertion.

This reproduces the `market_tool` browsing context without introducing a new backend API or requiring `alipay.trade.precreate` permissions.

## UI Layout

The Slint payment dialog uses the same compact structure as `market_tool`:

1. Header: “支付宝扫码支付” and close button.
2. White `232×232` QR frame containing the `220×220` Wry surface.
3. Green status dot and “正在等待支付宝付款结果…”.

Remove the separate order-summary row from the dialog. The dialog width remains `340px`; its height is reduced to fit the three elements and their existing `18px` padding and `16px`-style spacing.

The close action removes the Wry child view and hides the Slint dialog. Existing order recovery and background polling remain unchanged.

## Data Flow

1. Credit or membership purchase creates an order through the existing API.
2. The order returns an approved HTTPS Alipay checkout URL.
3. `payment_window.rs` validates the URL and builds the local iframe wrapper.
4. Wry loads the wrapper HTML; the iframe loads the signed Alipay page.
5. Slint shows the compact payment shell while the existing polling loop queries order status.
6. A fulfilled or closed order closes the child view and updates account state as before.

## Error Handling

- Invalid, non-HTTPS, or unapproved checkout hosts remain rejected before creating the WebView.
- HTML construction must escape `&`, `"`, `'`, `<`, and `>` in the checkout URL.
- A WebView creation failure remains visible on the source credit or membership surface.
- Transient payment-query failures keep the payment dialog status as “正在等待支付宝付款结果…”.
- After the existing retry limit is exhausted, show the user-facing message “暂时无法确认支付结果，请稍后查看订单状态”, without provider codes or internal error text.

## Testing

- Add a unit test proving the generated wrapper contains one iframe, the trusted escaped checkout URL, and the `220×220` no-scroll layout.
- Add a regression assertion that Wry uses wrapper HTML instead of navigating the checkout URL as its top-level document.
- Add a regression assertion that transient polling errors do not update `payment-qr-message` with the raw provider error.
- Keep the existing trusted-host, navigation, Retina scaling, direct credit purchase, direct membership purchase, and pending-order recovery tests.
- Run the full native-client test suite and `cargo check`.

## Out of Scope

- Changing to `alipay.trade.precreate`.
- Generating QR bitmaps in the backend or client.
- Completing a real payment or changing callback/HTTPS configuration.
- Changing order creation, fulfillment, or pending-order persistence semantics.
