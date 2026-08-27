use super::*;
use std::sync::{Mutex, OnceLock};

const RECOVERY_SCHEMA_VERSION: u32 = 1;

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

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PendingGenerationRecord {
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) created_at_epoch_ms: i64,
    pub(super) client_request_id: String,
    #[serde(default)]
    pub(super) owner_user_id: String,
    #[serde(default)]
    pub(super) auth_epoch: u64,
    pub(super) local_task_id: String,
    #[serde(default)]
    pub(super) server_task_id: String,
    pub(super) raw_prompt: String,
    pub(super) generation_prompt: String,
    #[serde(default)]
    pub(super) task_type: String,
    pub(super) category: String,
    pub(super) mode: String,
    pub(super) ratio: String,
    pub(super) quality: String,
    pub(super) model_code: String,
    pub(super) conversation_id: String,
    pub(super) count: i32,
    #[serde(default)]
    pub(super) target_width: u32,
    #[serde(default)]
    pub(super) target_height: u32,
    pub(super) create_conversation: bool,
    #[serde(default)]
    pub(super) reference_paths: Vec<String>,
    #[serde(default)]
    pub(super) reference_sha256: Vec<String>,
    #[serde(default)]
    pub(super) reference_size_bytes: Vec<u64>,
    #[serde(default)]
    pub(super) lineage_reference_paths: Vec<String>,
    #[serde(default)]
    pub(super) uploaded_file_ids: Vec<String>,
    #[serde(default)]
    pub(super) deliveries: Vec<PendingDeliveryRecord>,
    #[serde(default)]
    pub(super) terminal: bool,
    #[serde(default)]
    pub(super) expected_success_count: usize,
    #[serde(default)]
    pub(super) canvas_source_node_id: String,
    #[serde(default)]
    pub(super) canvas_ui_extraction: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct RecoveryFile {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    generations: Vec<PendingGenerationRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PendingOrderRecord {
    pub(super) schema_version: u32,
    pub(super) kind: String,
    pub(super) client_request_id: String,
    #[serde(default)]
    pub(super) owner_user_id: String,
    #[serde(default)]
    pub(super) auth_epoch: u64,
    #[serde(default)]
    pub(super) order_id: String,
    pub(super) product_code: String,
    #[serde(default)]
    pub(super) upgrade_quote_id: String,
    pub(super) created_at: String,
}

#[derive(Default, Serialize, Deserialize)]
struct OrderRecoveryFile {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    orders: Vec<PendingOrderRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct PendingPromptTaskRecord {
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) created_at_epoch_ms: i64,
    pub(super) client_request_id: String,
    #[serde(default)]
    pub(super) owner_user_id: String,
    #[serde(default)]
    pub(super) auth_epoch: u64,
    #[serde(default)]
    pub(super) server_task_id: String,
    pub(super) task_type: String,
    pub(super) model_code: String,
    pub(super) prompt: String,
    #[serde(default)]
    pub(super) target_language: Option<String>,
    #[serde(default)]
    pub(super) optimize: bool,
    pub(super) target_kind: String,
    #[serde(default)]
    pub(super) target_id: String,
    #[serde(default)]
    pub(super) target_category: String,
    #[serde(default)]
    pub(super) target_input: String,
    #[serde(default)]
    pub(super) append_result: bool,
    #[serde(default)]
    pub(super) activity_kind: String,
    #[serde(default)]
    pub(super) reference_paths: Vec<String>,
    #[serde(default)]
    pub(super) reference_sha256: Vec<String>,
    #[serde(default)]
    pub(super) reference_size_bytes: Vec<u64>,
    #[serde(default)]
    pub(super) uploaded_file_ids: Vec<String>,
    #[serde(default)]
    pub(super) result_prompt: String,
    #[serde(default)]
    pub(super) terminal_error: String,
    #[serde(default)]
    pub(super) applied_to_target: bool,
    #[serde(default)]
    pub(super) result_committed: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct PromptTaskRecoveryFile {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    prompt_tasks: Vec<PendingPromptTaskRecord>,
}

fn recovery_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn generation_recovery_path() -> PathBuf {
    app_data_dir().join("pending-generations.json")
}

pub(super) fn order_recovery_path() -> PathBuf {
    app_data_dir().join("pending-orders.json")
}

pub(super) fn prompt_task_recovery_path() -> PathBuf {
    app_data_dir().join("pending-prompt-tasks.json")
}

pub(super) fn load_pending_orders_checked() -> Result<Vec<PendingOrderRecord>> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    Ok(read_order_recovery_file_for_update_at(&order_recovery_path())?.orders)
}

pub(super) fn upsert_pending_order(record: PendingOrderRecord) -> Result<()> {
    mutate_order_recovery_file(|file| upsert_pending_order_in_memory(file, record))
}

pub(super) fn update_pending_order_id(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    order_id: &str,
) -> Result<()> {
    if order_id.trim().is_empty() {
        return Err(anyhow!("refusing to persist an empty payment order id"));
    }
    mutate_order_recovery_file(|file| {
        update_pending_order_scoped_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            client_request_id,
            |order| order.order_id = order_id.to_string(),
        )
        .then_some(())
        .ok_or_else(|| anyhow!("pending payment order ownership lease no longer matches"))
    })
}

pub(super) fn update_pending_order_quote_id(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    upgrade_quote_id: &str,
) -> Result<()> {
    if upgrade_quote_id.trim().is_empty() {
        return Err(anyhow!(
            "refusing to persist an empty membership upgrade quote id"
        ));
    }
    mutate_order_recovery_file(|file| {
        update_pending_order_scoped_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            client_request_id,
            |order| order.upgrade_quote_id = upgrade_quote_id.to_string(),
        )
        .then_some(())
        .ok_or_else(|| anyhow!("pending payment order ownership lease no longer matches"))
    })
}

pub(super) fn remove_pending_order(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
) -> Result<()> {
    mutate_order_recovery_file(|file| {
        remove_pending_order_scoped_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            client_request_id,
        )
        .then_some(())
        .ok_or_else(|| anyhow!("pending payment order ownership lease no longer matches"))
    })
}

