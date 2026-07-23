# Canvas Action Bar Above Node Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every infinite-canvas node action bar stay above its node and distribute its buttons evenly across the bar background.

**Architecture:** Keep the change inside `CanvasNodeCard`. A shared `action-bar-y()` function owns the fixed above-node offset, while each `HorizontalLayout` gives every `CanvasMediaAction` the same stretch factor instead of fixed widths.

**Tech Stack:** Rust 2021 tests, Slint 1.8 UI, Cargo.

## Global Constraints

- Apply the behavior to text, image, video, and audio node action bars.
- Action bars may extend outside the canvas viewport and must never flip inside a node.
- Preserve existing labels, icons, callbacks, total bar widths, padding, spacing, and zoom scaling.
- Do not modify archived crates under `crates/`.
- Do not push to the remote repository.

---

### Task 1: Lock the action-bar positioning behavior with a failing test

**Files:**
- Modify: `native-client/src/runtime/tests.rs`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: `CanvasNodeCard` source in `native-client/ui/pages/infinite-canvas-page.slint`
- Produces: regression test `infinite_canvas_action_bars_stay_above_nodes_at_every_zoom`

- [ ] **Step 1: Write the failing test**

Add this test after `infinite_canvas_media_editor_stays_below_node_at_every_zoom`:

```rust
#[test]
fn infinite_canvas_action_bars_stay_above_nodes_at_every_zoom() {
    let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
    let node = page
        .split("component CanvasNodeCard")
        .nth(1)
        .and_then(|value| value.split("export component InfiniteCanvasPage").next())
        .expect("canvas node component");
    let action_bar_y = node
        .split("function action-bar-y()")
        .nth(1)
        .and_then(|value| value.split("function dropdown-popup-x").next())
        .expect("action bar y function");
    let text_bar = node
        .split("text-action-bar := Rectangle")
        .nth(1)
        .and_then(|value| value.split("media-action-bar := Rectangle").next())
        .expect("text action bar");
    let media_bar = node
        .split("media-action-bar := Rectangle")
        .nth(1)
        .and_then(|value| value.split("media-editor-panel := Rectangle").next())
        .expect("media action bar");

    assert!(action_bar_y.contains("return -76px * root.node-scale();"));
    assert!(!action_bar_y.contains("root.y"));
    assert!(!action_bar_y.contains("root.viewport-height"));
    assert!(text_bar.contains("y: root.action-bar-y();"));
    assert!(media_bar.contains("y: root.action-bar-y();"));
    assert!(!text_bar.contains("root.y <"));
    assert!(!media_bar.contains("root.y <"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```powershell
