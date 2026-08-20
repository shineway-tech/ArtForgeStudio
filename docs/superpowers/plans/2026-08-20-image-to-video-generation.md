# Image-to-Video Generation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a server-authoritative image-to-video settings flow and an in-app Elunvi video player reached from the current image viewer.

**Architecture:** A dedicated Slint page owns the editable draft and presentation state. Rust callbacks request quotes, upload the source image, submit/recover the existing generation-task protocol, verify delivery, and hand a local MP4 to a restricted Wry child webview with a custom HTML/CSS control layer.

**Tech Stack:** Rust 2021, Slint 1.17.1, reqwest 0.12, serde, Wry 0.55.1, existing generation/upload/storage services.

**Spec:** `docs/superpowers/specs/2026-08-20-image-to-video-generation-design.md`

## Global Constraints

- Preserve all current worktree changes and existing user data.
- Server remains authoritative for model capabilities, pricing, credits and task state.
- Supported ratios are `21:9`, `16:9`, `4:3`, `1:1`, `3:4`, `9:16`.
- Supported resolutions are `480P`, `720P`, `1080P`; duration is an integer from 4 through 15 seconds.
- The client must never invent a billable price or a successful video when the service is unavailable.
- Only verified local video files may be loaded by the embedded player.

---

### Task 1: Viewer entry and video settings surface

**Files:**
- Create: `native-client/ui/pages/video-generation-page.slint`
- Modify: `native-client/ui/app.slint`
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/ui/dialogs/viewer-overlay.slint`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: existing `AppState.viewer-*` properties and `viewer-open-image-editor()` callback.
- Produces: `viewer-generate-video()`, `close-video-generation()`, `request-video-quote(string,string,int)`, `submit-video-generation()`, and page-bound video state properties.

- [ ] **Step 1: Write the failing UI contract tests**

Add tests asserting that the footer label is “生成视频”, its click calls `viewer-generate-video`, the right action is “局部修改” and calls `viewer-open-image-editor`, and `app.slint` mounts `VideoGenerationPage` for `AppState.page == "video-generation"`.

- [ ] **Step 2: Run the focused tests and verify RED**

Run `cargo test -p artforge-studio-native runtime::tests::viewer_opens_the_image_to_video_workspace -- --exact` and expect a missing page/callback assertion.

- [ ] **Step 3: Implement the settings page and state contract**

Create a two-column page with source preview, editable prompt, six ratio buttons, three resolution buttons, 4–15 second slider plus stepper, quote/status copy, and a bottom action whose label binds to the server quote. Wire the viewer entry and existing local-edit callback exactly as specified.

- [ ] **Step 4: Run focused tests and Slint compilation**

Run the focused test, then `cargo check -p artforge-studio-native`; both must pass.

### Task 2: Server quote and video task protocol

**Files:**
- Modify: `native-client/src/runtime/api/generation.rs`
- Test: `native-client/src/runtime/api/generation.rs`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: authenticated `ApiClient`, existing decimal-string conventions and generation task endpoints.
- Produces: `CreateVideoQuote`, `VideoQuote`, optional video fields on `CreateGenerationTask`, `GenerationApi::quote_video`, and validation helpers.

- [ ] **Step 1: Write failing serialization and validation tests**

Cover all six ratios, all three resolutions, duration boundaries 4/15, rejection of 3/16, omission of video fields for image requests, decimal-string credit cost, and quote/task JSON field names.

- [ ] **Step 2: Run protocol tests and verify RED**

Run `cargo test -p artforge-studio-native runtime::api::generation::tests -- --nocapture`; expect missing types/methods.

- [ ] **Step 3: Implement minimal protocol types**

Add optional `source_file_id`, `resolution`, `duration_secs`, and `quote_id` to `CreateGenerationTask` with `skip_serializing_if = "Option::is_none"`. Implement `POST /v1/generation/video-quotes` and client-side range validation used only to reject impossible requests before network I/O.

- [ ] **Step 4: Run protocol tests and existing generation tests**

Run the protocol tests and `cargo test -p artforge-studio-native runtime::tests::backend_generation_uses_platform_task_api -- --exact`.

### Task 3: Video workspace callbacks and safe unavailable state

**Files:**
- Create: `native-client/src/runtime/callbacks/video_generation.rs`
- Modify: `native-client/src/runtime/callbacks/mod.rs`
- Modify: `native-client/src/runtime/app.rs`
- Modify: `native-client/src/runtime/model.rs`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: Task 1 callbacks, Task 2 API, current viewer asset, authenticated backend and reference uploader.
- Produces: `wire_video_generation_callbacks`, quote-state updates, a stable `client_request_id`, and a guarded submission path.

- [ ] **Step 1: Write failing callback/state tests**

Assert viewer entry copies source image and prompt, parameter changes invalidate the previous quote, service absence yields “视频服务暂未开放”, and duplicate submits reuse a stable request ID.

- [ ] **Step 2: Run focused tests and verify RED**

Run the new exact tests and expect missing callback/module assertions.

- [ ] **Step 3: Implement entry, quote and submission state**

Wire the page entry, local prompt/parameter state, server quote request and guarded submit. If the deployed service lacks video support, retain the complete page and disable the primary action with the explicit unavailable message; do not calculate fallback credits.

- [ ] **Step 4: Run focused tests and cargo check**

Run all new callback tests plus `cargo check -p artforge-studio-native`.

### Task 4: Restricted embedded video player

**Files:**
- Create: `native-client/src/runtime/video_player.rs`
- Create: `native-client/src/runtime/video_player/player.html`
- Modify: `native-client/src/runtime/mod.rs`
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/ui/pages/video-generation-page.slint`
- Test: `native-client/src/runtime/video_player.rs`