pub(super) fn claim_pending_order_epoch(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    new_auth_epoch: u64,
    client_request_id: &str,
) -> Result<()> {
    if owner_user_id.trim().is_empty() || client_request_id.trim().is_empty() {
        return Err(anyhow!(
            "pending payment order claim requires an owner and request id"
        ));
    }
    mutate_order_recovery_file(|file| {
        claim_pending_order_epoch_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            new_auth_epoch,
            client_request_id,
        )
        .then_some(())
        .ok_or_else(|| anyhow!("pending payment order ownership lease no longer matches"))
    })
}

pub(super) fn claim_legacy_pending_order(
    owner_user_id: &str,
    new_auth_epoch: u64,
    client_request_id: &str,
    order_id: &str,
) -> Result<()> {
    if owner_user_id.trim().is_empty()
        || client_request_id.trim().is_empty()
        || order_id.trim().is_empty()
    {
        return Err(anyhow!(
            "legacy payment order claim requires an owner, request id, and server order id"
        ));
    }
    mutate_order_recovery_file(|file| {
        claim_legacy_pending_order_in_memory(
            file,
            owner_user_id,
            new_auth_epoch,
            client_request_id,
            order_id,
        )
        .then_some(())
        .ok_or_else(|| anyhow!("legacy payment order is missing, ambiguous, or already owned"))
    })
}

fn upsert_pending_order_in_memory(
    file: &mut OrderRecoveryFile,
    record: PendingOrderRecord,
) -> Result<()> {
    if record.owner_user_id.trim().is_empty() || record.client_request_id.trim().is_empty() {
        return Err(anyhow!(
            "pending payment order requires an owner and client request id"
        ));
    }

    let matching_request_indexes = file
        .orders
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            (item.client_request_id == record.client_request_id).then_some(index)
        })
        .collect::<Vec<_>>();
    match matching_request_indexes.as_slice() {
        [] => file.orders.push(record),
        [index]
            if file.orders[*index].owner_user_id == record.owner_user_id
                && file.orders[*index].auth_epoch == record.auth_epoch =>
        {
            file.orders[*index] = record;
        }
        _ => {
            return Err(anyhow!(
                "pending payment order request id is already bound to another ownership lease"
            ));
        }
    }
    Ok(())
}

fn update_pending_order_scoped_in_memory(
    file: &mut OrderRecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    update: impl FnOnce(&mut PendingOrderRecord),
) -> bool {
    if owner_user_id.trim().is_empty() || client_request_id.trim().is_empty() {
        return false;
    }
    let mut matches = file.orders.iter_mut().filter(|record| {
        record.owner_user_id == owner_user_id
            && record.auth_epoch == expected_auth_epoch
            && record.client_request_id == client_request_id
    });
    let Some(record) = matches.next() else {
        return false;
    };
    if matches.next().is_some() {
        return false;
    }
    update(record);
    true
}

fn remove_pending_order_scoped_in_memory(
    file: &mut OrderRecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
) -> bool {
    if owner_user_id.trim().is_empty() || client_request_id.trim().is_empty() {
        return false;
    }
    let matching_indexes = file
        .orders
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            (record.owner_user_id == owner_user_id
                && record.auth_epoch == expected_auth_epoch
                && record.client_request_id == client_request_id)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    file.orders.remove(*index);
    true
}

fn claim_pending_order_epoch_in_memory(
    file: &mut OrderRecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    new_auth_epoch: u64,
    client_request_id: &str,
) -> bool {
    update_pending_order_scoped_in_memory(
        file,
        owner_user_id,
        expected_auth_epoch,
        client_request_id,
        |order| order.auth_epoch = new_auth_epoch,
    )
}

fn claim_legacy_pending_order_in_memory(
    file: &mut OrderRecoveryFile,
    owner_user_id: &str,
    new_auth_epoch: u64,
    client_request_id: &str,
    order_id: &str,
) -> bool {
    if owner_user_id.trim().is_empty()
        || client_request_id.trim().is_empty()
        || order_id.trim().is_empty()
    {
        return false;
    }
    let matching_indexes = file
        .orders
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            (record.owner_user_id.is_empty()
                && record.client_request_id == client_request_id
                && record.order_id == order_id)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    let record = &mut file.orders[*index];
    record.owner_user_id = owner_user_id.to_string();
    record.auth_epoch = new_auth_epoch;
    true
}

pub(super) fn load_generation_recovery_candidates_checked(
    owner_user_id: &str,
    _auth_epoch: u64,
) -> Result<Vec<PendingGenerationRecord>> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    if owner_user_id.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(read_recovery_file_for_update()?
        .generations
        .into_iter()
        .filter(|record| {
            record.owner_user_id == owner_user_id
                || (record.owner_user_id.is_empty() && !record.server_task_id.is_empty())
        })
        .collect())
}

pub(super) fn upsert_pending_generation_scoped(
    record: PendingGenerationRecord,
    owner_user_id: &str,
    auth_epoch: u64,
) -> Result<()> {
    let mut outcome = Ok(());
    mutate_recovery_file(|file| {
        outcome =
            upsert_pending_generation_scoped_in_memory(file, record, owner_user_id, auth_epoch);
    })?;
    outcome
}

fn upsert_pending_generation_scoped_in_memory(
    file: &mut RecoveryFile,
    record: PendingGenerationRecord,
    owner_user_id: &str,
    auth_epoch: u64,
) -> Result<()> {
    if owner_user_id.trim().is_empty()
        || record.client_request_id.trim().is_empty()
        || record.owner_user_id != owner_user_id
        || record.auth_epoch != auth_epoch
    {
        return Err(anyhow!("pending generation scope is invalid"));
    }
    let matching_indexes = file
        .generations
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            (item.client_request_id == record.client_request_id).then_some(index)
        })
        .collect::<Vec<_>>();
    match matching_indexes.as_slice() {
        [] => file.generations.push(record),
        [index]
            if file.generations[*index].owner_user_id == owner_user_id
                && file.generations[*index].auth_epoch == auth_epoch =>
        {
            file.generations[*index] = record;
        }
        _ => {
            return Err(anyhow!(
                "pending generation request id is ambiguous or belongs to another session scope"
            ));
        }
    }
    Ok(())
}

