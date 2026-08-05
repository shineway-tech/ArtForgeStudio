# Remove Action Sequence Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove the action-sequence feature, its UI, generation branches, prompt rules, models, routes, assets, and tests without breaking existing user data.

**Architecture:** Delete all active and legacy feature entry points and action-specific generation code. Keep deserialization tolerant by mapping historical action-sequence category strings to a supported category at the local-storage boundary instead of retaining a runnable feature or changing the server API contract.

**Tech Stack:** Rust, Slint, Cargo tests, PowerShell packaging.

---

### Task 1: Add regression coverage for complete removal

**Files:**
- Modify: `native-client/src/runtime/tests.rs`

**Steps:**
1. Add a source-level regression test that scans active native-client UI/runtime sources for removed action-sequence identifiers.
2. Run the targeted test and confirm it fails before implementation.

### Task 2: Remove the active native-client feature

**Files:**
- Modify: `native-client/ui/components/*.slint`
- Modify: `native-client/src/runtime/{app,configuration,model,prompt}.rs`
- Modify: `native-client/src/runtime/generation/backend.rs`
- Modify: `native-client/src/runtime/storage/local_store.rs`

**Steps:**
1. Remove action-only controls, labels, limits, prompt instructions, draft/reference fields, and generation count branches.
2. Normalize historical `action-sequence` values to `character` when loading old local state.
3. Run the targeted native-client test and confirm it passes.

### Task 3: Remove the excluded legacy feature implementation

**Files:**
- Delete: `ui/pages/action-sequence.slint`
- Delete: `assets/icons/action-sequence.svg`
- Modify: legacy `ui`, `crates/artait-model`, `crates/artait-service`, and `crates/artait-app` references.

**Steps:**
1. Remove action-sequence routes, feature variants, task variants, asset domains, callbacks, translations, and sidebar imports.
2. Search the repository and confirm no action-sequence identifiers remain outside this plan and compatibility migration notes.

### Task 4: Verify, commit, package, and restart

**Steps:**
1. Run `cargo fmt --check` and the complete native-client test suite.
2. Commit the removal locally without pushing.
3. Build the release client, preserve the packaged `data` directory, overwrite the Windows directory and portable ZIP, verify hashes, and restart the packaged client.
