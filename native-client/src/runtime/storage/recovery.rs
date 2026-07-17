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
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PendingGenerationRecord {
    pub(super) schema_version: u32,
    pub(super) client_request_id: String,
    pub(super) local_task_id: String,
    #[serde(default)]
    pub(super) server_task_id: String,
    pub(super) raw_prompt: String,
    pub(super) generation_prompt: String,
    pub(super) category: String,
    pub(super) mode: String,
    pub(super) ratio: String,
    pub(super) quality: String,
    pub(super) model_code: String,
    pub(super) conversation_id: String,
    pub(super) count: i32,
    pub(super) create_conversation: bool,
    #[serde(default)]
    pub(super) reference_paths: Vec<String>,
    #[serde(default)]
    pub(super) uploaded_file_ids: Vec<String>,
    #[serde(default)]
    pub(super) deliveries: Vec<PendingDeliveryRecord>,
    #[serde(default)]
    pub(super) terminal: bool,
    #[serde(default)]
    pub(super) expected_success_count: usize,
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
    pub(super) order_id: String,
    pub(super) product_code: String,
    pub(super) created_at: String,
}

#[derive(Default, Serialize, Deserialize)]
struct OrderRecoveryFile {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    orders: Vec<PendingOrderRecord>,
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

pub(super) fn load_pending_orders() -> Vec<PendingOrderRecord> {
    let _guard = recovery_lock().lock().unwrap_or_else(|value| value.into_inner());
    read_order_recovery_file().orders
}

pub(super) fn upsert_pending_order(record: PendingOrderRecord) -> Result<()> {
    mutate_order_recovery_file(|file| {
        if let Some(existing) = file.orders.iter_mut().find(|item| {
            item.client_request_id == record.client_request_id
        }) {
            *existing = record;
        } else {
            file.orders.push(record);
        }
    })
}

pub(super) fn update_pending_order_id(client_request_id: &str, order_id: &str) -> Result<()> {
    mutate_order_recovery_file(|file| {
        if let Some(order) = file.orders.iter_mut().find(|item| {
            item.client_request_id == client_request_id
        }) {
            order.order_id = order_id.to_string();
        }
    })
}

pub(super) fn remove_pending_order(client_request_id: &str) -> Result<()> {
    mutate_order_recovery_file(|file| {
        file.orders.retain(|item| item.client_request_id != client_request_id);
    })
}

pub(super) fn load_pending_generations() -> Vec<PendingGenerationRecord> {
    let _guard = recovery_lock().lock().unwrap_or_else(|value| value.into_inner());
    read_recovery_file().generations
}

pub(super) fn upsert_pending_generation(record: PendingGenerationRecord) -> Result<()> {
    mutate_recovery_file(|file| {
        if let Some(existing) = file.generations.iter_mut().find(|item| {
            item.client_request_id == record.client_request_id
        }) {
            *existing = record;
        } else {
            file.generations.push(record);
        }
    })
}

pub(super) fn update_pending_generation(
    client_request_id: &str,
    update: impl FnOnce(&mut PendingGenerationRecord),
) -> Result<()> {
    mutate_recovery_file(|file| {
        if let Some(record) = file.generations.iter_mut().find(|item| {
            item.client_request_id == client_request_id
        }) {
            update(record);
        }
        prune_completed(file);
    })
}

pub(super) fn remove_pending_generation(client_request_id: &str) -> Result<()> {
    mutate_recovery_file(|file| {
        file.generations.retain(|item| item.client_request_id != client_request_id);
    })
}

pub(super) fn pending_delivery_saved(
    client_request_id: &str,
    delivery: &DeliveryConfirmation,
    local_path: &str,
) -> Result<()> {
    update_pending_generation(client_request_id, |record| {
        if let Some(item) = record.deliveries.iter_mut().find(|item| item.file_id == delivery.file_id) {
            item.local_path = local_path.to_string();
        } else {
            record.deliveries.push(PendingDeliveryRecord {
                item_index: delivery.item_index,
                file_id: delivery.file_id.clone(),
                sha256: delivery.sha256.clone(),
                size_bytes: delivery.size_bytes,
                local_path: local_path.to_string(),
                acknowledged: false,
            });
        }
    })
}

pub(super) fn pending_delivery_acknowledged(
    client_request_id: &str,
    file_id: &str,
) -> Result<()> {
    update_pending_generation(client_request_id, |record| {
        if let Some(item) = record.deliveries.iter_mut().find(|item| item.file_id == file_id) {
            item.acknowledged = true;
        }
    })
}

fn mutate_recovery_file(update: impl FnOnce(&mut RecoveryFile)) -> Result<()> {
    let _guard = recovery_lock().lock().unwrap_or_else(|value| value.into_inner());
    let mut file = read_recovery_file();
    file.schema_version = RECOVERY_SCHEMA_VERSION;
    update(&mut file);
    write_recovery_file(&file)
}

fn read_recovery_file() -> RecoveryFile {
    let path = generation_recovery_path();
    let Ok(text) = fs::read_to_string(path) else { return RecoveryFile::default(); };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_recovery_file(file: &RecoveryFile) -> Result<()> {
    let path = generation_recovery_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(file)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn mutate_order_recovery_file(update: impl FnOnce(&mut OrderRecoveryFile)) -> Result<()> {
    let _guard = recovery_lock().lock().unwrap_or_else(|value| value.into_inner());
    let mut file = read_order_recovery_file();
    file.schema_version = RECOVERY_SCHEMA_VERSION;
    update(&mut file);
    let path = order_recovery_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&file)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn read_order_recovery_file() -> OrderRecoveryFile {
    let Ok(text) = fs::read_to_string(order_recovery_path()) else {
        return OrderRecoveryFile::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn prune_completed(file: &mut RecoveryFile) {
    file.generations.retain(|record| {
        if !record.terminal {
            return true;
        }
        let acknowledged = record.deliveries.iter().filter(|item| item.acknowledged).count();
        acknowledged < record.expected_success_count
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_record() -> PendingGenerationRecord {
        PendingGenerationRecord {
            schema_version: 1,
            client_request_id: "request_123".to_string(),
            local_task_id: "local".to_string(),
            server_task_id: "server".to_string(),
            raw_prompt: "prompt".to_string(),
            generation_prompt: "prompt".to_string(),
            category: "character".to_string(),
            mode: "game".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model_code: "openai_image".to_string(),
            conversation_id: "conversation".to_string(),
            count: 1,
            create_conversation: true,
            reference_paths: vec![],
            uploaded_file_ids: vec![],
            deliveries: vec![PendingDeliveryRecord {
                acknowledged: false,
                ..PendingDeliveryRecord::default()
            }],
            terminal: true,
            expected_success_count: 1,
        }
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
}