pub(super) fn update_pending_generation_scoped(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    update: impl FnOnce(&mut PendingGenerationRecord),
) -> Result<bool> {
    let mut matched = false;
    mutate_recovery_file(|file| {
        matched = update_generation_scoped_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            client_request_id,
            update,
        );
    })?;
    Ok(matched)
}

fn update_generation_scoped_in_memory(
    file: &mut RecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    update: impl FnOnce(&mut PendingGenerationRecord),
) -> bool {
    if owner_user_id.trim().is_empty() || client_request_id.trim().is_empty() {
        return false;
    }
    let matching_indexes = file
        .generations
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (item.client_request_id == client_request_id).then_some(index))
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    let record = &mut file.generations[*index];
    if record.owner_user_id != owner_user_id || record.auth_epoch != expected_auth_epoch {
        return false;
    }
    update(record);
    if generation_record_complete(record) {
        file.generations.remove(*index);
    }
    true
}

pub(super) fn remove_pending_generation_scoped(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
) -> Result<bool> {
    let mut matched = false;
    mutate_recovery_file(|file| {
        matched = remove_generation_scoped_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            client_request_id,
        );
    })?;
    Ok(matched)
}

fn remove_generation_scoped_in_memory(
    file: &mut RecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
) -> bool {
    if owner_user_id.trim().is_empty() || client_request_id.trim().is_empty() {
        return false;
    }
    let matching_indexes = file
        .generations
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (item.client_request_id == client_request_id).then_some(index))
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    let record = &file.generations[*index];
    if record.owner_user_id != owner_user_id || record.auth_epoch != expected_auth_epoch {
        return false;
    }
    file.generations.remove(*index);
    true
}

pub(super) fn rebind_pending_generation_epoch(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    new_auth_epoch: u64,
    client_request_id: &str,
) -> Result<bool> {
    let mut matched = false;
    mutate_recovery_file(|file| {
        matched = rebind_generation_epoch_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            new_auth_epoch,
            client_request_id,
        );
    })?;
    Ok(matched)
}

fn rebind_generation_epoch_in_memory(
    file: &mut RecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    new_auth_epoch: u64,
    client_request_id: &str,
) -> bool {
    if owner_user_id.trim().is_empty() || client_request_id.trim().is_empty() {
        return false;
    }
    let matching_indexes = file
        .generations
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (item.client_request_id == client_request_id).then_some(index))
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    let record = &mut file.generations[*index];
    if record.owner_user_id != owner_user_id || record.auth_epoch != expected_auth_epoch {
        return false;
    }
    record.auth_epoch = new_auth_epoch;
    true
}

pub(super) fn claim_legacy_pending_generation(
    owner_user_id: &str,
    new_auth_epoch: u64,
    client_request_id: &str,
    server_task_id: &str,
) -> Result<bool> {
    let mut matched = false;
    mutate_recovery_file(|file| {
        matched = claim_legacy_generation_in_memory(
            file,
            owner_user_id,
            new_auth_epoch,
            client_request_id,
            server_task_id,
        );
    })?;
    Ok(matched)
}

fn claim_legacy_generation_in_memory(
    file: &mut RecoveryFile,
    owner_user_id: &str,
    new_auth_epoch: u64,
    client_request_id: &str,
    server_task_id: &str,
) -> bool {
    if owner_user_id.trim().is_empty()
        || client_request_id.trim().is_empty()
        || server_task_id.trim().is_empty()
    {
        return false;
    }
    let matching_indexes = file
        .generations
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            (record.client_request_id == client_request_id).then_some(index)
        })
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    let record = &mut file.generations[*index];
    if !record.owner_user_id.is_empty() || record.server_task_id != server_task_id {
        return false;
    }
    record.owner_user_id = owner_user_id.to_string();
    record.auth_epoch = new_auth_epoch;
    true
}

pub(super) fn load_pending_prompt_tasks() -> Vec<PendingPromptTaskRecord> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    match read_prompt_task_recovery_file_for_update() {
        Ok(file) => file.prompt_tasks,
        Err(error) => {
            eprintln!("failed to read pending prompt task recovery file: {error}");
            Vec::new()
        }
    }
}

pub(super) fn load_pending_prompt_tasks_checked() -> Result<Vec<PendingPromptTaskRecord>> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    Ok(read_prompt_task_recovery_file_for_update()?.prompt_tasks)
}

pub(super) fn upsert_pending_prompt_task(record: PendingPromptTaskRecord) -> Result<()> {
    let mut outcome = Ok(());
    mutate_prompt_task_recovery_file(|file| {
        outcome = upsert_prompt_task_in_memory(file, record);
    })?;
    outcome
}

fn upsert_prompt_task_in_memory(
    file: &mut PromptTaskRecoveryFile,
    record: PendingPromptTaskRecord,
) -> Result<()> {
    if record.owner_user_id.trim().is_empty() || record.client_request_id.trim().is_empty() {
        return Err(anyhow!(
            "pending prompt task requires an owner and client request id"
        ));
    }
    let matching_indexes = file
        .prompt_tasks
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            (item.client_request_id == record.client_request_id).then_some(index)
        })
        .collect::<Vec<_>>();
    match matching_indexes.as_slice() {
        [] => file.prompt_tasks.push(record),
        [index]
            if file.prompt_tasks[*index].owner_user_id == record.owner_user_id
                && file.prompt_tasks[*index].auth_epoch == record.auth_epoch =>
        {
            file.prompt_tasks[*index] = record;
        }
        _ => {
            return Err(anyhow!(
                "pending prompt task request id is already bound to another ownership lease"
            ));
        }
    }
    Ok(())
}

pub(super) fn update_pending_prompt_task_scoped(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    update: impl FnOnce(&mut PendingPromptTaskRecord),
) -> Result<bool> {
    let mut matched = false;
    mutate_prompt_task_recovery_file(|file| {
        matched = update_prompt_task_scoped_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            client_request_id,
            update,
        );
    })?;
    Ok(matched)
}

fn update_prompt_task_scoped_in_memory(
    file: &mut PromptTaskRecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    update: impl FnOnce(&mut PendingPromptTaskRecord),
) -> bool {
    let matching_indexes = file
        .prompt_tasks
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            (item.client_request_id == client_request_id
                && item.owner_user_id == owner_user_id
                && item.auth_epoch == expected_auth_epoch)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    update(&mut file.prompt_tasks[*index]);
    true
}