**Interfaces:**
- Consumes: verified local MP4 path and physical bounds reported by the Slint page.
- Produces: `open_video_player`, `set_video_player_bounds`, `close_video_player`, strict IPC command parser, and the custom controls requested in the spec.

- [ ] **Step 1: Write failing security and layout tests**

Test that non-file/absent paths are rejected, HTML escapes the generated file URI, navigation only allows `about:blank`, IPC accepts only play/pause/seek/volume/mute/loop/fullscreen/download/open-folder/regenerate with bounded numbers, and bounds reject zero area.

- [ ] **Step 2: Run player tests and verify RED**

Run `cargo test -p artforge-studio-native runtime::video_player::tests -- --nocapture` and expect the module not to exist.

- [ ] **Step 3: Implement Wry child webview and custom controls**

Use `WebViewBuilder::new().with_html(...)`, deny navigation/new windows/downloads, keep one thread-local child webview, and expose only validated local-file and player-command operations. The HTML renders Elunvi-styled controls and communicates state through the narrow IPC bridge.

- [ ] **Step 4: Run tests on Windows and compile platform gates**

Run player tests and `cargo check -p artforge-studio-native`; non-Windows/macOS builds must retain a clear unsupported error path.

### Task 5: Delivery integration, regression verification and packaging

**Files:**
- Modify: `native-client/src/runtime/generation/poll.rs`
- Modify: `native-client/src/runtime/storage/*` only where existing verified-delivery helpers require video MIME/extension support
- Modify: `native-client/src/runtime/tests.rs`
- Modify: `docs/superpowers/plans/2026-08-20-image-to-video-generation.md`

**Interfaces:**
- Consumes: existing task polling and verified download/acknowledgement sequence.
- Produces: MP4 delivery to the video workspace/player without changing image delivery behavior.

- [ ] **Step 1: Write failing delivery regression tests**

Assert MP4 uses `.mp4`, SHA-256/size verification precedes metadata update and acknowledgment, and image output behavior remains unchanged.

- [ ] **Step 2: Implement the minimal MIME-aware delivery extension**

Route successful `video/mp4` output to the video workspace, persist its local path, and open the player only after atomic save and delivery confirmation readiness.

- [ ] **Step 3: Run full verification**

Run `cargo fmt --check`, `cargo test -p artforge-studio-native`, `cargo check -p artforge-studio-native`, and `cargo build --release -p artforge-studio-native --bin ElunviCanvas`.

- [ ] **Step 4: Perform Windows interaction acceptance**

Launch the newly built executable and verify the viewer labels, settings page layout, all parameter controls, unavailable/quote state, return navigation, existing local modification flow, and player opening against a verified local MP4 fixture.

- [ ] **Step 5: Commit only intended files**

Review `git diff --check` and `git status --short`, then stage only the feature, its tests, design/plan documents, and the already-approved bounded UI fixes. Commit with a descriptive local message; do not push.
