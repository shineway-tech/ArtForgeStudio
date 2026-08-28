# Recoverable Generation Download Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user manually re-download a server-succeeded image from ArtForge OSS after client delivery timeout, then save it locally, notify once, and acknowledge delivery so the server deletes the OSS object.

**Architecture:** Classify terminal client download errors separately from provider generation failures and persist their file-to-card association inside the existing account-scoped generation recovery file. A dedicated retry worker re-queries the task for a fresh signed URL, reuses verified atomic download and local persistence, replaces the failed card, then reuses the existing delivery acknowledgement. The server API and schema remain unchanged.

**Tech Stack:** Rust 2021, Slint 1.17.1, blocking reqwest, serde JSON recovery records, existing SQLite local store, existing generation task and delivery-ack APIs.

**Spec:** `docs/superpowers/specs/2026-08-27-recoverable-generation-download-design.md`

## Global Constraints

- Only handle `ArtForge OSS -> client` delivery failures for server task items already marked `succeeded`.
- Do not persist or log signed download URLs.
- Re-downloading must not create a generation task or reserve/deduct credits.
- Never acknowledge delivery before verified local persistence succeeds.
- Keep existing generation retry behavior as a separate, potentially billable action.
- Use the current workspace and existing Cargo cache; do not create another git worktree.
- Do not add a server endpoint or server database migration.

---

### Task 1: Persist Recoverable Delivery Associations

**Files:**
- Modify: `native-client/src/runtime/storage/recovery.rs`
- Modify: `native-client/src/runtime/model.rs`
- Test: `native-client/src/runtime/storage/recovery.rs`

**Interfaces:**
- Consumes: existing `PendingGenerationRecord`, `PendingDeliveryRecord`, `DeliveryConfirmation`, and account-scoped recovery mutation functions.
- Produces:
  - `PendingDeliveryRecord.failed_asset_id: String`
  - `PendingDeliveryRecord.abandoned: bool`
  - `pending_delivery_failed(owner_user_id, auth_epoch, client_request_id, delivery, failed_asset_id) -> Result<bool>`
  - `recoverable_delivery_for_failed_asset(owner_user_id, auth_epoch, failed_asset_id) -> Result<Option<(PendingGenerationRecord, PendingDeliveryRecord)>>`
  - `recoverable_failed_asset_ids(owner_user_id, auth_epoch) -> Result<BTreeSet<String>>`
  - `abandon_pending_delivery(owner_user_id, auth_epoch, failed_asset_id) -> Result<bool>`

- [ ] **Step 1: Write failing recovery tests**

Add tests that construct scoped recovery data and assert the new fields default safely for legacy JSON, a failed delivery is upserted once, lookup rejects another account/auth epoch, and abandonment counts as locally resolved without setting `acknowledged`:

```rust
#[test]
fn legacy_delivery_defaults_to_non_recoverable_and_not_abandoned() {
    let delivery: PendingDeliveryRecord = serde_json::from_value(serde_json::json!({
        "item_index": 0,
        "file_id": "file-1",
        "sha256": "abc",
        "size_bytes": 3,
        "local_path": "",
        "acknowledged": false
    }))
    .unwrap();
    assert!(delivery.failed_asset_id.is_empty());
    assert!(!delivery.abandoned);
}

#[test]
fn abandoned_delivery_resolves_recovery_without_faking_ack() {
    let mut record = pending_record();
    record.terminal = true;
    record.expected_success_count = 1;
    record.deliveries[0].abandoned = true;
    assert!(generation_record_complete(&record));
    assert!(!record.deliveries[0].acknowledged);
}
```

- [ ] **Step 2: Run the focused recovery tests and verify failure**

Run: `cargo test -p artforge-studio-native storage::recovery::tests --lib`

Expected: compilation fails because `failed_asset_id` and `abandoned` do not exist.

- [ ] **Step 3: Add backward-compatible recovery fields and helpers**

Extend the record without changing `RECOVERY_SCHEMA_VERSION` because both fields use serde defaults:

```rust
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct PendingDeliveryRecord {
    pub(super) item_index: usize,
    pub(super) file_id: String,
    pub(super) sha256: String,
    pub(super) size_bytes: u64,
    #[serde(default)]
    pub(super) local_path: String,
    #[serde(default)]
    pub(super) acknowledged: bool,
    #[serde(default)]
    pub(super) failed_asset_id: String,
    #[serde(default)]
    pub(super) abandoned: bool,
}
```

Make `pending_delivery_saved` update an existing delivery without clearing `failed_asset_id`, and make completion count `acknowledged || abandoned`. Implement all lookups through the existing recovery lock and require exact owner plus auth epoch matches.