pub(super) fn remove_pending_prompt_task_scoped(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
) -> Result<bool> {
    let mut matched = false;
    mutate_prompt_task_recovery_file(|file| {
        matched = remove_prompt_task_scoped_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            client_request_id,
        );
    })?;
    Ok(matched)
}

fn remove_prompt_task_scoped_in_memory(
    file: &mut PromptTaskRecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
) -> bool {
    let matching_indexes = file
        .prompt_tasks
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            (item.client_request_id == client_request_id
                && item.owner_user_id == owner_user_id
                && item.auth_epoch == expected_auth_epoch)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    file.prompt_tasks.remove(*index);
    true
}

pub(super) fn pending_delivery_saved(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    delivery: &DeliveryConfirmation,
    local_path: &str,
) -> Result<bool> {
    update_pending_generation_scoped(
        owner_user_id,
        expected_auth_epoch,
        client_request_id,
        |record| {
            if let Some(item) = record
                .deliveries
                .iter_mut()
                .find(|item| item.file_id == delivery.file_id)
            {
                item.local_path = local_path.to_string();
            } else {
                record.deliveries.push(PendingDeliveryRecord {
                    item_index: delivery.item_index,
                    file_id: delivery.file_id.clone(),
                    sha256: delivery.sha256.clone(),
                    size_bytes: delivery.size_bytes,
                    local_path: local_path.to_string(),
                    acknowledged: false,
                    failed_asset_id: String::new(),
                    abandoned: false,
                });
            }
        },
    )
}

pub(super) fn pending_delivery_failed(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    delivery: &DeliveryConfirmation,
    failed_asset_id: &str,
) -> Result<bool> {
    let mut matched = false;
    mutate_recovery_file(|file| {
        matched = pending_delivery_failed_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            client_request_id,
            delivery,
            failed_asset_id,
        );
    })?;
    Ok(matched)
}

fn pending_delivery_failed_in_memory(
    file: &mut RecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    delivery: &DeliveryConfirmation,
    failed_asset_id: &str,
) -> bool {
    if owner_user_id.trim().is_empty() || failed_asset_id.trim().is_empty() {
        return false;
    }
    update_generation_scoped_in_memory(
        file,
        owner_user_id,
        expected_auth_epoch,
        client_request_id,
        |record| {
            if let Some(item) = record
                .deliveries
                .iter_mut()
                .find(|item| item.file_id == delivery.file_id)
            {
                item.failed_asset_id = failed_asset_id.to_string();
            } else {
                record.deliveries.push(PendingDeliveryRecord {
                    item_index: delivery.item_index,
                    file_id: delivery.file_id.clone(),
                    sha256: delivery.sha256.clone(),
                    size_bytes: delivery.size_bytes,
                    local_path: String::new(),
                    acknowledged: false,
                    failed_asset_id: failed_asset_id.to_string(),
                    abandoned: false,
                });
            }
        },
    )
}

pub(super) fn recoverable_delivery_for_failed_asset(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    failed_asset_id: &str,
) -> Result<Option<(PendingGenerationRecord, PendingDeliveryRecord)>> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    let file = read_recovery_file_for_update()?;
    Ok(recoverable_delivery_for_failed_asset_in_memory(
        &file,
        owner_user_id,
        expected_auth_epoch,
        failed_asset_id,
    ))
}

fn recoverable_delivery_for_failed_asset_in_memory(
    file: &RecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    failed_asset_id: &str,
) -> Option<(PendingGenerationRecord, PendingDeliveryRecord)> {
    if owner_user_id.trim().is_empty() || failed_asset_id.trim().is_empty() {
        return None;
    }
    let mut matches = file.generations.iter().flat_map(|record| {
        record
            .deliveries
            .iter()
            .filter(move |delivery| {
                record.owner_user_id == owner_user_id
                    && record.auth_epoch == expected_auth_epoch
                    && delivery.failed_asset_id == failed_asset_id
                    && !delivery.acknowledged
                    && !delivery.abandoned
            })
            .map(move |delivery| (record.clone(), delivery.clone()))
    });
    let result = matches.next();
    if matches.next().is_some() {
        None
    } else {
        result
    }
}

pub(super) fn recoverable_failed_asset_ids(
    owner_user_id: &str,
    expected_auth_epoch: u64,
) -> Result<BTreeSet<String>> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    let file = read_recovery_file_for_update()?;
    Ok(recoverable_failed_asset_ids_in_memory(
        &file,
        owner_user_id,
        expected_auth_epoch,
    ))
}

fn recoverable_failed_asset_ids_in_memory(
    file: &RecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
) -> BTreeSet<String> {
    if owner_user_id.trim().is_empty() {
        return BTreeSet::new();
    }
    file.generations
        .iter()
        .filter(|record| {
            record.owner_user_id == owner_user_id && record.auth_epoch == expected_auth_epoch
        })
        .flat_map(|record| record.deliveries.iter())
        .filter(|delivery| {
            !delivery.failed_asset_id.trim().is_empty()
                && !delivery.acknowledged
                && !delivery.abandoned
        })
        .map(|delivery| delivery.failed_asset_id.clone())
        .collect()
}

pub(super) fn abandon_pending_delivery(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    failed_asset_id: &str,
) -> Result<bool> {
    let mut matched = false;
    mutate_recovery_file(|file| {
        matched = abandon_pending_delivery_in_memory(
            file,
            owner_user_id,
            expected_auth_epoch,
            failed_asset_id,
        );
    })?;
    Ok(matched)
}

