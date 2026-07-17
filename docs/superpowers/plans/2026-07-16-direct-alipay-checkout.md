# Direct Alipay Checkout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make credit recharge and membership purchase create an order immediately, then show the same compact Alipay QR-only payment dialog without a confirmation step or purchase-agreement controls.

**Architecture:** Both purchase entry points keep their existing server callbacks, but the UI no longer opens a pre-payment dialog. After the server returns an order with a checkout URL, Rust opens a shared payment surface and updates shared dialog state. The Slint dialog supplies the header, order summary, white QR frame, close action, and payment status; the child Wry WebView is reduced to the 220×220 QR area.

**Tech Stack:** Rust 2021, Slint 1.8, Wry child WebView, Cargo tests.

**Status:** Completed locally on 2026-07-16. No commit was created.

## Global Constraints

- Apply the flow to both credit recharge and membership purchase/renewal/upgrade.
- Do not show a confirmation dialog or purchase-agreement controls.
- Clicking the purchase action creates the server order immediately.
- Show a compact QR-only Alipay dialog after checkout creation succeeds.
- Keep the existing pending-order recovery and server payment polling behavior.
- Do not create a Git commit.

---

### Task 1: Lock the direct purchase flow with tests

**Files:**
- Modify: `native-client/src/runtime/tests.rs`
- Modify: `native-client/src/runtime/payment_window.rs`

**Interfaces:**
- Consumes: Slint source files through `include_str!` and `payment_webview_config(...)`.
- Produces: Regression checks for direct callbacks, shared QR dialog copy, removed agreement UI, and 220×220 WebView bounds.

- [x] **Step 1: Write failing tests**

Add assertions that the credit button invokes `AppState.recharge-credits(...)`, membership buttons invoke `AppState.purchase-membership(...)`, the QR dialog has no order-creation button or agreement component, and the WebView dimensions are `(220, 220)`.

- [x] **Step 2: Run the focused tests and verify RED**

Run: `cargo test -p artforge-studio-native payment_ui_uses_direct_alipay_qr_flow -- --nocapture`

Expected: FAIL because the credit page still opens the old mixed confirmation dialog.

Run: `cargo test -p artforge-studio-native webview_config_keeps_the_qr_surface_centered -- --nocapture`

Expected: FAIL because the WebView is still `360×380`.

- [x] **Step 3: Preserve the failing-test evidence before production edits**

Record the assertion messages in the task output; do not edit assertions to match the old behavior.

---

### Task 2: Implement the shared QR-only payment surface

**Files:**
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/ui/pages/credits-page.slint`
- Modify: `native-client/ui/dialogs/membership-dialog.slint`
- Modify: `native-client/ui/dialogs/ali-pay-qr-dialog.slint`
- Modify: `native-client/src/runtime/callbacks/payment.rs`
- Modify: `native-client/src/runtime/payment_window.rs`

**Interfaces:**
- Consumes: `AppState.recharge-credits(string)`, `AppState.purchase-membership(string)`, `open_payment_window(...)`, and payment polling.
- Produces: `AppState.payment-qr-open`, `AppState.payment-qr-summary`, and `AppState.payment-qr-message` shared by credit and membership checkout.

- [x] **Step 1: Make the credit action direct**

Change the credit page button to call `AppState.recharge-credits(AppState.selected-credit-pack-code)` directly, disable it while `credit-payment-busy` is true, and surface order-creation errors on the page.

- [x] **Step 2: Remove purchase agreements from both payment entry surfaces**

Delete the membership purchase agreement row and the old QR dialog agreement components. Remove the Rust-side agreement gates and purchase-time agreement acceptance calls.

- [x] **Step 3: Add the shared payment dialog state**

Add these properties to `AppState`:

```slint
in-out property <bool> payment-qr-open: false;
in-out property <string> payment-qr-summary: "";
in-out property <string> payment-qr-message: "";
```

- [x] **Step 4: Open the shared surface only after checkout creation succeeds**

In `continue_payment_order`, set the summary for the selected credit pack or membership plan, close the membership-plan dialog when appropriate, set `payment-qr-open = true`, open the Wry view, and copy polling status into `payment-qr-message`.

- [x] **Step 5: Replace the mixed dialog with a QR-only layout**

Render a `340×380` centered dialog with a title/close header, a short order summary, a `232×232` white QR shell, and a waiting-status row. Do not render cancel/confirm buttons, purchase agreements, or “生成支付二维码”.

- [x] **Step 6: Align the child WebView to the QR shell**

Return a logical `220×220` surface, convert it with the Slint window scale factor, center X, and align Y with the QR shell's logical `18px` offset from `payment_webview_config(...)`.

- [x] **Step 7: Run focused and full tests and verify GREEN**

Run: `cargo test -p artforge-studio-native payment_ui_uses_direct_alipay_qr_flow -- --nocapture`

Expected: PASS.

Run: `cargo test -p artforge-studio-native webview_config_keeps_the_qr_surface_centered -- --nocapture`

Expected: PASS.

Run: `cargo test -p artforge-studio-native`

Expected: all tests PASS.

Run: `cargo check -p artforge-studio-native`

Expected: exit code 0.

---

### Task 3: Runtime verification

**Files:**
- No source changes expected.

**Interfaces:**
- Consumes: local backend at `http://127.0.0.1:3000` and the native client.
- Produces: visual confirmation that both purchase routes share the same QR-only layout.

- [x] **Step 1: Restart the backend and client**

Start the dev backend, then launch the client with `ARTFORGE_API_BASE_URL=http://127.0.0.1:3000`.

- [x] **Step 2: Verify credit recharge**

Select a credit pack and click “充值”; confirm there is no second confirmation or agreement UI and the payment dialog contains only the Alipay QR surface and waiting state.

- [x] **Step 3: Verify membership purchase**

Open membership plans and click purchase/renew/upgrade; confirm the plan dialog gives way to the same payment-only QR dialog.

- [x] **Step 4: Verify close and failure states**

Close the payment dialog and verify the child WebView disappears. Simulate an order-creation failure and verify the source page/dialog shows the server error instead of an empty QR surface.