- [ ] **Step 4: Run the focused recovery tests**

Run: `cargo test -p artforge-studio-native storage::recovery::tests --lib`

Expected: all recovery tests pass.

- [ ] **Step 5: Commit the recovery contract**

```bash
git add native-client/src/runtime/model.rs native-client/src/runtime/storage/recovery.rs
git commit -m "feat: persist recoverable image deliveries"
```

---

### Task 2: Classify Client Delivery Failures and Bind Failed Cards

**Files:**
- Modify: `native-client/src/runtime/model.rs`
- Modify: `native-client/src/runtime/generation/backend.rs`
- Modify: `native-client/src/runtime/generation/poll.rs`
- Modify: `native-client/src/runtime/generation/controller.rs`
- Modify: `native-client/src/runtime/presentation/sync.rs`
- Modify: `native-client/ui/types.slint`
- Modify: every existing `AssetData` constructor under `native-client/src/runtime/`
- Test: `native-client/src/runtime/generation/backend.rs`
- Test: `native-client/src/runtime/tests.rs`

**Interfaces:**
- Consumes: Task 1 recovery helpers and existing `DeliveryConfirmation` metadata.
- Produces:
  - `GenerationOutcome::ImageFailure { reason, time, delivery: Option<DeliveryConfirmation> }`
  - `DeliveryConfirmation.failed_asset_id: Option<String>`
  - `AssetData.delivery_recoverable: bool`
  - `AssetData.delivery_downloading: bool`
  - `AssetItem.delivery-recoverable: bool`
  - `AssetItem.delivery-downloading: bool`
  - `add_stream_failure_item(...) -> String`

- [ ] **Step 1: Write failing classification and view-model tests**

Add a pure helper that builds recoverable delivery metadata only for `succeeded` items with a file, then test that provider/item failures remain ordinary failures. Add source-level assertions that `AssetItem` receives the two delivery state flags:

```rust
#[test]
fn succeeded_item_download_error_keeps_delivery_identity() {
    let detail = completed_task_with_available_file("file-1");
    let delivery = delivery_confirmation_for_item("request-1", &detail, 0).unwrap();
    assert_eq!(delivery.file_id, "file-1");
    assert_eq!(delivery.item_index, 0);
}

#[test]
fn failed_provider_item_has_no_recoverable_delivery() {
    let detail = failed_generation_task("provider_error", "failed");
    assert!(delivery_confirmation_for_item("request-1", &detail, 0).is_none());
}
```

- [ ] **Step 2: Run focused tests and verify failure**

Run: `cargo test -p artforge-studio-native backend_generation::tests --lib`

Expected: compilation fails because the delivery classification helper and view-model fields do not exist.

- [ ] **Step 3: Carry delivery identity through terminal download failure**

For each generation/gallery worker path, change only the terminal `download_verified_to_path_scoped` error branch. Generate one stable failed asset ID, persist it with `pending_delivery_failed`, and send:

```rust
GenerationOutcome::ImageFailure {
    reason: error.generation_message(),
    time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
    delivery: Some(DeliveryConfirmation {
        client_request_id: request_id.clone(),
        item_index: item.index,
        task_id: task_id.clone(),
        file_id: file.id.clone(),
        sha256: file.sha256.clone(),
        size_bytes: file.size_bytes.parse().unwrap_or(0),
        failed_asset_id: Some(failed_asset_id),
    }),
}
```

All provider `failed/cancelled` items and synthetic failures send `delivery: None`. If persisting the association fails, surface a non-recoverable local error and never acknowledge the file.

- [ ] **Step 4: Mark and restore recoverable cards**

Make `add_stream_failure_item` accept the stable ID and set `delivery_recoverable=true`. Add both runtime flags to `AssetData` and Slint `AssetItem`; ordinary constructors set both to `false`. `asset_from_stored` initializes both to `false`, while login recovery reconciles current-account `recoverable_failed_asset_ids` before pushing the gallery.

When a resumed automatic download succeeds, copy `failed_asset_id` from the matching pending delivery into `DeliveryConfirmation` so Task 3 can replace the old card rather than add a duplicate.

- [ ] **Step 5: Run model and generation tests**

Run: `cargo test -p artforge-studio-native backend_generation::tests --lib`

Run: `cargo test -p artforge-studio-native --lib`

Expected: all selected tests pass and every `AssetData`/`AssetItem` constructor compiles.

- [ ] **Step 6: Commit failure classification**

```bash
git add native-client/src/runtime/model.rs native-client/src/runtime/generation native-client/src/runtime/presentation/sync.rs native-client/src/runtime/tests.rs native-client/src/runtime/callbacks native-client/src/runtime/features native-client/src/runtime/storage/local_store.rs native-client/ui/types.slint
git commit -m "feat: distinguish image delivery failures"
```