fn abandon_pending_delivery_in_memory(
    file: &mut RecoveryFile,
    owner_user_id: &str,
    expected_auth_epoch: u64,
    failed_asset_id: &str,
) -> bool {
    if owner_user_id.trim().is_empty() || failed_asset_id.trim().is_empty() {
        return false;
    }
    let matching_indexes = file
        .generations
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            (record.owner_user_id == owner_user_id
                && record.auth_epoch == expected_auth_epoch
                && record
                    .deliveries
                    .iter()
                    .any(|delivery| {
                        delivery.failed_asset_id == failed_asset_id && !delivery.acknowledged
                    }))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let [index] = matching_indexes.as_slice() else {
        return false;
    };
    for delivery in &mut file.generations[*index].deliveries {
        if delivery.failed_asset_id == failed_asset_id && !delivery.acknowledged {
            delivery.abandoned = true;
        }
    }
    if generation_record_complete(&file.generations[*index]) {
        file.generations.remove(*index);
    }
    true
}

pub(super) fn pending_delivery_acknowledged(
    owner_user_id: &str,
    expected_auth_epoch: u64,
    client_request_id: &str,
    file_id: &str,
) -> Result<bool> {
    update_pending_generation_scoped(
        owner_user_id,
        expected_auth_epoch,
        client_request_id,
        |record| {
            if let Some(item) = record
                .deliveries
                .iter_mut()
                .find(|item| item.file_id == file_id)
            {
                item.acknowledged = true;
            }
        },
    )
}

fn mutate_recovery_file(update: impl FnOnce(&mut RecoveryFile)) -> Result<()> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    mutate_recovery_file_at(&generation_recovery_path(), update)
}

fn mutate_recovery_file_at(path: &Path, update: impl FnOnce(&mut RecoveryFile)) -> Result<()> {
    let mut file = read_recovery_file_for_update_at(path)?;
    file.schema_version = RECOVERY_SCHEMA_VERSION;
    update(&mut file);
    write_recovery_file_at(path, &file)
}

fn read_recovery_file_for_update() -> Result<RecoveryFile> {
    let path = generation_recovery_path();
    read_recovery_file_for_update_at(&path)
}

fn read_recovery_file_for_update_at(path: &Path) -> Result<RecoveryFile> {
    restore_json_backup_if_needed(path);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecoveryFile::default());
        }
        Err(error) => return Err(error.into()),
    };
    serde_json::from_str(&text).context("pending generation recovery file is invalid")
}

fn write_recovery_file_at(path: &Path, file: &RecoveryFile) -> Result<()> {
    let text = serde_json::to_string_pretty(file)?;
    replace_json_file(path, &text)?;
    Ok(())
}

fn mutate_order_recovery_file(
    update: impl FnOnce(&mut OrderRecoveryFile) -> Result<()>,
) -> Result<()> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    mutate_order_recovery_file_at(&order_recovery_path(), update)
}

fn mutate_order_recovery_file_at(
    path: &Path,
    update: impl FnOnce(&mut OrderRecoveryFile) -> Result<()>,
) -> Result<()> {
    let mut file = read_order_recovery_file_for_update_at(path)?;
    file.schema_version = RECOVERY_SCHEMA_VERSION;
    update(&mut file)?;
    let text = serde_json::to_string_pretty(&file)?;
    replace_json_file(path, &text)?;
    Ok(())
}

fn read_order_recovery_file_for_update_at(path: &Path) -> Result<OrderRecoveryFile> {
    restore_json_backup_if_needed(path);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OrderRecoveryFile::default());
        }
        Err(error) => return Err(error.into()),
    };
    serde_json::from_str(&text).context("pending payment order recovery file is invalid")
}

fn mutate_prompt_task_recovery_file(
    update: impl FnOnce(&mut PromptTaskRecoveryFile),
) -> Result<()> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    mutate_prompt_task_recovery_file_at(&prompt_task_recovery_path(), update)
}

fn mutate_prompt_task_recovery_file_at(
    path: &Path,
    update: impl FnOnce(&mut PromptTaskRecoveryFile),
) -> Result<()> {
    let mut file = read_prompt_task_recovery_file_for_update_at(path)?;
    file.schema_version = RECOVERY_SCHEMA_VERSION;
    update(&mut file);
    let text = serde_json::to_string_pretty(&file)?;
    replace_json_file(path, &text)?;
    Ok(())
}

fn read_prompt_task_recovery_file_for_update() -> Result<PromptTaskRecoveryFile> {
    read_prompt_task_recovery_file_for_update_at(&prompt_task_recovery_path())
}

fn read_prompt_task_recovery_file_for_update_at(path: &Path) -> Result<PromptTaskRecoveryFile> {
    restore_json_backup_if_needed(path);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PromptTaskRecoveryFile::default());
        }
        Err(error) => return Err(error.into()),
    };
    serde_json::from_str(&text).context("pending prompt task recovery file is invalid")
}

pub(super) fn pending_recovery_file_references() -> Result<Vec<(String, String, String)>> {
    let _guard = recovery_lock()
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    let generations = read_recovery_file_for_update()?;
    let prompt_tasks = read_prompt_task_recovery_file_for_update()?;
    let mut references = Vec::new();
    for record in generations.generations {
        for path in record
            .reference_paths
            .iter()
            .chain(record.lineage_reference_paths.iter())
        {
            references.push((
                "pending-generation".to_string(),
                record.client_request_id.clone(),
                path.clone(),
            ));
        }
        for delivery in record.deliveries {
            if !delivery.local_path.trim().is_empty() {
                references.push((
                    "pending-delivery".to_string(),
                    record.client_request_id.clone(),
                    delivery.local_path,
                ));
            }
        }
    }
    for record in prompt_tasks.prompt_tasks {
        for path in record.reference_paths {
            references.push((
                "pending-prompt-task".to_string(),
                record.client_request_id.clone(),
                path,
            ));
        }
    }
    Ok(references)
}

pub(super) fn path_is_referenced_by_pending_recovery(path: &Path) -> bool {
    match pending_recovery_file_references() {
        Ok(references) => references
            .iter()
            .any(|(_, _, candidate)| paths_refer_to_same_file(Path::new(candidate), path)),
        // Recovery data is the authority for interrupted billable tasks. If it cannot be
        // parsed, physical deletion must stop until recovery can be inspected safely.
        Err(_) => true,
    }
}

fn prune_completed(file: &mut RecoveryFile) {
    file.generations
        .retain(|record| !generation_record_complete(record));
}

