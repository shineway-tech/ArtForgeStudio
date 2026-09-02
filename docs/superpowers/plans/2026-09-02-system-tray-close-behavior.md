# System Tray Close Behavior Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first-close choice between exiting and hiding Elunvi Canvas to the system tray, with a persistent Basic Settings option.

**Architecture:** Store a normalized close preference in `UserProfileData`, expose it through `AppState`, and intercept the native close request in Rust. A Slint `SystemTrayIcon` keeps the process alive and provides restore/exit actions; a Slint modal handles the first unresolved close.

**Tech Stack:** Rust, Slint 1.17.1, serde JSON/SQLite-backed client profile, Cargo tests, Windows portable packaging.

**Spec:** `docs/superpowers/specs/2026-09-02-system-tray-close-behavior-design.md`

## Global Constraints

- Missing or unknown stored preference must resolve to `ask`.
- Hiding to tray must remove the main window from the taskbar without terminating the process.
- Existing portable user data must be preserved during packaging.
- Do not add a second tray library; use Slint's built-in `SystemTrayIcon`.

---

### Task 1: Persisted close preference

**Files:**
- Modify: `native-client/src/runtime/model.rs`
- Modify: `native-client/src/runtime/storage/local_store.rs`
- Modify: `native-client/ui/app-state.slint`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Produces: `normalize_close_behavior(&str) -> &'static str`, `AppState.close-behavior`, and `UserProfileData.close_behavior`.

- [ ] Add failing tests proving legacy `{}` resolves to `ask`, valid `exit`/`tray` values round-trip, and invalid values normalize to `ask`.
- [ ] Run `cargo test -p artforge-studio-native close_behavior -- --nocapture` and confirm it fails before production changes.
- [ ] Add the serde-default field, normalization helper, startup application, and serialization from `AppState`.
- [ ] Re-run the focused test and confirm it passes.

### Task 2: Close dialog and Basic Settings selector

**Files:**
- Create: `native-client/ui/dialogs/close-behavior-dialog.slint`
- Create: `native-client/ui/components/close-behavior-option.slint`
- Modify: `native-client/ui/app.slint`
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/ui/pages/settings-page.slint`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: `AppState.close-behavior`.
- Produces: `AppState.close-choice-open`, `set-close-behavior(string)`, and `confirm-close-behavior(string)`.

- [ ] Add failing integration assertions for both dialog choices, the Basic Settings options, and the AppState callbacks.
- [ ] Run the focused test and confirm it fails.
- [ ] Add the modal and selector, increasing Basic Settings scroll height and moving lower sections without overlap.
- [ ] Re-run the focused test and confirm it passes.

### Task 3: Native close interception and system tray

**Files:**
- Modify: `native-client/ui/app.slint`
- Modify: `native-client/src/runtime/app.rs`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: normalized `ask`/`exit`/`tray` behavior.
- Produces: exported `AppTray`, `wire_close_behavior`, tray restore, tray menu exit, and main-window close interception.

- [ ] Add failing assertions for a visible Slint `SystemTrayIcon`, left-click restore, native close interception, and explicit tray exit.
- [ ] Run the focused test and confirm it fails.
- [ ] Instantiate and retain the tray component, wire restore/exit callbacks, and intercept `on_close_requested`.
- [ ] Wire modal/settings callbacks to persist before acting.
- [ ] Re-run the focused test and confirm it passes.

### Task 4: Verification and packaging

**Files:**
- Verify all changed files and `dist/ElunviCanvas-windows-x64`.

**Interfaces:**
- Consumes: completed close behavior implementation.
- Produces: verified portable build with preserved user data.

- [ ] Run `cargo fmt --check` and the complete native-client test suite.
- [ ] Run the release build/package script and verify exit code 0.
- [ ] Compare the portable `data` file count before and after packaging.
- [ ] Launch the packaged executable, verify the process remains running, and commit the verified changes locally without pushing.