---

### Task 3: Implement Verified Manual Re-download and Card Replacement

**Files:**
- Create: `native-client/src/runtime/generation/delivery_retry.rs`
- Modify: `native-client/src/runtime/mod.rs`
- Modify: `native-client/src/runtime/model.rs`
- Modify: `native-client/src/runtime/callbacks/generation.rs`
- Modify: `native-client/src/runtime/generation/controller.rs`
- Modify: `native-client/src/runtime/generation/poll.rs`
- Modify: `native-client/src/runtime/storage/local_store.rs`
- Test: `native-client/src/runtime/generation/delivery_retry.rs`
- Test: `native-client/src/runtime/storage/local_store.rs`

**Interfaces:**
- Consumes: `recoverable_delivery_for_failed_asset`, `GenerationApi::task_scoped`, `download_verified_to_path_scoped`, `pending_delivery_saved`, and `acknowledge_delivery_after_local_save`.
- Produces:
  - `retry_failed_delivery(app: &AppWindow, context: AppContext, failed_asset_id: String)`
  - `run_failed_delivery_retry(api: &GenerationApi, scope: &SessionScope, record: &PendingGenerationRecord, delivery: &PendingDeliveryRecord) -> Result<RetrySuccess, DeliveryRetryError>`
  - `select_recoverable_task_file<'a>(detail: &'a GenerationTaskDetail, delivery: &PendingDeliveryRecord) -> Result<&'a TaskOutputFile, DeliveryRetryError>`
  - `replace_failed_delivery_asset_checked(app, store, failed_asset_id, staged_path, time) -> Result<(String, String)>`
  - `replace_failed_delivery_asset_with(store, failed_asset_id, completed_asset, notification, persist) -> Result<()>`
  - `GenerationRegistry.delivery_downloads: RefCell<BTreeSet<String>>`

- [ ] **Step 1: Write failing file-selection and persistence tests**

Test exact item/file matching, expired/deleted results, and replacement rollback:

```rust
#[test]
fn retry_selects_only_the_original_successful_file() {
    let detail = completed_task_with_two_files();
    let pending = delivery("file-2", Path::new(""), b"");
    let selected = select_recoverable_task_file(&detail, &pending).unwrap();
    assert_eq!(selected.id, "file-2");
}

#[test]
fn failed_card_replacement_preserves_position_and_adds_one_notification() {
    let mut store = Store::default();
    store.generations.push(recoverable_failed_asset("failed-1"));
    replace_failed_delivery_asset_with(&mut store, "failed-1", completed_asset("failed-1"), || Ok(())).unwrap();
    assert_eq!(store.generations[0].source_path, "/saved/image.png");
    assert_eq!(store.assets.iter().filter(|item| item.id == "failed-1").count(), 1);
    assert_eq!(store.notifications.len(), 1);
}
```

- [ ] **Step 2: Run focused tests and verify failure**

Run: `cargo test -p artforge-studio-native delivery_retry --lib`

Run: `cargo test -p artforge-studio-native failed_card_replacement --lib`

Expected: compilation fails because the retry module and replacement helper do not exist.

- [ ] **Step 3: Implement the scoped, deduplicated retry worker**

Register `generation/delivery_retry.rs` in `runtime/mod.rs`. The background worker calls `run_failed_delivery_retry`, which must perform the following sequence:

```rust
let scope = current_generation_session_scope(&context).ok_or(AuthenticationRequired)?;
let (record, delivery) = recoverable_delivery_for_failed_asset(
    &scope.owner_user_id,
    scope.auth_epoch,
    &failed_asset_id,
)?.ok_or(DeliveryRetryError::Expired)?;
let detail = api.task_scoped(&record.server_task_id, &scope)?;
let file = select_recoverable_task_file(&detail, &delivery)?;
api.download_verified_to_path_scoped(file, &scope, &staging_path)?;
```

Use `GenerationRegistry.delivery_downloads` keyed by `file_id`; set and clear `AssetData.delivery_downloading` on the UI thread. A second click while the key exists must return without spawning another worker.

- [ ] **Step 4: Persist success before acknowledgement**

On successful verified download:

1. Atomically save to the managed output directory.
2. Replace the failed generation card at its current position and add the same asset to the local asset collection.
3. Add exactly one local notification titled `图片下载完成：<short prompt>` with success state.
4. Call `pending_delivery_saved` with the final local path.
5. Call `acknowledge_delivery_after_local_save`; never call it on an earlier branch.

If the task/file is expired or deleted, call `abandon_pending_delivery`, clear the card's recovery flags, and show `文件已过期，请重新生成`. For transient errors, clear only the loading flag and keep the download icon.

