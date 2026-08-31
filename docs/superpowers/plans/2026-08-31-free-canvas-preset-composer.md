# Free Canvas Preset Composer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route all Free Canvas generator cards into the existing infinite canvas with an entry-specific hidden prompt template, one simple-description input, and reference-image upload.

**Architecture:** Store the selected workflow metadata in `AppState`, render a compact quick-generation dock inside `InfiniteCanvasPage`, and create a persisted image source node before invoking the existing canvas generation pipeline. Reuse the current reference-image callbacks and canvas result delivery instead of introducing new server contracts.

**Tech Stack:** Rust, Slint, existing native client generation and canvas persistence layers.

**Spec:** `docs/plans/2026-08-31-free-canvas-preset-composer-design.md`

## Global Constraints

- Preserve the current infinite canvas data model, interactions, and server-facing generation contracts.
- Keep one prompt input and one reference-image upload area in the quick-generation dock.
- Five preset entries enter the same canvas; the plain Infinite Canvas entry starts with a blank preset.
- Preserve existing packaged user data and do not push remote changes.

---

### Task 1: Lock the entry and composer behavior with tests

**Files:**
- Modify: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: existing source-text UI regression tests.
- Produces: `free_canvas_presets_open_the_shared_canvas_composer`.

- [ ] **Step 1: Write the failing test**

Add assertions that generator cards assign `canvas-workflow-id`, `canvas-workflow-title`, and `canvas-workflow-prompt`, that both preset and plain entries navigate to `canvas`, and that the canvas contains the prompt input, reference upload, and source-node generation call.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p artforge-studio-native runtime::tests::free_canvas_presets_open_the_shared_canvas_composer -- --exact --nocapture`

Expected: FAIL because the workflow state and composer do not exist.

### Task 2: Add workflow state and route every card into the existing canvas

**Files:**
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/ui/pages/free-canvas-page.slint`
- Modify: `native-client/src/runtime/callbacks/viewer.rs`

**Interfaces:**
- Produces: `canvas-workflow-id`, `canvas-workflow-title`, `canvas-workflow-prompt`, `canvas-workflow-template`, and `canvas-workflow-hint`.
- Consumes: existing `AppState.navigate("canvas")` and viewer import flow.

- [ ] **Step 1: Add the workflow properties**

Place the properties with the other transient canvas state.

- [ ] **Step 2: Change card clicks**

Preset cards set the workflow properties, category, dark background, dot grid, then navigate to `canvas`. The plain card clears workflow state before navigation.

- [ ] **Step 3: Clear workflow state for viewer imports**

Reset all workflow properties immediately before the viewer import navigates to canvas.

### Task 3: Create a canvas generation source callback

**Files:**
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/src/runtime/callbacks/infinite_canvas.rs`

**Interfaces:**
- Produces: `create-canvas-generation-source(string, float, float) -> string`.
- Consumes: canvas history, persistence, selection sync, and `generate-canvas-node(string, string)`.

- [ ] **Step 1: Declare the callback**

Add a callback returning the persisted source node ID or an empty string on capacity failure.

- [ ] **Step 2: Implement source-node creation**

Create a selected image node centered at the requested world coordinates, store the prompt as its content, record history, persist it, and return its UUID.

### Task 4: Render the quick-generation dock

**Files:**
- Modify: `native-client/ui/pages/infinite-canvas-page.slint`

**Interfaces:**
- Consumes: workflow state, `AppState.references`, reference callbacks, source-node callback, and `generate-canvas-node`.

- [ ] **Step 1: Add the single prompt editor**

Bind one multiline `TextInput` to `AppState.canvas-workflow-prompt`, show the selected workflow title, and use the workflow-specific hint as its empty-state example. Keep the full template hidden.

- [ ] **Step 2: Add reference upload and thumbnails**

Use `AppState.add-reference`, `open-reference`, and `remove-reference`; cap display at the existing eight-reference limit.

- [ ] **Step 3: Connect generation**

Create a source node at the visible canvas center, automatically combine the hidden template with the user's concise prompt, then call `generate-canvas-node` with the returned ID and composed prompt. Show current generation status and disable generation for empty prompts or missing models.

- [ ] **Step 4: Reposition existing controls**

Move zoom and toolbar controls above the dock so no controls overlap.

### Task 5: Verify and package

**Files:**
- Test: `native-client/src/runtime/tests.rs`
- Package: `dist/ElunviCanvas-windows-x64`

**Interfaces:**
- Consumes: all tasks above.
- Produces: verified Windows portable package.

- [ ] **Step 1: Run the focused test and `cargo check`**

- [ ] **Step 2: Run the full native-client test suite with `SLINT_EMIT_DEBUG_INFO=1`**

- [ ] **Step 3: Build the Windows package while preserving `data`**

- [ ] **Step 4: Open the packaged client and visually verify the five preset entry flows and plain canvas flow**

- [ ] **Step 5: Commit locally without pushing**
