# Studio Content Top Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the generation workbench controls top-aligned in tall client windows for character, scene, UI, and effect categories.

**Architecture:** Preserve the existing shared `StudioWorkPanel` and its scrolling behavior. Add one real Slint rendering regression test covering all four asset categories, then explicitly anchor the scroll content at the top so the shared fix applies to every category.

**Tech Stack:** Rust 2021, Slint 1.17.1, `i_slint_backend_testing`, Cargo.

**Spec:** Screenshot-backed bug report in the current 2026-08-25 task; no separate specification document.

## Global Constraints

- Modify only the active client under `native-client/`; do not touch the historical root `ui/` tree.
- Preserve the current scrollable workbench and its content height calculations.
- Cover `character`, `scene`, `ui`, and `effect` with the same regression test.
- Preserve unrelated uncommitted video model work.

---

### Task 1: Top-align shared workbench scroll content

**Files:**
- Modify: `native-client/src/runtime/tests.rs`
- Modify: `native-client/ui/components/studio-work-panel.slint`

**Interfaces:**
- Consumes: `AppWindow`, `AppState.asset-type`, and the existing `PromptComposer` Slint component.
- Produces: A rendered-geometry contract that `PromptComposer.absolute_position().y` remains below `200.0` in a `1351 × 1335` window for all four generation categories.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn studio_content_stays_top_aligned_in_tall_windows_for_every_category() {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("create app window");
    let state = app.global::<AppState>();

    state.set_logged_in(true);
    state.set_page("generation".into());
    app.window().set_size(slint::LogicalSize::new(1351.0, 1335.0));
    app.show().expect("show app window");

    for category in ["character", "scene", "ui", "effect"] {
        state.set_asset_type(category.into());
        let composers = i_slint_backend_testing::ElementHandle::find_by_element_type_name(
            &app,
            "PromptComposer",
        )
        .collect::<Vec<_>>();
        assert_eq!(composers.len(), 1, "expected one composer for {category}");
        let composer_y = composers[0].absolute_position().y;
        assert!(
            composer_y < 200.0,
            "{category} composer should stay near the workbench header, but started at y={composer_y}"
        );
    }
}
```

- [x] **Step 2: Run the focused test and verify RED**

Run: `SLINT_EMIT_DEBUG_INFO=1 cargo test -p artforge-studio-native runtime::tests::studio_content_stays_top_aligned_in_tall_windows_for_every_category -- --exact --nocapture`

Observed: FAIL with the character composer at `y=288.5` because the unanchored `work-content` is vertically displaced in the tall viewport.

- [x] **Step 3: Implement the minimal fix**

Add `y: 0px;` to `work-content := Rectangle` inside `StudioWorkPanel` without changing any other geometry.

- [x] **Step 4: Run focused and full verification**

Run: `SLINT_EMIT_DEBUG_INFO=1 cargo test -p artforge-studio-native runtime::tests::studio_content_stays_top_aligned_in_tall_windows_for_every_category -- --exact --nocapture`

Observed: PASS for all four categories.

Run: `SLINT_EMIT_DEBUG_INFO=1 cargo test -p artforge-studio-native` outside the filesystem/network sandbox because existing tests bind local loopback sockets and write managed temporary client directories.

Observed: `502 passed; 0 failed; 46 ignored` (the ignored tests require the external dev Mock API server).

- [x] **Step 5: Review the diff**

Run: `git diff --check && git diff -- native-client/src/runtime/tests.rs native-client/ui/components/studio-work-panel.slint docs/superpowers/plans/2026-08-25-studio-content-top-alignment.md`

Expected: No whitespace errors; diff contains only the regression test, the one-line layout fix, and this plan.