- [ ] **Step 5: Reuse replacement for automatic recovery success**

In the existing `GenerationOutcome::ImageSuccess` handler, when `delivery.failed_asset_id` is present, call `replace_failed_delivery_asset_checked` instead of `add_stream_success_item`. Preserve the same save → recovery record → ack ordering.

- [ ] **Step 6: Run focused delivery and persistence tests**

Run: `cargo test -p artforge-studio-native delivery_retry --lib`

Run: `cargo test -p artforge-studio-native storage::local_store::tests --lib`

Expected: all tests pass, including assertions that an error before local persistence never invokes the acknowledgement hook.

- [ ] **Step 7: Commit manual delivery recovery**

```bash
git add native-client/src/runtime/mod.rs native-client/src/runtime/model.rs native-client/src/runtime/callbacks/generation.rs native-client/src/runtime/generation native-client/src/runtime/storage/local_store.rs
git commit -m "feat: retry failed image deliveries"
```

---

### Task 4: Add Download Icon, Abandon Semantics, and Full Verification

**Files:**
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/ui/components/thumbnail-card.slint`
- Modify: `native-client/src/runtime/callbacks/generation.rs`
- Modify: `native-client/src/runtime/callbacks/viewer.rs`
- Modify: `native-client/src/runtime/generation/controller.rs`
- Test: `native-client/src/runtime/tests.rs`
- Test: `native-client/src/runtime/storage/recovery.rs`

**Interfaces:**
- Consumes: Task 1 abandonment helper and Task 3 `retry_failed_delivery`.
- Produces: Slint callback `retry-generation-delivery(string)` and final recoverable failure-card behavior.

- [ ] **Step 1: Write failing UI contract tests**

Add source-contract tests that require the dedicated callback, icon, loading disablement, distinct copy, and non-overlapping trash placement:

```rust
#[test]
fn recoverable_failure_card_has_independent_download_action() {
    let card = include_str!("../ui/components/thumbnail-card.slint");
    let state = include_str!("../ui/app-state.slint");
    assert!(state.contains("callback retry-generation-delivery(string);"));
    assert!(card.contains("root.item.delivery-recoverable"));
    assert!(card.contains("../../assets/icons/download.svg"));
    assert!(card.contains("AppState.retry-generation-delivery(root.item.id)"));
    assert!(card.contains("root.item.delivery-downloading"));
    assert!(card.contains("图片已生成，下载失败"));
}
```

- [ ] **Step 2: Run the UI contract test and verify failure**

Run: `cargo test -p artforge-studio-native recoverable_failure_card_has_independent_download_action --lib`

Expected: test fails because the callback and icon are absent.

- [ ] **Step 3: Implement the failure-card interaction**

Add the callback to `app-state.slint` and wire it to `retry_failed_delivery`. On recoverable failure cards:

- keep the center `重试`/`Retry` button unchanged;
- show `图片已生成，下载失败` / `Image ready, download failed`;
- show a persistent 30×30 download icon button at the upper right;
- move the hover trash action left when the download action is present;
- disable the download action while `delivery-downloading` is true and expose a visible loading state;
- ordinary failures retain the existing layout and have no download action.

- [ ] **Step 4: Mark recovery abandoned on regenerate or delete**

Before `retry_failed_generation` removes a recoverable card, call `abandon_pending_delivery` for the current session. When deletion confirmation removes a recoverable generation card, perform the same scoped abandonment only after the local store deletion succeeds. Do not send delivery ack; the OSS object remains eligible for existing TTL cleanup.

- [ ] **Step 5: Run all native-client tests and lint checks**

Run: `cargo test -p artforge-studio-native --lib`

Run: `cargo fmt --all -- --check`

Run: `cargo clippy -p artforge-studio-native --lib -- -D warnings`

Expected: all commands exit successfully.

- [ ] **Step 6: Review the final diff for scope and secrets**

Run: `git diff --check`

Run: `git diff --stat de7f5da..HEAD`

Run: `rg -n "download_url|Authorization|Bearer |sk-" native-client/src/runtime/generation native-client/src/runtime/storage/recovery.rs`

Expected: no whitespace errors, no server files changed, and no signed URL or credential persisted/logged by the new flow.

- [ ] **Step 7: Commit the UI and lifecycle integration**

```bash
git add native-client/ui/app-state.slint native-client/ui/components/thumbnail-card.slint native-client/src/runtime/callbacks/generation.rs native-client/src/runtime/callbacks/viewer.rs native-client/src/runtime/generation/controller.rs native-client/src/runtime/tests.rs native-client/src/runtime/storage/recovery.rs
git commit -m "feat: add image delivery recovery action"
```
