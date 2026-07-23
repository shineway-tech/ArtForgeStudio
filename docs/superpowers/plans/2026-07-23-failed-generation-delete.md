# Failed Generation Delete Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a delete control in the upper-right corner of a failed generation card while hovered, then reuse the existing confirmation flow to remove that local failure record.

**Architecture:** Keep the change inside the existing `ThumbnailCard` presentation boundary. The failed-card branch will expose a hover-only trash control and call the already-wired `request-delete-thumbnail` callback with the `generation` source; the existing viewer callback continues to own confirmation, Store mutation, persistence, and UI refresh.

**Tech Stack:** Rust 2021, Slint 1.16.1, Cargo test runner, existing source-contract tests in `native-client/src/runtime/tests.rs`.

## Global Constraints

- Only records with `source_path == "failed"` receive the new control.
- The trash control is visible only while the failed card or its delete hit area is hovered.
- Deletion must reuse the existing confirmation dialog and local `generations` removal path.
- Do not call a server deletion endpoint or change credits, notifications, successful assets, or active tasks.
- Failed cards remain non-draggable and must not open the viewer.
- Successful thumbnail deletion behavior must remain unchanged.

---

### Task 1: Add confirmed deletion to failed generation cards

**Files:**
- Modify: `native-client/src/runtime/tests.rs`
- Modify: `native-client/ui/components/thumbnail-card.slint`

**Interfaces:**
- Consumes: `AppState.request-delete-thumbnail(string, string)` and the existing `"generation"` source handled by `wire_viewer_callbacks`.
- Produces: A failed-card hover region named `failed-hover`, a delete hit region named `failed-delete-touch`, and a call to `AppState.request-delete-thumbnail(root.item.id, "generation")`.

- [ ] **Step 1: Write the failing source-contract test**

Add this test beside `thumbnail_hover_delete_reuses_confirmation_with_explicit_source` in `native-client/src/runtime/tests.rs`:

```rust
#[test]
fn failed_generation_thumbnail_hover_requests_confirmed_delete() {
    let card = include_str!("../../ui/components/thumbnail-card.slint");
    let callbacks = include_str!("callbacks/viewer.rs");

    assert!(card.contains("failed-hover := TouchArea"));
    assert!(card.contains("failed-delete-touch := TouchArea"));
    assert!(card.contains(
        "visible: failed-hover.has-hover || failed-delete-touch.has-hover"
    ));
    assert!(card.contains(
        "AppState.request-delete-thumbnail(root.item.id, \"generation\")"
    ));
    assert!(card.contains("visible: root.item.source-path != \"failed\";"));
    assert!(callbacks.contains("store_mut.generations.retain(|a| a.id != id)"));
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test -p artforge-studio-native failed_generation_thumbnail_hover_requests_confirmed_delete
```

Expected: FAIL because `thumbnail-card.slint` does not yet contain `failed-hover`, `failed-delete-touch`, or the failed-card delete callback.

- [ ] **Step 3: Add the minimal failed-card hover and delete control**

In the active failed-card branch of `native-client/ui/components/thumbnail-card.slint`, keep the existing failure text and retry button, and add:

```slint
failed-hover := TouchArea {
    width: parent.width;
    height: parent.height;
}
failed-delete-touch := TouchArea {
    x: parent.width - 38px;
    y: 8px;
    width: 30px;
    height: 30px;
    mouse-cursor: pointer;
    clicked => {
        AppState.request-delete-thumbnail(root.item.id, "generation");
    }
}
Rectangle {
    visible: failed-hover.has-hover || failed-delete-touch.has-hover;
    x: parent.width - 38px;
    y: 8px;
    width: 30px;
    height: 30px;
    background: failed-delete-touch.pressed ? AppTheme.danger : #000000b8;
    border-radius: AppState.card-style == "rounded" ? 6px : 0px;
    Image {
        x: 7px;
        y: 7px;
        width: 16px;
        height: 16px;
        source: @image-url("../../assets/icons/trash.svg");
        colorize: #ffffff;
        image-fit: contain;
    }
}
```

Place the delete hit region above the background hover area in input priority while keeping it separate from the centered retry button. Do not change `can-drag-preview()`, the successful-card `hover` handler, or viewer opening behavior.

- [ ] **Step 4: Run the focused test and Slint compilation check**

Run:

```bash
cargo test -p artforge-studio-native failed_generation_thumbnail_hover_requests_confirmed_delete
cargo check -p artforge-studio-native
```

Expected: the focused test passes and the active client compiles without Slint errors.

- [ ] **Step 5: Run the complete client verification**

Run:

```bash
cargo test -p artforge-studio-native
cargo fmt --all -- --check
git diff --check
```

Expected: all client tests pass, formatting is unchanged, and the diff has no whitespace errors.

- [ ] **Step 6: Commit the implementation**

```bash
git add native-client/src/runtime/tests.rs native-client/ui/components/thumbnail-card.slint
git commit -m "feat: delete failed generation records"
```