fn generation_record_complete(record: &PendingGenerationRecord) -> bool {
    record.terminal
        && record
            .deliveries
            .iter()
            .filter(|item| item.acknowledged || item.abandoned)
            .count()
            >= record.expected_success_count
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn failed_delivery_is_upserted_once_and_recovery_is_scope_bound() {
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![PendingGenerationRecord {
                deliveries: Vec::new(),
                ..pending_record()
            }],
        };
        let delivery = DeliveryConfirmation {
            client_request_id: "request_123".to_string(),
            item_index: 0,
            task_id: "server".to_string(),
            file_id: "file-1".to_string(),
            sha256: "abc".to_string(),
            size_bytes: 3,
        };

        assert!(pending_delivery_failed_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
            &delivery,
            "asset-1",
        ));
        assert!(pending_delivery_failed_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
            &delivery,
            "asset-1",
        ));
        assert_eq!(file.generations[0].deliveries.len(), 1);
        assert_eq!(file.generations[0].deliveries[0].failed_asset_id, "asset-1");
        assert!(recoverable_delivery_for_failed_asset_in_memory(
            &file,
            "user-a",
            9,
            "asset-1",
        )
        .is_some());
        assert!(recoverable_delivery_for_failed_asset_in_memory(
            &file,
            "user-b",
            9,
            "asset-1",
        )
        .is_none());
        assert!(recoverable_delivery_for_failed_asset_in_memory(
            &file,
            "user-a",
            10,
            "asset-1",
        )
        .is_none());
    }

    fn pending_record() -> PendingGenerationRecord {
        PendingGenerationRecord {
            schema_version: 1,
            created_at_epoch_ms: Local::now().timestamp_millis(),
            client_request_id: "request_123".to_string(),
            owner_user_id: "user-a".to_string(),
            auth_epoch: 9,
            local_task_id: "local".to_string(),
            server_task_id: "server".to_string(),
            raw_prompt: "prompt".to_string(),
            generation_prompt: "prompt".to_string(),
            task_type: "image_generation".to_string(),
            category: "character".to_string(),
            mode: "game".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model_code: "openai_image".to_string(),
            conversation_id: "conversation".to_string(),
            count: 1,
            target_width: 0,
            target_height: 0,
            create_conversation: true,
            reference_paths: vec![],
            reference_sha256: vec![],
            reference_size_bytes: vec![],
            lineage_reference_paths: vec![],
            uploaded_file_ids: vec![],
            deliveries: vec![PendingDeliveryRecord {
                acknowledged: false,
                ..PendingDeliveryRecord::default()
            }],
            terminal: true,
            expected_success_count: 1,
            canvas_source_node_id: String::new(),
            canvas_ui_extraction: false,
        }
    }

    fn pending_order_record(owner_user_id: &str, auth_epoch: u64) -> PendingOrderRecord {
        PendingOrderRecord {
            schema_version: 1,
            kind: "membership_upgrade".to_string(),
            client_request_id: "payment-request".to_string(),
            owner_user_id: owner_user_id.to_string(),
            auth_epoch,
            order_id: String::new(),
            product_code: "pro-yearly".to_string(),
            upgrade_quote_id: String::new(),
            created_at: "2026-08-10T00:00:00+08:00".to_string(),
        }
    }

    fn pending_prompt_record() -> PendingPromptTaskRecord {
        PendingPromptTaskRecord {
            schema_version: 1,
            created_at_epoch_ms: 1,
            client_request_id: "prompt-request".to_string(),
            owner_user_id: "user-a".to_string(),
            auth_epoch: 9,
            server_task_id: String::new(),
            task_type: "prompt_optimize".to_string(),
            model_code: "prompt-model".to_string(),
            prompt: "draft".to_string(),
            target_language: None,
            optimize: true,
            target_kind: "composer".to_string(),
            target_id: String::new(),
            target_category: "character".to_string(),
            target_input: "draft".to_string(),
            append_result: false,
            activity_kind: "optimize".to_string(),
            reference_paths: Vec::new(),
            reference_sha256: Vec::new(),
            reference_size_bytes: Vec::new(),
            uploaded_file_ids: Vec::new(),
            result_prompt: String::new(),
            terminal_error: String::new(),
            applied_to_target: false,
            result_committed: false,
        }
    }

    #[test]
    fn corrupt_prompt_recovery_file_is_never_overwritten_by_a_mutation() {
        let directory = std::env::temp_dir().join(format!(
            "artforge-corrupt-prompt-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending-prompt-tasks.json");
        let corrupt_bytes = br#"{"prompt_tasks":[{"client_request_id":"paid-task"}"#;
        fs::write(&path, corrupt_bytes).unwrap();

        let result = mutate_prompt_task_recovery_file_at(&path, |file| {
            file.prompt_tasks.push(pending_prompt_record());
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap().as_slice(), corrupt_bytes);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn corrupt_generation_recovery_file_fails_closed_and_is_never_overwritten() {
        let directory = std::env::temp_dir().join(format!(
            "artforge-corrupt-generation-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending-generations.json");
        let corrupt_bytes = br#"{"generations":[{"client_request_id":"paid-task"}"#;
        fs::write(&path, corrupt_bytes).unwrap();

        assert!(read_recovery_file_for_update_at(&path).is_err());
        let result = mutate_recovery_file_at(&path, |file| {
            file.generations.push(pending_record());
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), corrupt_bytes);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn corrupt_payment_recovery_file_is_never_overwritten_by_a_mutation() {
        let directory = std::env::temp_dir().join(format!(
            "artforge-corrupt-payment-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending-orders.json");
        let corrupt_bytes = br#"{"orders":[{"client_request_id":"paid-order"}"#;
        fs::write(&path, corrupt_bytes).unwrap();

        let result = mutate_order_recovery_file_at(&path, |file| {
            file.orders.push(pending_order_record("user-a", 9));
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), corrupt_bytes);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn generation_recovery_file_can_replace_an_existing_snapshot() {
        let directory = std::env::temp_dir().join(format!(
            "artforge-replace-generation-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending-generations.json");
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![pending_record()],
        };

        write_recovery_file_at(&path, &file).unwrap();
        file.generations[0].server_task_id = "server-updated".to_string();
        write_recovery_file_at(&path, &file).unwrap();

        let restored: RecoveryFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored.generations.len(), 1);
        assert_eq!(restored.generations[0].server_task_id, "server-updated");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn payment_recovery_file_can_replace_an_existing_snapshot() {
        let directory = std::env::temp_dir().join(format!(
            "artforge-replace-payment-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending-orders.json");

        mutate_order_recovery_file_at(&path, |file| {
            file.orders.push(pending_order_record("user-a", 9));
            Ok(())
        })
        .unwrap();
        mutate_order_recovery_file_at(&path, |file| {
            file.orders[0].order_id = "server-order".to_string();
            Ok(())
        })
        .unwrap();

        let restored: OrderRecoveryFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored.orders.len(), 1);
        assert_eq!(restored.orders[0].order_id, "server-order");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn terminal_record_is_complete_only_after_every_success_is_acknowledged() {
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![pending_record()],
        };
        prune_completed(&mut file);
        assert_eq!(file.generations.len(), 1);
        file.generations[0].deliveries[0].acknowledged = true;
        prune_completed(&mut file);
        assert!(file.generations.is_empty());
    }

    #[test]
    fn partial_success_recovery_waits_for_every_delivery_ack() {
        let mut record = pending_record();
        record.count = 4;
        record.expected_success_count = 2;
        record.deliveries = vec![
            PendingDeliveryRecord {
                file_id: "file-1".to_string(),
                acknowledged: true,
                ..PendingDeliveryRecord::default()
            },
            PendingDeliveryRecord {
                file_id: "file-2".to_string(),
                acknowledged: false,
                ..PendingDeliveryRecord::default()
            },
        ];
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![record],
        };

        prune_completed(&mut file);
        assert_eq!(file.generations.len(), 1);
        file.generations[0].deliveries[1].acknowledged = true;
        prune_completed(&mut file);
        assert!(file.generations.is_empty());
    }

    #[test]
    fn generation_updates_are_isolated_between_accounts() {
        let record_a = pending_record();
        let mut record_b = pending_record();
        record_b.client_request_id = "request_b".to_string();
        record_b.owner_user_id = "user-b".to_string();
        record_b.auth_epoch = 4;
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![record_a, record_b],
        };

        assert!(update_generation_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
            |record| record.raw_prompt = "updated-a".to_string(),
        ));
        assert!(!update_generation_scoped_in_memory(
            &mut file,
            "user-b",
            4,
            "request_123",
            |record| record.raw_prompt = "wrong-account".to_string(),
        ));
        assert_eq!(file.generations[0].raw_prompt, "updated-a");
        assert_eq!(file.generations[1].raw_prompt, "prompt");
    }

    #[test]
    fn same_account_relogin_rebinds_generation_and_rejects_stale_worker() {
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![pending_record()],
        };

        assert!(rebind_generation_epoch_in_memory(
            &mut file,
            "user-a",
            9,
            10,
            "request_123",
        ));
        assert!(!update_generation_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
            |record| record.server_task_id = "stale-task".to_string(),
        ));
        assert!(!remove_generation_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
        ));
        assert_eq!(file.generations[0].auth_epoch, 10);
        assert_eq!(file.generations[0].server_task_id, "server");
    }

    #[test]
    fn legacy_generation_requires_a_server_task_before_claim() {
        let mut legacy = pending_record();
        legacy.owner_user_id.clear();
        legacy.auth_epoch = 0;
        legacy.server_task_id.clear();
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![legacy],
        };

        assert!(!claim_legacy_generation_in_memory(
            &mut file,
            "user-a",
            10,
            "request_123",
            "",
        ));
        assert!(file.generations[0].owner_user_id.is_empty());

        file.generations[0].server_task_id = "server-verified".to_string();
        assert!(claim_legacy_generation_in_memory(
            &mut file,
            "user-a",
            10,
            "request_123",
            "server-verified",
        ));
        assert_eq!(file.generations[0].owner_user_id, "user-a");
        assert_eq!(file.generations[0].auth_epoch, 10);
    }

    #[test]
    fn ambiguous_generation_request_ids_fail_closed() {
        let first = pending_record();
        let mut duplicate = pending_record();
        duplicate.owner_user_id = "user-b".to_string();
        duplicate.auth_epoch = 2;
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![first, duplicate],
        };

        assert!(!update_generation_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
            |_| {},
        ));
        assert!(!remove_generation_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
        ));
        assert!(!rebind_generation_epoch_in_memory(
            &mut file,
            "user-a",
            9,
            10,
            "request_123",
        ));
        assert!(upsert_pending_generation_scoped_in_memory(
            &mut file,
            pending_record(),
            "user-a",
            9,
        )
        .is_err());
        assert_eq!(file.generations.len(), 2);
    }

    #[test]
    fn terminal_generation_is_retained_until_scoped_delivery_ack() {
        let mut file = RecoveryFile {
            schema_version: 1,
            generations: vec![pending_record()],
        };

        assert!(update_generation_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
            |_| {},
        ));
        assert_eq!(file.generations.len(), 1);
        assert!(!update_generation_scoped_in_memory(
            &mut file,
            "user-b",
            9,
            "request_123",
            |record| record.deliveries[0].acknowledged = true,
        ));
        assert_eq!(file.generations.len(), 1);
        assert!(update_generation_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "request_123",
            |record| record.deliveries[0].acknowledged = true,
        ));
        assert!(file.generations.is_empty());
    }

    #[test]
    fn legacy_payment_order_defaults_to_unowned_without_a_quote() {
        let file: OrderRecoveryFile = serde_json::from_str(
            r#"{
                "schema_version": 1,
                "orders": [{
                    "schema_version": 1,
                    "kind": "credit",
                    "client_request_id": "legacy-request",
                    "order_id": "server-order",
                    "product_code": "credits-100",
                    "created_at": "2026-08-10T00:00:00+08:00"
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(file.orders.len(), 1);
        assert!(file.orders[0].owner_user_id.is_empty());
        assert_eq!(file.orders[0].auth_epoch, 0);
        assert!(file.orders[0].upgrade_quote_id.is_empty());
    }

    #[test]
    fn same_account_relogin_rebinds_order_and_rejects_the_old_worker() {
        let mut file = OrderRecoveryFile {
            schema_version: 1,
            orders: vec![pending_order_record("user-a", 9)],
        };

        assert!(claim_pending_order_epoch_in_memory(
            &mut file,
            "user-a",
            9,
            10,
            "payment-request",
        ));
        assert!(!update_pending_order_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "payment-request",
            |record| record.order_id = "stale-order".to_string(),
        ));
        assert!(!remove_pending_order_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "payment-request",
        ));
        assert!(file.orders[0].order_id.is_empty());
        assert_eq!(file.orders[0].auth_epoch, 10);
    }

    #[test]
    fn current_payment_lease_can_persist_quote_and_order_ids() {
        let mut file = OrderRecoveryFile {
            schema_version: 1,
            orders: vec![pending_order_record("user-a", 10)],
        };

        assert!(update_pending_order_scoped_in_memory(
            &mut file,
            "user-a",
            10,
            "payment-request",
            |record| record.upgrade_quote_id = "quote-1".to_string(),
        ));
        assert!(update_pending_order_scoped_in_memory(
            &mut file,
            "user-a",
            10,
            "payment-request",
            |record| record.order_id = "order-1".to_string(),
        ));
        assert_eq!(file.orders[0].upgrade_quote_id, "quote-1");
        assert_eq!(file.orders[0].order_id, "order-1");
    }

    #[test]
    fn foreign_account_cannot_update_or_remove_a_payment_order() {
        let mut file = OrderRecoveryFile {
            schema_version: 1,
            orders: vec![pending_order_record("user-a", 10)],
        };

        assert!(!update_pending_order_scoped_in_memory(
            &mut file,
            "user-b",
            10,
            "payment-request",
            |record| record.order_id = "foreign-order".to_string(),
        ));
        assert!(!remove_pending_order_scoped_in_memory(
            &mut file,
            "user-b",
            10,
            "payment-request",
        ));
        assert_eq!(file.orders.len(), 1);
        assert!(file.orders[0].order_id.is_empty());
    }

    #[test]
    fn payment_upsert_cannot_overwrite_another_ownership_lease() {
        let mut file = OrderRecoveryFile {
            schema_version: 1,
            orders: vec![pending_order_record("user-a", 10)],
        };
        let mut conflicting = pending_order_record("user-b", 10);
        conflicting.order_id = "foreign-order".to_string();

        assert!(upsert_pending_order_in_memory(&mut file, conflicting).is_err());
        assert_eq!(file.orders.len(), 1);
        assert_eq!(file.orders[0].owner_user_id, "user-a");
        assert!(file.orders[0].order_id.is_empty());
    }

    #[test]
    fn legacy_payment_order_requires_a_server_id_before_claiming() {
        let mut legacy = pending_order_record("", 0);
        legacy.client_request_id = "legacy-request".to_string();
        let mut file = OrderRecoveryFile {
            schema_version: 1,
            orders: vec![legacy],
        };

        assert!(!claim_legacy_pending_order_in_memory(
            &mut file,
            "user-a",
            10,
            "legacy-request",
            "",
        ));
        file.orders[0].order_id = "server-order".to_string();
        assert!(claim_legacy_pending_order_in_memory(
            &mut file,
            "user-a",
            10,
            "legacy-request",
            "server-order",
        ));
        assert_eq!(file.orders[0].owner_user_id, "user-a");
        assert_eq!(file.orders[0].auth_epoch, 10);
    }

    #[test]
    fn stale_prompt_worker_cannot_update_a_rebound_record() {
        let mut file = PromptTaskRecoveryFile {
            schema_version: 1,
            prompt_tasks: vec![pending_prompt_record()],
        };
        file.prompt_tasks[0].auth_epoch = 10;

        let updated = update_prompt_task_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "prompt-request",
            |record| record.result_prompt = "stale result".to_string(),
        );

        assert!(!updated);
        assert!(file.prompt_tasks[0].result_prompt.is_empty());
    }

    #[test]
    fn stale_prompt_worker_cannot_remove_a_rebound_record() {
        let mut file = PromptTaskRecoveryFile {
            schema_version: 1,
            prompt_tasks: vec![pending_prompt_record()],
        };
        file.prompt_tasks[0].auth_epoch = 10;

        let removed = remove_prompt_task_scoped_in_memory(&mut file, "user-a", 9, "prompt-request");

        assert!(!removed);
        assert_eq!(file.prompt_tasks.len(), 1);
    }

    #[test]
    fn prompt_upsert_cannot_overwrite_another_ownership_lease() {
        let mut file = PromptTaskRecoveryFile {
            schema_version: 1,
            prompt_tasks: vec![pending_prompt_record()],
        };
        let mut conflicting = pending_prompt_record();
        conflicting.owner_user_id = "user-b".to_string();
        conflicting.result_prompt = "wrong account".to_string();

        assert!(upsert_prompt_task_in_memory(&mut file, conflicting).is_err());
        assert_eq!(file.prompt_tasks.len(), 1);
        assert_eq!(file.prompt_tasks[0].owner_user_id, "user-a");
        assert!(file.prompt_tasks[0].result_prompt.is_empty());
    }

    #[test]
    fn duplicate_prompt_lease_fails_closed_on_update() {
        let duplicate = pending_prompt_record();
        let mut file = PromptTaskRecoveryFile {
            schema_version: 1,
            prompt_tasks: vec![duplicate.clone(), duplicate],
        };

        let updated = update_prompt_task_scoped_in_memory(
            &mut file,
            "user-a",
            9,
            "prompt-request",
            |record| record.result_prompt = "must not be written".to_string(),
        );

        assert!(!updated);
        assert!(file
            .prompt_tasks
            .iter()
            .all(|record| record.result_prompt.is_empty()));
    }

    #[test]
    fn duplicate_prompt_lease_fails_closed_on_remove() {
        let duplicate = pending_prompt_record();
        let mut file = PromptTaskRecoveryFile {
            schema_version: 1,
            prompt_tasks: vec![duplicate.clone(), duplicate],
        };

        let removed = remove_prompt_task_scoped_in_memory(&mut file, "user-a", 9, "prompt-request");

        assert!(!removed);
        assert_eq!(file.prompt_tasks.len(), 2);
    }
}