cargo test -p artforge-studio-native runtime::tests::infinite_canvas_action_bars_stay_above_nodes_at_every_zoom -- --exact
```

Expected: FAIL because `function action-bar-y()` does not exist.

- [ ] **Step 3: Stop after confirming the expected RED state**

Do not modify Slint production code until the failure is confirmed to come from the missing shared positioning function.

---

### Task 2: Keep every action bar above its node

**Files:**
- Modify: `native-client/ui/pages/infinite-canvas-page.slint:588-608`
- Modify: `native-client/ui/pages/infinite-canvas-page.slint:985-1037`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: `CanvasNodeCard.node-scale() -> float`
- Produces: `CanvasNodeCard.action-bar-y() -> length`

- [ ] **Step 1: Add the minimal shared positioning function**

Immediately after `media-editor-y()`, add:

```slint
function action-bar-y() -> length {
    return -76px * root.node-scale();
}
```

- [ ] **Step 2: Route both action bars through the shared function**

In both `text-action-bar` and `media-action-bar`, replace the conditional `y` expression with:

```slint
y: root.action-bar-y();
```

- [ ] **Step 3: Run the targeted positioning test**

Run:

```powershell
cargo test -p artforge-studio-native runtime::tests::infinite_canvas_action_bars_stay_above_nodes_at_every_zoom -- --exact
```

Expected: PASS.

---

### Task 3: Lock equal button distribution with a failing test

**Files:**
- Modify: `native-client/src/runtime/tests.rs`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: text, video, and non-video action-bar `HorizontalLayout` source
- Produces: regression test `infinite_canvas_action_bar_buttons_evenly_fill_the_background`

- [ ] **Step 1: Write the failing test**

Add:

```rust
#[test]
fn infinite_canvas_action_bar_buttons_evenly_fill_the_background() {
    let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
    let node = page
        .split("component CanvasNodeCard")
        .nth(1)
        .and_then(|value| value.split("export component InfiniteCanvasPage").next())
        .expect("canvas node component");
    let text_bar = node
        .split("text-action-bar := Rectangle")
        .nth(1)
        .and_then(|value| value.split("media-action-bar := Rectangle").next())
        .expect("text action bar");
    let media_bar = node
        .split("media-action-bar := Rectangle")
        .nth(1)
        .and_then(|value| value.split("media-editor-panel := Rectangle").next())
        .expect("media action bar");
    let video_actions = media_bar
        .split("if root.note.kind == \"video\": HorizontalLayout")
        .nth(1)
        .and_then(|value| {
            value
                .split("if root.note.kind != \"video\": HorizontalLayout")
                .next()
        })
        .expect("video action layout");
    let other_actions = media_bar
        .split("if root.note.kind != \"video\": HorizontalLayout")
        .nth(1)
        .expect("image and audio action layout");

    assert_eq!(
        text_bar
            .matches("CanvasMediaAction { horizontal-stretch: 1;")
            .count(),
        4
    );
    assert_eq!(
        video_actions
            .matches("CanvasMediaAction { horizontal-stretch: 1;")
            .count(),
        4
    );
    assert_eq!(
        other_actions
            .matches("CanvasMediaAction { horizontal-stretch: 1;")
            .count(),
        3
    );
    assert!(!text_bar.contains("CanvasMediaAction { scale-factor: root.node-scale(); width:"));
    assert!(!media_bar.contains("CanvasMediaAction { scale-factor: root.node-scale(); width:"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```powershell
cargo test -p artforge-studio-native runtime::tests::infinite_canvas_action_bar_buttons_evenly_fill_the_background -- --exact
```

Expected: FAIL because the action buttons still declare fixed widths and none declares `horizontal-stretch: 1`.

---

### Task 4: Evenly distribute every action-bar button

**Files:**
- Modify: `native-client/ui/pages/infinite-canvas-page.slint:994-1036`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: Slint `HorizontalLayout` stretch allocation
- Produces: equal-width text, video, image, and audio action buttons

- [ ] **Step 1: Replace fixed widths with equal stretch**

Replace the four actions inside `text-action-bar` with:

```slint
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/info.svg"); text: AppState.en ? "Info" : "信息"; clicked => { AppState.canvas-show-image-info = !AppState.canvas-show-image-info; } }
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/trash.svg"); text: AppState.en ? "Delete" : "删除"; danger: true; clicked => { AppState.pending-delete-kind = "canvas-note"; AppState.pending-delete-id = root.note.id; AppState.pending-delete-source = "canvas"; AppState.delete-confirm-open = true; } }
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/edit.svg"); text: AppState.en ? "Edit" : "编辑"; clicked => { root.current-content = root.is-default-text-prompt() ? "" : root.note.content; root.editing = true; text-editor.focus(); } }
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/image.svg"); text: AppState.en ? "Generate" : "生图"; clicked => { root.generate-from-text(); } }
```

Replace the four actions inside the video `HorizontalLayout` with:

```slint
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/info.svg"); text: AppState.en ? "Info" : "信息"; clicked => { AppState.canvas-show-image-info = !AppState.canvas-show-image-info; } }
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/trash.svg"); text: AppState.en ? "Delete" : "删除"; danger: true; clicked => { AppState.pending-delete-kind = "canvas-note"; AppState.pending-delete-id = root.note.id; AppState.pending-delete-source = "canvas"; AppState.delete-confirm-open = true; } }
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/edit.svg"); text: AppState.en ? "Edit" : "编辑"; clicked => { root.editing = true; } }
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/upload.svg"); text: AppState.en ? "Upload video" : "上传视频"; clicked => { AppState.generation-status = AppState.en ? "Video upload requires a configured video provider" : "请先配置视频生成服务"; } }
```

Replace the three actions inside the non-video `HorizontalLayout` with:

```slint
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/info.svg"); text: AppState.en ? "Info" : "信息"; clicked => { AppState.canvas-show-image-info = !AppState.canvas-show-image-info; } }
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/trash.svg"); text: AppState.en ? "Delete" : "删除"; danger: true; clicked => { AppState.pending-delete-kind = "canvas-note"; AppState.pending-delete-id = root.note.id; AppState.pending-delete-source = "canvas"; AppState.delete-confirm-open = true; } }
CanvasMediaAction { horizontal-stretch: 1; scale-factor: root.node-scale(); icon: @image-url("../../assets/icons/upload.svg"); text: root.note.kind == "image" ? (AppState.en ? "Upload image" : "上传图片") : (AppState.en ? "Upload audio" : "上传音频"); clicked => { if root.note.kind == "image" { AppState.add-reference(); } else { AppState.generation-status = AppState.en ? "Audio upload requires a configured audio provider" : "请先配置音频生成服务"; } } }
```

Keep the existing `HorizontalLayout` padding and spacing unchanged.

- [ ] **Step 2: Run both targeted tests**

Run:

```powershell
cargo test -p artforge-studio-native runtime::tests::infinite_canvas_action_bars_stay_above_nodes_at_every_zoom -- --exact
cargo test -p artforge-studio-native runtime::tests::infinite_canvas_action_bar_buttons_evenly_fill_the_background -- --exact
```

Expected: both tests PASS.

- [ ] **Step 3: Run formatting and source checks**

Run:

```powershell
cargo fmt --all -- --check
git diff --check
```

Expected: both commands exit with code 0.

- [ ] **Step 4: Commit the implementation**

```powershell
git add -- native-client/src/runtime/tests.rs native-client/ui/pages/infinite-canvas-page.slint
git commit -m "Keep canvas action bars above nodes"
```

---

### Task 5: Verify and rebuild the Windows portable package

**Files:**
- Verify: `native-client/src/runtime/tests.rs`
- Verify: `native-client/ui/pages/infinite-canvas-page.slint`
- Generate: `dist/ArtForgeStudio_1.0.2_windows_x64_portable.zip`
- Copy: `D:\ArtForgeStudio\ArtForgeStudio_1.0.2_windows_x64_portable.zip`

**Interfaces:**
- Consumes: committed action-bar implementation
- Produces: verified Windows x64 portable package

- [ ] **Step 1: Run the complete native-client test suite**

```powershell
cargo test -p artforge-studio-native
```

Expected: all non-ignored tests PASS with 0 failures.

- [ ] **Step 2: Run the native-client compiler check**

```powershell
cargo check -p artforge-studio-native
```

Expected: exit code 0.

- [ ] **Step 3: Build the Windows portable package**

```powershell
& .\scripts\package-native-client.ps1 -Target windows
```

Expected: `dist\ArtForgeStudio_1.0.2_windows_x64_portable.zip` is generated successfully.

- [ ] **Step 4: Copy and verify the package**

```powershell
$destination = "D:\ArtForgeStudio"
New-Item -ItemType Directory -Force -Path $destination | Out-Null
Copy-Item -LiteralPath ".\dist\ArtForgeStudio_1.0.2_windows_x64_portable.zip" -Destination (Join-Path $destination "ArtForgeStudio_1.0.2_windows_x64_portable.zip") -Force
Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $destination "ArtForgeStudio_1.0.2_windows_x64_portable.zip")
```

Expected: the destination package exists and a SHA-256 hash is reported.
