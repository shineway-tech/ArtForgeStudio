# Canvas Media Editor Below Node Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the infinite-canvas media editor panel below its selected node at every zoom level, even when the panel extends beyond the visible canvas.

**Architecture:** Preserve the existing `CanvasNodeCard` coordinate system and scaled gap. Replace only the viewport-dependent vertical flip in `media-editor-y()` with a deterministic node-bottom anchor; keep secondary `PopupWindow` boundary logic unchanged.

**Tech Stack:** Rust 2021 tests, Slint 1.16.1

## Global Constraints

- The main media editor panel must always start below the node.
- The vertical gap remains 20px multiplied by the canvas zoom scale.
- The panel may extend outside the visible canvas.
- Secondary model and settings popups retain their existing viewport constraints.
- Do not change node sizing, dragging, panning, generation logic, or server-facing structures.

---

### Task 1: Anchor the media editor below the selected node

**Files:**
- Modify: `native-client/src/runtime/tests.rs`
- Modify: `native-client/ui/pages/infinite-canvas-page.slint`

**Interfaces:**
- Consumes: `CanvasNodeCard.height` and `CanvasNodeCard.node-scale() -> float`
- Produces: `CanvasNodeCard.media-editor-y() -> length`

- [ ] **Step 1: Write the failing regression test**

Add this test beside the existing infinite-canvas overlay tests:

```rust
#[test]
fn infinite_canvas_media_editor_stays_below_node_at_every_zoom() {
    let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
    let node = page
        .split("component CanvasNodeCard")
        .nth(1)
        .and_then(|value| value.split("export component InfiniteCanvasPage").next())
        .expect("canvas node component");
    let editor_y = node
        .split("function media-editor-y()")
        .nth(1)
        .and_then(|value| value.split("function dropdown-popup-x").next())
        .expect("media editor y function");

    assert!(editor_y.contains("return root.height + 20px * root.node-scale();"));
    assert!(!editor_y.contains("root.viewport-height"));
    assert!(!editor_y.contains("-root.media-editor-height()"));
}
```

- [ ] **Step 2: Run the regression test and verify RED**

Run:

```powershell
cargo test -p artforge-studio-native infinite_canvas_media_editor_stays_below_node_at_every_zoom
```

Expected: FAIL because the current function checks `root.viewport-height` and can return a negative Y position.

- [ ] **Step 3: Implement the fixed below-node anchor**

Replace the body of `media-editor-y()` with:

```slint
function media-editor-y() -> length {
    return root.height + 20px * root.node-scale();
}
```

- [ ] **Step 4: Verify GREEN and the complete client**

Run:

```powershell
cargo test -p artforge-studio-native infinite_canvas_media_editor_stays_below_node_at_every_zoom
cargo test -p artforge-studio-native
cargo check -p artforge-studio-native
```

Expected: the regression test passes, the full local suite has zero failures, and Slint compilation succeeds.

- [ ] **Step 5: Review and commit**

Run:

```powershell
git diff --check
git diff -- native-client/src/runtime/tests.rs native-client/ui/pages/infinite-canvas-page.slint
git add native-client/src/runtime/tests.rs native-client/ui/pages/infinite-canvas-page.slint docs/superpowers/plans/2026-07-23-canvas-media-editor-below-node.md
git commit -m "Keep canvas media editor below nodes"
```

