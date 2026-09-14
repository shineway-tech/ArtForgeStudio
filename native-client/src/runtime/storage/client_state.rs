use super::*;
use anyhow::ensure;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::OnceLock;

const CLIENT_STATE_FILE_NAME: &str = "client-state.sqlite3";
const CLIENT_STATE_SCHEMA_VERSION: i32 = 2;
pub(super) const KNOWN_DEVICE_SETTING_KEYS: [&str; 10] = [
    "export_directory",
    "theme_id",
    "card_style",
    "language",
    "close_behavior",
    "font_family",
    "font_size",
    "generation_gallery_layout",
    "asset_gallery_layout",
    "inspiration_gallery_layout",
];
const V1_TABLES: [&str; 8] = [
    "assets",
    "asset_references",
    "notifications",
    "canvas_nodes",
    "canvas_links",
    "custom_prompts",
    "client_settings",
    "client_meta",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ClientStateWriteError {
    StaleLease,
    RetirementInProgress,
    AuthorityExhausted,
    LocalState { message: String },
}
impl std::fmt::Display for ClientStateWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleLease => f.write_str("用户命名空间已失效"),
            Self::RetirementInProgress => f.write_str("用户命名空间正在停止写入"),
            Self::AuthorityExhausted => f.write_str("本地写入权限已关闭"),
            Self::LocalState { message } => f.write_str(message),
        }
    }
}
impl std::error::Error for ClientStateWriteError {}
pub(super) type WriteResult = std::result::Result<(), ClientStateWriteError>;
pub(super) type StoreWriteAdmission = (UserActivityPermit, api::OrdinaryDurableCommitPermit);
/// Retains unqueued admission. Return this error out of any latch completion
/// before formatting or dropping it: releasing a counted guard re-enters the latch.
pub(super) struct PreparedStoreEnqueueError {
    error: ClientStateWriteError,
    _admission: StoreWriteAdmission,
}
impl std::fmt::Debug for PreparedStoreEnqueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { std::fmt::Debug::fmt(&self.error, f) }
}
impl std::fmt::Display for PreparedStoreEnqueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { std::fmt::Display::fmt(&self.error, f) }
}
impl std::error::Error for PreparedStoreEnqueueError {}
type Ack = Sender<WriteResult>;
fn local_error(_: impl std::fmt::Display) -> ClientStateWriteError {
    ClientStateWriteError::LocalState {
        message: "本地状态操作失败".into(),
    }
}
enum ClientStateWrite {
    Wake(NamespaceLease),
    WakeDevice,
    FlushDevice {
        acknowledgement: Ack,
    },
    Flush {
        lease: NamespaceLease,
        acknowledgement: Ack,
    },
    FlushForRetirement {
        lease: NamespaceLease,
        generation: u64,
        acknowledgement: Ack,
    },
    LocalStoreChecked {
        lease: NamespaceLease,
        data: LocalStoreData,
        acknowledgement: Ack,
        admission: Option<StoreWriteAdmission>,
    },
    RetainedRedemptionRead {
        lease: NamespaceLease,
        client_request_id: String,
        acknowledgement: Sender<std::result::Result<Option<PendingCreditRedemption>, ClientStateWriteError>>,
    },
    UserProfileChecked {
        lease: NamespaceLease,
        data: UserProfileData,
        acknowledgement: Ack,
        admission: Option<StoreWriteAdmission>,
    },
    DeviceSettingsChecked {
        data: DeviceSettings,
        acknowledgement: Ack,
    },
    ExportDirectoryChecked {
        data: Option<ExportDirectoryPreference>,
        acknowledgement: Ack,
    },
    SelectedGroupChecked {
        user_public_id: String,
        device_installation_id: String,
        account_group_id: String,
        acknowledgement: Ack,
    },
}
struct QueuedWrite {
    sequence: u64,
    command: ClientStateWrite,
}
#[derive(Default)]
struct PendingClientState {
    active: Option<NamespaceLease>,
    sequence: u64,
    retirement: Option<u64>,
    authority_exhausted: bool,
    local_store: Option<(u64, NamespaceLease, LocalStoreData)>,
    user_profile: Option<(u64, NamespaceLease, UserProfileData)>,
    pending_device_settings: Option<(u64, DeviceSettings)>,
    pending_export_directory: Option<(u64, Option<ExportDirectoryPreference>)>,
    private_wake: bool,
    device_wake: bool,
}
impl PendingClientState {
    fn require(&self, lease: &NamespaceLease) -> WriteResult {
        if self.active.as_ref() == Some(lease) {
            Ok(())
        } else {
            Err(ClientStateWriteError::StaleLease)
        }
    }
    fn require_unreserved(&self) -> WriteResult {
        if self.authority_exhausted {
            Err(ClientStateWriteError::AuthorityExhausted)
        } else if self.retirement.is_some() {
            Err(ClientStateWriteError::RetirementInProgress)
        } else {
            Ok(())
        }
    }
    fn require_enqueue(&self, lease: &NamespaceLease) -> WriteResult {
        self.require(lease)?;
        self.require_unreserved()
    }
    fn next_sequence(&mut self) -> std::result::Result<u64, ClientStateWriteError> {
        if self.authority_exhausted {
            return Err(ClientStateWriteError::AuthorityExhausted);
        }
        match self.sequence.checked_add(1) {
            Some(sequence) => { self.sequence = sequence; Ok(sequence) }
            None => {
                self.authority_exhausted = true;
                Err(ClientStateWriteError::AuthorityExhausted)
            }
        }
    }
    fn clear_private(&mut self) {
        self.local_store = None;
        self.user_profile = None;
        // An already queued wake still runs; it cannot consume another lease.
        self.private_wake = false;
    }
}
#[derive(Clone)]
pub(super) struct ClientStateWriter {
    data_root: Arc<DataRootCapability>,
    path: PathBuf,
    sender: Sender<QueuedWrite>,
    pending: Arc<Mutex<PendingClientState>>,
}

// Logical ownership may move between worker and UI; the mutex never does.
// The queue generation is checked and unique for this original pending Arc.
struct WriterRetirementReservation {
    pending: Arc<Mutex<PendingClientState>>,
    lease: NamespaceLease,
    generation: u64,
    armed: bool,
}
impl WriterRetirementReservation {
    fn matches(&self, state: &PendingClientState) -> bool {
        state.retirement == Some(self.generation) && state.active.as_ref() == Some(&self.lease)
    }
}
impl Drop for WriterRetirementReservation {
    fn drop(&mut self) {
        if !self.armed { return; }
        let mut state = self.pending.lock().unwrap_or_else(|poisoned| {
            let mut state = poisoned.into_inner();
            state.authority_exhausted = true;
            state
        });
        // Clean abort only releases this reservation. It never restores a lease.
        if self.matches(&state) { state.retirement = None; }
    }
}

pub(super) struct FlushedWriterRetirement {
    reservation: WriterRetirementReservation,
}
impl FlushedWriterRetirement {
    pub(super) fn lease(&self) -> &NamespaceLease { &self.reservation.lease }
    /// Consumes the acknowledged original binding; no arbitrary writer argument,
    /// filesystem work, dispatch or acknowledgement remains after the proof.
    pub(super) fn retire_flushed(mut self) {
        let mut state = self.reservation.pending.lock().unwrap_or_else(|poisoned| {
            let mut state = poisoned.into_inner();
            state.authority_exhausted = true;
            state
        });
        if self.reservation.matches(&state) {
            state.clear_private();
            state.active = None;
            state.retirement = None;
        } else {
            // Unreachable through the sealed producer; corruption never grants
            // admission or retires an unrelated binding.
            state.authority_exhausted = true;
        }
        self.reservation.armed = false;
    }
}
static CLIENT_STATE_WRITER: OnceLock<ClientStateWriter> = OnceLock::new();
pub(super) fn client_state_path() -> PathBuf {
    app_data_dir().join(CLIENT_STATE_FILE_NAME)
}
pub(super) fn initialize_client_state_repository(data_root: Arc<DataRootCapability>) -> Result<()> {
    if CLIENT_STATE_WRITER.get().is_some() {
        anyhow::bail!("本地数据库已初始化");
    }
    let path = client_state_path();
    let mut connection = open_client_state_connection(&path)?;
    migrate_client_state_schema(&mut connection, data_root.as_ref())?;
    {
        let mut statement = connection.prepare("SELECT value_json FROM user_settings WHERE key='account_directory_mappings_v1'")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let mappings: Vec<AccountDirectoryMapping> = serde_json::from_str(&row?)?;
            for mapping in mappings { register_mapped_private_identity(mapping.identity)?; }
        }
    }
    let (writer, receiver) = ClientStateWriter::channel(path, data_root);
    let worker = writer.clone();
    std::thread::Builder::new()
        .name("client-state-writer".into())
        .spawn(move || client_state_writer_loop(connection, receiver, worker))
        .context("无法启动本地数据库写入线程")?;
    CLIENT_STATE_WRITER
        .set(writer)
        .map_err(|_| anyhow!("本地数据库重复初始化"))
}
pub(super) fn client_state_writer() -> Result<&'static ClientStateWriter> {
    CLIENT_STATE_WRITER
        .get()
        .ok_or_else(|| anyhow!("本地数据库尚未初始化"))
}
impl ClientStateWriter {
    fn channel(path: PathBuf, data_root: Arc<DataRootCapability>) -> (Self, Receiver<QueuedWrite>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                data_root,
                path,
                sender,
                pending: Arc::new(Mutex::new(PendingClientState::default())),
            },
            receiver,
        )
    }
    pub(super) fn activate(&self, lease: NamespaceLease) -> WriteResult {
        let mut pending = self.pending.lock().map_err(local_error)?;
        pending.require_unreserved()?;
        pending.clear_private();
        pending.active = Some(lease);
        Ok(())
    }
    pub(super) fn deactivate(&self, lease: &NamespaceLease) -> WriteResult {
        let mut pending = self.pending.lock().map_err(local_error)?;
        pending.require_enqueue(lease)?;
        pending.clear_private();
        pending.active = None;
        Ok(())
    }
    fn checked(
        &self,
        lease: Option<&NamespaceLease>,
        make: impl FnOnce(Ack) -> ClientStateWrite,
    ) -> WriteResult {
        let (acknowledgement, receiver) = mpsc::channel();
        {
            let mut pending = self.pending.lock().map_err(local_error)?;
            if let Some(lease) = lease {
                pending.require_enqueue(lease)?;
            }
            let sequence = pending.next_sequence()?;
            self.sender
                .send(QueuedWrite {
                    sequence,
                    command: make(acknowledgement),
                })
                .map_err(local_error)?;
        }
        receiver.recv().map_err(local_error)?
    }
    fn queue_private(
        &self,
        lease: NamespaceLease,
        store: Option<LocalStoreData>,
        profile: Option<UserProfileData>,
    ) -> WriteResult {
        let mut pending = self.pending.lock().map_err(local_error)?;
        pending.require_enqueue(&lease)?;
        let sequence = pending.next_sequence()?;
        if let Some(data) = store {
            pending.local_store = Some((sequence, lease.clone(), data));
        }
        if let Some(data) = profile {
            pending.user_profile = Some((sequence, lease.clone(), data));
        }
        if !pending.private_wake {
            self.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::Wake(lease),
                })
                .map_err(local_error)?;
            pending.private_wake = true;
        }
        Ok(())
    }
    fn queue_device(
        &self,
        settings: Option<DeviceSettings>,
        export: Option<Option<ExportDirectoryPreference>>,
    ) -> WriteResult {
        let export = export
            .map(|value| validate_export(self.data_root.as_ref(), value))
            .transpose()
            .map_err(local_error)?;
        let mut pending = self.pending.lock().map_err(local_error)?;
        let sequence = pending.next_sequence()?;
        if let Some(data) = settings {
            pending.pending_device_settings = Some((sequence, data.normalized()));
        }
        if let Some(data) = export {
            pending.pending_export_directory = Some((sequence, data));
        }
        if !pending.device_wake {
            self.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::WakeDevice,
                })
                .map_err(local_error)?;
            pending.device_wake = true;
        }
        Ok(())
    }
    pub(super) fn flush(&self, lease: &NamespaceLease) -> WriteResult {
        self.checked(Some(lease), |acknowledgement| ClientStateWrite::Flush {
            lease: lease.clone(),
            acknowledgement,
        })
    }
    pub(super) fn flush_for_retirement(
        &self,
        lease: &NamespaceLease,
    ) -> std::result::Result<FlushedWriterRetirement, ClientStateWriteError> {
        let (acknowledgement, receiver) = mpsc::channel();
        let (generation, sent) = {
            let mut pending = self.pending.lock().map_err(local_error)?;
            pending.require_enqueue(lease)?;
            let generation = pending.next_sequence()?;
            pending.retirement = Some(generation);
            let sent = self.sender.send(QueuedWrite {
                sequence: generation,
                command: ClientStateWrite::FlushForRetirement {
                    lease: lease.clone(), generation, acknowledgement,
                },
            });
            (generation, sent)
        };
        // Construct cleanup before any fallible wait, outside the short lock.
        let reservation = WriterRetirementReservation {
            pending: self.pending.clone(), lease: lease.clone(), generation, armed: true,
        };
        sent.map_err(local_error)?;
        receiver.recv().map_err(local_error)??;
        {
            let pending = self.pending.lock().map_err(local_error)?;
            if !reservation.matches(&pending) {
                return Err(ClientStateWriteError::StaleLease);
            }
        }
        Ok(FlushedWriterRetirement { reservation })
    }
    pub(super) fn flush_device(&self) -> WriteResult {
        self.checked(None, |acknowledgement| ClientStateWrite::FlushDevice {
            acknowledgement,
        })
    }
    pub(super) fn enqueue_client_state_checked_for_namespace(
        &self, lease: &NamespaceLease, data: LocalStoreData, admission: StoreWriteAdmission,
    ) -> std::result::Result<Receiver<WriteResult>, PreparedStoreEnqueueError> {
        let (acknowledgement, receiver) = mpsc::channel();
        let mut admission = Some(admission);
        let result = (|| {
            let mut pending = self.pending.lock().map_err(local_error)?;
            pending.require_enqueue(lease)?;
            let sequence = pending.next_sequence()?;
            let command = ClientStateWrite::LocalStoreChecked {
                lease: lease.clone(), data, acknowledgement, admission: admission.take(),
            };
            match self.sender.send(QueuedWrite { sequence, command }) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let ClientStateWrite::LocalStoreChecked { admission: returned, .. } = error.0.command else { unreachable!() };
                    admission = returned;
                    Err(local_error("writer unavailable"))
                }
            }
        })();
        match result {
            Ok(()) => Ok(receiver),
            Err(error) => Err(PreparedStoreEnqueueError { error, _admission: admission.expect("failed enqueue retains owned admission") }),
        }
    }
    pub(super) fn enqueue_client_user_profile_checked_for_namespace(
        &self, lease: &NamespaceLease, data: UserProfileData, admission: StoreWriteAdmission,
    ) -> std::result::Result<Receiver<WriteResult>, PreparedStoreEnqueueError> {
        let (acknowledgement, receiver) = mpsc::channel();
        let mut admission = Some(admission);
        let result = (|| {
            let mut pending = self.pending.lock().map_err(local_error)?;
            pending.require_enqueue(lease)?;
            let sequence = pending.next_sequence()?;
            let command = ClientStateWrite::UserProfileChecked {
                lease: lease.clone(), data, acknowledgement, admission: admission.take(),
            };
            match self.sender.send(QueuedWrite { sequence, command }) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let ClientStateWrite::UserProfileChecked { admission: returned, .. } = error.0.command else { unreachable!() };
                    admission = returned;
                    Err(local_error("writer unavailable"))
                }
            }
        })();
        match result {
            Ok(()) => Ok(receiver),
            Err(error) => Err(PreparedStoreEnqueueError { error, _admission: admission.expect("failed enqueue retains owned admission") }),
        }
    }
    pub(super) fn persist_client_state_checked_for_namespace(
        &self,
        lease: &NamespaceLease,
        data: LocalStoreData,
    ) -> WriteResult {
        self.checked(Some(lease), |acknowledgement| {
            ClientStateWrite::LocalStoreChecked {
                lease: lease.clone(),
                data,
                acknowledgement,
                admission: None,
            }
        })
    }
    pub(super) fn persist_client_user_profile_checked_for_namespace(
        &self,
        lease: &NamespaceLease,
        data: UserProfileData,
    ) -> WriteResult {
        self.checked(Some(lease), |acknowledgement| {
            ClientStateWrite::UserProfileChecked {
                lease: lease.clone(),
                data,
                acknowledgement,
                admission: None,
            }
        })
    }
    fn persist_device_settings_checked(&self, data: DeviceSettings) -> WriteResult {
        self.checked(None, |acknowledgement| {
            ClientStateWrite::DeviceSettingsChecked {
                data: data.normalized(),
                acknowledgement,
            }
        })
    }
    fn persist_export_directory_checked(
        &self,
        data: Option<ExportDirectoryPreference>,
    ) -> WriteResult {
        let data = validate_export(self.data_root.as_ref(), data).map_err(local_error)?;
        self.checked(None, |acknowledgement| {
            ClientStateWrite::ExportDirectoryChecked {
                data,
                acknowledgement,
            }
        })
    }
    pub(super) fn read_retained_redemption_checked(
        &self, lease: &NamespaceLease, client_request_id: &str,
    ) -> Result<Option<PendingCreditRedemption>> {
        let (acknowledgement, receiver) = mpsc::channel();
        {
            let mut pending = self.pending.lock().map_err(local_error)?;
            pending.require_enqueue(lease)?;
            let sequence = pending.next_sequence()?;
            self.sender.send(QueuedWrite { sequence, command: ClientStateWrite::RetainedRedemptionRead {
                lease: lease.clone(), client_request_id: client_request_id.into(), acknowledgement,
            }}).map_err(local_error)?;
        }
        receiver.recv().map_err(local_error)?.map_err(Into::into)
    }
    pub(super) fn load_account_directory_namespace(&self, data_root: &Path, user: &str) -> Result<UserNamespace> {
        let connection = open_client_state_connection(&self.path)?;
        let mappings: Vec<AccountDirectoryMapping> = read_setting_json_or_default(&connection, user, "account_directory_mappings_v1")?;
        mappings.into_iter().try_fold(UserNamespace::new(data_root, user)?, |namespace, mapping| namespace.with_mapping(mapping))
    }
    pub(super) fn prepare_account_directory_migration(&self, proof: &FlushedWriterRetirement, attempt: &serde_json::Value) -> Result<()> {
        let pending = self.pending.lock().map_err(local_error)?;
        ensure!(Arc::ptr_eq(&proof.reservation.pending, &self.pending) && proof.reservation.matches(&pending), "migration writer reservation expired");
        let mut connection = open_client_state_connection(&self.path)?;
        let tx = connection.transaction()?;
        write_setting_json(&tx, proof.lease().namespace.user_public_id(), "account_directory_migration_attempt_v1", attempt)?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn commit_account_directory_mapping(&self, proof: &FlushedWriterRetirement, namespace: &UserNamespace) -> Result<()> {
        let pending = self.pending.lock().map_err(local_error)?;
        ensure!(Arc::ptr_eq(&proof.reservation.pending, &self.pending) && proof.reservation.matches(&pending), "migration writer reservation expired");
        ensure!(proof.lease().namespace.user_public_id() == namespace.user_public_id(), "migration owner changed");
        let mut connection = open_client_state_connection(&self.path)?;
        let tx = connection.transaction()?;
        let saved: Vec<AccountDirectoryMapping> = read_setting_json_or_default(&tx, namespace.user_public_id(), "account_directory_mappings_v1")?;
        ensure!(saved == proof.lease().namespace.mappings(), "mapping changed since preparation");
        write_setting_json(&tx, namespace.user_public_id(), "account_directory_mappings_v1", &namespace.mappings())?;
        let mappings = namespace.mappings().strip_prefix(saved.as_slice())
            .filter(|mappings| !mappings.is_empty()).ok_or_else(|| anyhow!("missing migration mappings"))?;
        write_setting_json(&tx, namespace.user_public_id(), "account_directory_migration_attempt_v1", &serde_json::json!({ "version": 2, "phase": "committed", "mappings": mappings }))?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn complete_directory_rebind(&self, namespace: &UserNamespace) -> Result<UserNamespace> {
        let mut mappings = namespace.mappings().to_vec();
        for mapping in &mut mappings { mapping.pending_rebind.clear(); }
        let mut connection = open_client_state_connection(&self.path)?;
        let tx = connection.transaction()?;
        let saved: Vec<AccountDirectoryMapping> = read_setting_json_or_default(&tx, namespace.user_public_id(), "account_directory_mappings_v1")?;
        ensure!(saved == namespace.mappings(), "mapping changed during index rebind");
        write_setting_json(&tx, namespace.user_public_id(), "account_directory_mappings_v1", &mappings)?;
        let mut attempt: serde_json::Value = read_setting_json_or_default(&tx, namespace.user_public_id(), "account_directory_migration_attempt_v1")?;
        if attempt.get("phase").and_then(serde_json::Value::as_str) == Some("committed") {
            attempt["phase"] = "completed".into();
            write_setting_json(&tx, namespace.user_public_id(), "account_directory_migration_attempt_v1", &attempt)?;
        }
        tx.commit()?;
        let data_root = namespace.root().parent().and_then(Path::parent).ok_or_else(|| anyhow!("invalid namespace root"))?;
        mappings.into_iter().try_fold(UserNamespace::new(data_root, namespace.user_public_id())?, |namespace, mapping| namespace.with_mapping(mapping))
    }
    pub(super) fn take_account_directory_migration_notice(&self, user: &str) -> Result<Option<String>> {
        let mut connection = open_client_state_connection(&self.path)?;
        let tx = connection.transaction()?;
        let mut attempt: serde_json::Value = read_setting_json_or_default(&tx, user, "account_directory_migration_attempt_v1")?;
        if attempt.get("phase").and_then(serde_json::Value::as_str) != Some("prepared") { return Ok(None); }
        attempt["phase"] = "prepared-notified".into();
        write_setting_json(&tx, user, "account_directory_migration_attempt_v1", &attempt)?;
        tx.commit()?;
        Ok(Some("上次目录迁移未完成，当前账号仍使用原目录。目标目录可能留有副本，原文件未删除；重新迁移前请选择新的空目录。".into()))
    }
    pub(super) fn load_client_state_for_namespace(
        &self,
        lease: &NamespaceLease,
    ) -> Result<Option<LocalStoreData>> {
        let mut connection = open_client_state_connection(&self.path)?;
        let user = lease.namespace.user_public_id();
        let tx = connection.transaction()?;
        if read_meta(&tx, user, "local_store_initialized")?.as_deref() != Some("1") {
            return Ok(None);
        }
        let mut data = read_local_store_transaction(&tx, user)?;
        lease.namespace.remap_locations().remap_local_store(&mut data);
        tx.commit()?;
        Ok(Some(data))
    }
    pub(super) fn load_client_user_profile_for_namespace(
        &self,
        lease: &NamespaceLease,
    ) -> Result<Option<UserProfileData>> {
        let mut connection = open_client_state_connection(&self.path)?;
        let tx = connection.transaction()?;
        let user = lease.namespace.user_public_id();
        if read_meta(&tx, user, "user_profile_initialized")?.as_deref() != Some("1") {
            return Ok(None);
        }
        let data = read_setting_json(&tx, user, "user_profile")?;
        tx.commit()?;
        Ok(Some(data))
    }
    fn load_device_settings(&self) -> Result<Option<DeviceSettings>> {
        read_device_settings(&open_client_state_connection(&self.path)?)
    }
    fn load_export_directory(&self) -> Result<Option<ExportDirectoryPreference>> {
        let connection = open_client_state_connection(&self.path)?;
        let rows = read_device_rows(&connection)?;
        let preference = rows
            .get("export_directory")
            .map(|value| {
                serde_json::from_str::<PathBuf>(value)
                    .map(|normalized_path| ExportDirectoryPreference { normalized_path })
            })
            .transpose()?;
        validate_export(self.data_root.as_ref(), preference)
    }
    pub(super) fn load_selected_group(&self, user: &str, device: &str) -> Result<Option<String>> {
        validate_uuid(user)?;
        anyhow::ensure!(!device.is_empty(), "设备标识不能为空");
        Ok(open_client_state_connection(&self.path)?.query_row("SELECT account_group_id FROM billing_context_preferences WHERE user_public_id = ?1 AND device_installation_id = ?2", params![user, device], |row| row.get(0)).optional()?)
    }
    pub(super) fn save_selected_group(&self, user: &str, device: &str, group: &str) -> Result<()> {
        validate_uuid(user)?;
        validate_uuid(group)?;
        anyhow::ensure!(!device.is_empty(), "设备标识不能为空");
        self.checked(None, |acknowledgement| {
            ClientStateWrite::SelectedGroupChecked {
                user_public_id: user.into(),
                device_installation_id: device.into(),
                account_group_id: group.into(),
                acknowledgement,
            }
        })
        .map_err(Into::into)
    }
}
fn validate_uuid(value: &str) -> Result<()> {
    anyhow::ensure!(
        uuid::Uuid::parse_str(value)?.to_string() == value,
        "标识必须采用标准 UUID 格式"
    );
    Ok(())
}

// Each slot has its own committed sequence. A coalesced async snapshot can
// overtake a checked command, but an older command can never replay over it.
#[derive(Default)]
struct CommittedSequences {
    store: u64,
    profile: u64,
    device: u64,
    export: u64,
    store_error: Option<(NamespaceLease, ClientStateWriteError)>,
    profile_error: Option<(NamespaceLease, ClientStateWriteError)>,
    device_error: Option<ClientStateWriteError>,
    export_error: Option<ClientStateWriteError>,
    // Unlike the compatibility Flush errors above, these obligations cannot be
    // consumed by an empty retry. Only an actually committed covering snapshot
    // for the same slot and exact lease can discharge one.
    retirement_debts: Vec<RetirementWriteDebt>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum RetirementWriteSlot { Store, Profile, Device, Export }
struct RetirementWriteDebt {
    slot: RetirementWriteSlot,
    lease: Option<NamespaceLease>,
    sequence: u64,
    error: ClientStateWriteError,
}
impl CommittedSequences {
    fn record_retirement_result(
        &mut self, slot: RetirementWriteSlot, lease: Option<&NamespaceLease>,
        sequence: u64, result: &WriteResult,
    ) {
        let existing = self.retirement_debts.iter().position(|debt| {
            debt.slot == slot && debt.lease.as_ref() == lease
        });
        if let Some(index) = existing {
            if self.retirement_debts[index].sequence > sequence { return; }
            self.retirement_debts.remove(index);
        }
        if let Err(error) = result {
            self.retirement_debts.push(RetirementWriteDebt {
                slot, lease: lease.cloned(), sequence, error: error.clone(),
            });
        }
    }
    fn retirement_result(&self, lease: &NamespaceLease) -> WriteResult {
        self.retirement_debts.iter()
            .find(|debt| debt.lease.as_ref().is_none_or(|owner| owner == lease))
            .map_or(Ok(()), |debt| Err(debt.error.clone()))
    }
    fn take_device_error(&mut self) -> WriteResult {
        let error = self.device_error.take().or(self.export_error.take());
        error.map_or(Ok(()), Err)
    }
    fn take_private_error(&mut self, lease: &NamespaceLease) -> WriteResult {
        let error = self
            .store_error
            .take()
            .into_iter()
            .chain(self.profile_error.take())
            .find(|(owner, _)| owner == lease)
            .map(|(_, error)| error);
        error.map_or(Ok(()), Err)
    }
}
fn commit_private(
    connection: &mut Connection,
    writer: &ClientStateWriter,
    lease: &NamespaceLease,
    sequence: u64,
    last: &mut u64,
    write: impl FnOnce(&mut Connection, &str) -> Result<()>,
) -> WriteResult {
    let guard = writer.pending.lock().map_err(local_error)?;
    guard.require(lease)?;
    if sequence <= *last {
        return Ok(());
    }
    // This guard spans transaction.commit(): activation cannot publish B between
    // A's lease check and A's commit. It never covers provider or UI operations.
    write(connection, lease.namespace.user_public_id()).map_err(local_error)?;
    *last = sequence;
    Ok(())
}
fn drain_private(
    connection: &mut Connection,
    writer: &ClientStateWriter,
    lease: &NamespaceLease,
    last: &mut CommittedSequences,
) -> WriteResult {
    let (store, profile) = {
        let mut pending = writer.pending.lock().map_err(local_error)?;
        pending.require(lease)?;
        pending.private_wake = false;
        (pending.local_store.take(), pending.user_profile.take())
    };
    let mut result = Ok(());
    if let Some((seq, lease, data)) = store {
        let before = last.store;
        result = commit_private(connection, writer, &lease, seq, &mut last.store, |c, u| {
            write_local_store(c, u, &data)
        });
        if result.is_err() || last.store > before {
            last.record_retirement_result(RetirementWriteSlot::Store, Some(&lease), seq, &result);
        }
        last.store_error = result.as_ref().err().map(|error| (lease, error.clone()));
    }
    if let Some((seq, lease, data)) = profile {
        let before = last.profile;
        let next = commit_private(
            connection,
            writer,
            &lease,
            seq,
            &mut last.profile,
            |c, u| write_user_profile(c, u, &data),
        );
        if next.is_err() || last.profile > before {
            last.record_retirement_result(RetirementWriteSlot::Profile, Some(&lease), seq, &next);
        }
        last.profile_error = next.as_ref().err().map(|error| (lease, error.clone()));
        result = result.and(next);
    }
    result
}
fn drain_device(
    connection: &mut Connection,
    writer: &ClientStateWriter,
    last: &mut CommittedSequences,
) -> WriteResult {
    let (settings, export) = {
        let mut pending = writer.pending.lock().map_err(local_error)?;
        pending.device_wake = false;
        (
            pending.pending_device_settings.take(),
            pending.pending_export_directory.take(),
        )
    };
    let mut result = Ok(());
    if let Some((seq, data)) = settings {
        if seq > last.device {
            result = write_device_settings(connection, &data).map_err(local_error);
            last.record_retirement_result(RetirementWriteSlot::Device, None, seq, &result);
            last.device_error = result.as_ref().err().cloned();
            if result.is_ok() {
                last.device = seq;
            }
        }
    }
    if let Some((seq, data)) = export {
        if seq > last.export {
            let next = write_export_directory(connection, writer.data_root.as_ref(), data)
                .map_err(local_error);
            last.record_retirement_result(RetirementWriteSlot::Export, None, seq, &next);
            last.export_error = next.as_ref().err().cloned();
            if next.is_ok() {
                last.export = seq;
            }
            result = result.and(next);
        }
    }
    result
}
fn process_client_state_write(
    connection: &mut Connection,
    writer: &ClientStateWriter,
    last: &mut CommittedSequences,
    queued: QueuedWrite,
) {
    let seq = queued.sequence;
    match queued.command {
        ClientStateWrite::RetainedRedemptionRead { lease, client_request_id, acknowledgement } => {
            let result = (|| {
                // Same held connection as writes. No path resolution, open,
                // schema migration, chmod, or WAL reconfiguration is permitted.
                let pending = writer.pending.lock().map_err(local_error)?;
                pending.require(&lease)?;
                let tx = connection.transaction().map_err(local_error)?;
                let records: Option<BTreeMap<String, PendingCreditRedemption>> =
                    read_setting_json(&tx, lease.namespace.user_public_id(), "pending_credit_redemptions_by_owner").map_err(local_error)?;
                let record = records.and_then(|mut rows| rows.remove(lease.namespace.user_public_id()))
                    .filter(|row| row.client_request_id == client_request_id);
                tx.commit().map_err(local_error)?;
                Ok(record)
            })();
            let _ = acknowledgement.send(result);
        }
        ClientStateWrite::Wake(lease) => {
            let _ = drain_private(connection, writer, &lease, last);
        }
        ClientStateWrite::WakeDevice => {
            let _ = drain_device(connection, writer, last);
        }
        ClientStateWrite::FlushDevice { acknowledgement } => {
            let result = drain_device(connection, writer, last);
            let prior = last.take_device_error();
            let _ = acknowledgement.send(result.and(prior));
        }
        ClientStateWrite::Flush {
            lease,
            acknowledgement,
        } => {
            let device = drain_device(connection, writer, last);
            let private = drain_private(connection, writer, &lease, last);
            let prior_device = last.take_device_error();
            let prior_private = last.take_private_error(&lease);
            let _ = acknowledgement.send(device.and(private).and(prior_device).and(prior_private));
        }
        ClientStateWrite::FlushForRetirement { lease, generation, acknowledgement } => {
            let result = (|| {
                {
                    let pending = writer.pending.lock().map_err(local_error)?;
                    pending.require(&lease)?;
                    if pending.retirement != Some(generation) {
                        return Err(ClientStateWriteError::StaleLease);
                    }
                }
                let device = drain_device(connection, writer, last);
                let private = drain_private(connection, writer, &lease, last);
                device.and(private).and(last.retirement_result(&lease))
            })();
            let _ = acknowledgement.send(result);
        }
        ClientStateWrite::LocalStoreChecked {
            lease,
            data,
            acknowledgement,
            admission,
        } => {
            let before = last.store;
            let result = commit_private(
                connection,
                writer,
                &lease,
                seq,
                &mut last.store,
                |c, u| write_local_store(c, u, &data),
            );
            if result.is_err() || last.store > before {
                last.record_retirement_result(RetirementWriteSlot::Store, Some(&lease), seq, &result);
            }
            let _ = acknowledgement.send(result);
            drop(admission);
        }
        ClientStateWrite::UserProfileChecked {
            lease,
            data,
            acknowledgement,
            admission,
        } => {
            let before = last.profile;
            let result = commit_private(
                connection,
                writer,
                &lease,
                seq,
                &mut last.profile,
                |c, u| write_user_profile(c, u, &data),
            );
            if result.is_err() || last.profile > before {
                last.record_retirement_result(RetirementWriteSlot::Profile, Some(&lease), seq, &result);
            }
            let _ = acknowledgement.send(result);
            drop(admission);
        }
        ClientStateWrite::DeviceSettingsChecked {
            data,
            acknowledgement,
        } => {
            let result = if seq > last.device {
                let result = write_device_settings(connection, &data).map_err(local_error);
                last.record_retirement_result(RetirementWriteSlot::Device, None, seq, &result);
                result
            } else {
                Ok(())
            };
            if result.is_ok() {
                last.device = last.device.max(seq);
            }
            let _ = acknowledgement.send(result);
        }
        ClientStateWrite::ExportDirectoryChecked {
            data,
            acknowledgement,
        } => {
            let result = if seq > last.export {
                let result = write_export_directory(connection, writer.data_root.as_ref(), data)
                    .map_err(local_error);
                last.record_retirement_result(RetirementWriteSlot::Export, None, seq, &result);
                result
            } else {
                Ok(())
            };
            if result.is_ok() {
                last.export = last.export.max(seq);
            }
            let _ = acknowledgement.send(result);
        }
        ClientStateWrite::SelectedGroupChecked {
            user_public_id,
            device_installation_id,
            account_group_id,
            acknowledgement,
        } => {
            let result = write_selected_group(
                connection,
                &user_public_id,
                &device_installation_id,
                &account_group_id,
            )
            .map_err(local_error);
            let _ = acknowledgement.send(result);
        }
    }
}
fn client_state_writer_loop(
    mut connection: Connection,
    receiver: Receiver<QueuedWrite>,
    writer: ClientStateWriter,
) {
    let mut last = CommittedSequences::default();
    while let Ok(command) = receiver.recv() {
        process_client_state_write(&mut connection, &writer, &mut last, command);
    }
}
pub(super) fn load_client_state_for_namespace(
    lease: &NamespaceLease,
) -> Result<Option<LocalStoreData>> {
    client_state_writer()?.load_client_state_for_namespace(lease)
}
pub(super) fn load_client_user_profile_for_namespace(
    lease: &NamespaceLease,
) -> Result<Option<UserProfileData>> {
    client_state_writer()?.load_client_user_profile_for_namespace(lease)
}
pub(super) fn persist_client_state_async_for_namespace(
    lease: NamespaceLease,
    data: LocalStoreData,
) -> Result<()> {
    client_state_writer()?
        .queue_private(lease, Some(data), None)
        .map_err(Into::into)
}
pub(super) fn persist_client_state_checked_for_namespace(
    lease: &NamespaceLease,
    data: LocalStoreData,
) -> Result<()> {
    client_state_writer()?
        .persist_client_state_checked_for_namespace(lease, data)
        .map_err(Into::into)
}
pub(super) fn persist_client_user_profile_async_for_namespace(
    lease: NamespaceLease,
    data: UserProfileData,
) -> Result<()> {
    client_state_writer()?
        .queue_private(lease, None, Some(data))
        .map_err(Into::into)
}
pub(super) fn persist_client_user_profile_checked_for_namespace(
    lease: &NamespaceLease,
    data: UserProfileData,
) -> Result<()> {
    client_state_writer()?
        .persist_client_user_profile_checked_for_namespace(lease, data)
        .map_err(Into::into)
}
pub(super) fn load_device_settings() -> Result<Option<DeviceSettings>> {
    client_state_writer()?.load_device_settings()
}
pub(super) fn persist_device_settings_async(data: DeviceSettings) -> Result<()> {
    client_state_writer()?
        .queue_device(Some(data), None)
        .map_err(Into::into)
}
pub(super) fn persist_device_settings_checked(data: DeviceSettings) -> Result<()> {
    client_state_writer()?
        .persist_device_settings_checked(data)
        .map_err(Into::into)
}
pub(super) fn load_export_directory() -> Result<Option<ExportDirectoryPreference>> {
    client_state_writer()?.load_export_directory()
}
pub(super) fn persist_export_directory_async(
    data: Option<ExportDirectoryPreference>,
) -> Result<()> {
    client_state_writer()?
        .queue_device(None, Some(data))
        .map_err(Into::into)
}
pub(super) fn persist_export_directory_checked(
    data: Option<ExportDirectoryPreference>,
) -> Result<()> {
    client_state_writer()?
        .persist_export_directory_checked(data)
        .map_err(Into::into)
}
pub(super) fn flush_device() -> Result<()> {
    client_state_writer()?.flush_device().map_err(Into::into)
}
pub(super) fn load_selected_group(
    user_public_id: &str,
    device_installation_id: &str,
) -> Result<Option<String>> {
    client_state_writer()?.load_selected_group(user_public_id, device_installation_id)
}
pub(super) fn save_selected_group(
    user_public_id: &str,
    device_installation_id: &str,
    account_group_id: &str,
) -> Result<()> {
    client_state_writer()?.save_selected_group(
        user_public_id,
        device_installation_id,
        account_group_id,
    )
}

fn migrate_client_state_schema(
    connection: &mut Connection,
    data_root: &DataRootCapability,
) -> Result<()> {
    migrate_schema_with_checkpoint(connection, data_root, |_| Ok(()))
}
fn migrate_schema_with_checkpoint(
    connection: &mut Connection,
    data_root: &DataRootCapability,
    mut checkpoint: impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    anyhow::ensure!(
        version <= CLIENT_STATE_SCHEMA_VERSION,
        "本地数据库来自更新版本"
    );
    if version == CLIENT_STATE_SCHEMA_VERSION {
        return Ok(());
    }
    let tx = connection.transaction()?;
    if version == 1 {
        for (index, table) in V1_TABLES.iter().enumerate() {
            tx.execute_batch(&format!(
                "ALTER TABLE {table} RENAME TO legacy_unassigned_{table};"
            ))?;
            checkpoint(index)?;
        }
    }
    tx.execute_batch(V2_SCHEMA)?;
    if version == 1 {
        extract_legacy_device_settings(&tx, data_root)?;
    }
    tx.pragma_update(None, "user_version", CLIENT_STATE_SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}
fn extract_legacy_device_settings(tx: &Transaction<'_>, root: &DataRootCapability) -> Result<()> {
    let legacy = |key: &str| -> Result<Option<String>> {
        Ok(tx
            .query_row(
                "SELECT value_json FROM legacy_unassigned_client_settings WHERE key=?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    };
    let mut settings = DeviceSettings::default();
    if let Some(bytes) = legacy("user_profile")? {
        match decode_legacy_device_settings(&bytes) {
            Ok(extracted) => settings = extracted,
            Err(diagnostic) => eprintln!("{diagnostic}"),
        }
    }
    let mut values = serde_json::to_value(settings)?;
    for key in KNOWN_DEVICE_SETTING_KEYS.iter().skip(1) {
        if let Some(bytes) = legacy(key)? {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&bytes) {
                values[*key] = value;
            }
        }
    }
    let settings: DeviceSettings = serde_json::from_value(values)?;
    write_device_rows(tx, &settings.normalized())?;
    if let Some(bytes) = legacy("directory_locations")? {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&bytes) {
            if let Some(output) = value
                .get("output")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
            {
                if let Ok(destination) = ExternalExportDestination::open(root, Path::new(output)) {
                    tx.execute("INSERT INTO device_settings(key,value_json) VALUES ('export_directory',?1)", params![serde_json::to_string(destination.normalized_display_path())?])?;
                }
            }
        }
    }
    Ok(())
}
fn decode_legacy_device_settings(bytes: &str) -> std::result::Result<DeviceSettings, &'static str> {
    serde_json::from_str::<LegacyUserProfileData>(bytes)
        .map(|profile| profile.device_settings())
        .map_err(|_| "legacy_user_profile_device_extract_skipped")
}
fn read_device_rows(connection: &Connection) -> Result<BTreeMap<String, String>> {
    let mut stmt = connection.prepare("SELECT key,value_json FROM device_settings")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    anyhow::ensure!(
        rows.keys()
            .all(|key| KNOWN_DEVICE_SETTING_KEYS.contains(&key.as_str())),
        "未知设备设置项"
    );
    Ok(rows)
}
fn read_device_settings(connection: &Connection) -> Result<Option<DeviceSettings>> {
    let rows = read_device_rows(connection)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let mut value = serde_json::to_value(DeviceSettings::default())?;
    for key in KNOWN_DEVICE_SETTING_KEYS.iter().skip(1) {
        if let Some(bytes) = rows.get(*key) {
            value[*key] = serde_json::from_str::<serde_json::Value>(bytes)?;
        }
    }
    Ok(Some(
        serde_json::from_value::<DeviceSettings>(value)?.normalized(),
    ))
}
fn write_device_rows(tx: &Transaction<'_>, data: &DeviceSettings) -> Result<()> {
    for (key, value) in serde_json::to_value(data.normalized())?
        .as_object()
        .ok_or_else(|| anyhow!("设备设置无效"))?
    {
        tx.execute("INSERT INTO device_settings(key,value_json) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json", params![key, serde_json::to_string(value)?])?;
    }
    Ok(())
}
fn write_device_settings(connection: &mut Connection, data: &DeviceSettings) -> Result<()> {
    let tx = connection.transaction()?;
    write_device_rows(&tx, data)?;
    tx.commit()?;
    Ok(())
}
fn validate_export(
    root: &DataRootCapability,
    data: Option<ExportDirectoryPreference>,
) -> Result<Option<ExportDirectoryPreference>> {
    data.map(|value| {
        ExternalExportDestination::open(root, &value.normalized_path).map(|destination| {
            ExportDirectoryPreference {
                normalized_path: destination.normalized_display_path().to_path_buf(),
            }
        })
    })
    .transpose()
}
fn write_export_directory(
    connection: &mut Connection,
    root: &DataRootCapability,
    data: Option<ExportDirectoryPreference>,
) -> Result<()> {
    let tx = connection.transaction()?;
    let destination = data
        .map(|value| ExternalExportDestination::open(root, &value.normalized_path))
        .transpose()?;
    if let Some(ref destination) = destination {
        tx.execute("INSERT INTO device_settings(key,value_json) VALUES ('export_directory',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json", params![serde_json::to_string(destination.normalized_display_path())?])?;
    } else {
        tx.execute(
            "DELETE FROM device_settings WHERE key='export_directory'",
            [],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn write_selected_group(
    connection: &Connection,
    user: &str,
    device: &str,
    group: &str,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    connection.execute(
        "INSERT INTO billing_context_preferences(user_public_id, device_installation_id, account_group_id, updated_at_epoch_ms)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_public_id, device_installation_id) DO UPDATE SET
            account_group_id=excluded.account_group_id, updated_at_epoch_ms=excluded.updated_at_epoch_ms
         WHERE user_public_id = ?1 AND device_installation_id = ?2",
        params![user, device, group, i64::try_from(now)?],
    )?;
    Ok(())
}

fn open_client_state_connection(path: &Path) -> Result<Connection> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("本地数据库不能使用符号链接")
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            anyhow::bail!("本地数据库路径不是普通文件")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let connection = Connection::open(path)?;
    restrict_client_state_file(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    let integrity: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        anyhow::bail!("本地数据库完整性检查失败: {integrity}");
    }
    Ok(connection)
}

#[cfg(unix)]
fn restrict_client_state_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_client_state_file(_path: &Path) -> Result<()> {
    Ok(())
}

const V2_SCHEMA: &str = r#"CREATE TABLE user_meta (
            user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (user_public_id, key)
        );
        CREATE TABLE assets (
            user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
            collection TEXT NOT NULL,
            id TEXT NOT NULL,
            position INTEGER NOT NULL,
            conversation_id TEXT NOT NULL,
            title TEXT NOT NULL,
            category TEXT NOT NULL,
            kind TEXT NOT NULL,
            time TEXT NOT NULL,
            prompt TEXT NOT NULL,
            ratio TEXT NOT NULL,
            quality TEXT NOT NULL,
            model TEXT NOT NULL,
            origin TEXT NOT NULL,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            source_path TEXT NOT NULL,
            cutout_done INTEGER NOT NULL,
            remove_black_done INTEGER NOT NULL,
            upscale_done INTEGER NOT NULL,
            PRIMARY KEY (user_public_id, collection, id)
        );
        CREATE INDEX assets_v2_user_collection_position
            ON assets(user_public_id, collection, position);
        CREATE TABLE asset_references (
            user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
            collection TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            path TEXT NOT NULL,
            PRIMARY KEY (user_public_id, collection, asset_id, position),
            FOREIGN KEY (user_public_id, collection, asset_id)
                REFERENCES assets(user_public_id, collection, id) ON DELETE CASCADE
        );
        CREATE TABLE notifications (
            user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
            id TEXT NOT NULL,
            position INTEGER NOT NULL,
            title TEXT NOT NULL,
            model TEXT NOT NULL,
            time TEXT NOT NULL,
            reason TEXT NOT NULL,
            success INTEGER NOT NULL,
            read INTEGER NOT NULL,
            PRIMARY KEY (user_public_id, id)
        );
        CREATE TABLE canvas_nodes (
            user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
            id TEXT NOT NULL,
            position INTEGER NOT NULL,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            x REAL NOT NULL,
            y REAL NOT NULL,
            width REAL NOT NULL,
            height REAL NOT NULL,
            parent_group_id TEXT NOT NULL,
            z_index INTEGER NOT NULL,
            image_path TEXT NOT NULL,
            font_size REAL NOT NULL,
            PRIMARY KEY (user_public_id, id)
        );
        CREATE TABLE canvas_links (
            user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
            id TEXT NOT NULL,
            position INTEGER NOT NULL,
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            flow_reversed INTEGER NOT NULL,
            PRIMARY KEY (user_public_id, id)
        );
        CREATE TABLE custom_prompts (
            user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
            prompt TEXT NOT NULL,
            position INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            profile_json TEXT NOT NULL,
            PRIMARY KEY (user_public_id, prompt)
        );
        CREATE TABLE user_settings (
            user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
            key TEXT NOT NULL,
            value_json TEXT NOT NULL,
            PRIMARY KEY (user_public_id, key)
        );
CREATE TABLE device_settings (key TEXT PRIMARY KEY NOT NULL, value_json TEXT NOT NULL);
CREATE TABLE billing_context_preferences (user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36), device_installation_id TEXT NOT NULL CHECK(length(device_installation_id)>0), account_group_id TEXT NOT NULL CHECK(length(account_group_id)=36), updated_at_epoch_ms INTEGER NOT NULL, PRIMARY KEY(user_public_id, device_installation_id));"#;
fn write_local_store(
    connection: &mut Connection,
    user_public_id: &str,
    data: &LocalStoreData,
) -> Result<()> {
    let transaction = connection.transaction()?;
    let previous: BTreeMap<String,SavedVideoOutput> = read_setting_json_or_default(&transaction,user_public_id,"video_outputs")?;
    for (key,output) in &data.video_outputs {
        output.validate()?;
        anyhow::ensure!(key==&output.key(),"saved video key mismatch");
    }
    for (key,output) in &previous {
        anyhow::ensure!(data.video_outputs.get(key)==Some(output),"retained video cannot be replaced or erased by a Store snapshot");
    }
    write_setting_json(&transaction,user_public_id,"video_outputs",&data.video_outputs)?;

    write_asset_collection(
        &transaction,
        user_public_id,
        "generation",
        &data.generations,
    )?;
    write_asset_collection(&transaction, user_public_id, "asset", &data.assets)?;
    write_notifications(&transaction, user_public_id, &data.notifications)?;
    write_canvas_nodes(&transaction, user_public_id, &data.canvas_notes)?;
    write_canvas_links(&transaction, user_public_id, &data.canvas_links)?;
    write_custom_prompts(&transaction, user_public_id, data)?;

    write_setting_json(&transaction, user_public_id, "pending_credit_redemptions_by_owner", &data.pending_credit_redemptions_by_owner)?;
    write_setting_json(&transaction, user_public_id, "references", &data.references)?;

    write_setting_json(
        &transaction,
        user_public_id,
        "image_model",
        &data.image_model,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "reasoning_model",
        &data.reasoning_model,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "video_model",
        &data.video_model,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "prompt_drafts",
        &data.prompt_drafts,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "dismissed_prompt_history",
        &data.dismissed_prompt_history,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "selected_custom_prompts",
        &data.selected_custom_prompts,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "active_canvas_workspace_id",
        &data.active_canvas_workspace_id,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "canvas_workspaces",
        &data.canvas_workspaces,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "deep_prompt_job_id",
        &data.deep_prompt_job_id,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "deep_prompt_jobs_by_owner",
        &data.deep_prompt_jobs_by_owner,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "deep_prompt_pending_requests_by_owner",
        &data.deep_prompt_pending_requests_by_owner,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "deep_prompt_bindings",
        &data.deep_prompt_bindings,
    )?;
    write_setting_json(
        &transaction,
        user_public_id,
        "contact_popup_dismissed",
        &data.contact_popup_dismissed,
    )?;
    write_meta(&transaction, user_public_id, "local_store_initialized", "1")?;
    transaction.commit()?;
    Ok(())
}

fn write_user_profile(
    connection: &mut Connection,
    user_public_id: &str,
    data: &UserProfileData,
) -> Result<()> {
    let transaction = connection.transaction()?;
    write_setting_json(&transaction, user_public_id, "user_profile", data)?;
    write_meta(
        &transaction,
        user_public_id,
        "user_profile_initialized",
        "1",
    )?;
    transaction.commit()?;
    Ok(())
}

fn write_asset_collection(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    collection: &str,
    assets: &[StoredAssetData],
) -> Result<()> {
    transaction.execute(
        "DELETE FROM assets WHERE user_public_id = ?1 AND collection = ?2
         AND id NOT IN (SELECT value FROM json_each(?3))",
        params![
            user_public_id,
            collection,
            serde_json::to_string(&assets.iter().map(|item| &item.id).collect::<Vec<_>>())?
        ],
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO assets(user_public_id,
            collection, id, position, conversation_id, title, category, kind, time,
            prompt, ratio, quality, model, origin, width, height, source_path,
            cutout_done, remove_black_done, upscale_done
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18, ?19, ?20
        ) ON CONFLICT(user_public_id, collection, id) DO UPDATE SET
            position=excluded.position, conversation_id=excluded.conversation_id,
            title=excluded.title, category=excluded.category, kind=excluded.kind,
            time=excluded.time, prompt=excluded.prompt, ratio=excluded.ratio,
            quality=excluded.quality, model=excluded.model, origin=excluded.origin,
            width=excluded.width, height=excluded.height, source_path=excluded.source_path,
            cutout_done=excluded.cutout_done,
            remove_black_done=excluded.remove_black_done,
            upscale_done=excluded.upscale_done
        WHERE user_public_id = ?1 AND (position != excluded.position OR conversation_id != excluded.conversation_id
            OR title != excluded.title OR category != excluded.category OR kind != excluded.kind
            OR time != excluded.time OR prompt != excluded.prompt OR ratio != excluded.ratio
            OR quality != excluded.quality OR model != excluded.model OR origin != excluded.origin
            OR width != excluded.width OR height != excluded.height
            OR source_path != excluded.source_path OR cutout_done != excluded.cutout_done
            OR remove_black_done != excluded.remove_black_done
            OR upscale_done != excluded.upscale_done)",
    )?;
    for (position, asset) in assets.iter().enumerate() {
        statement.execute(params![
            user_public_id,
            collection,
            asset.id,
            position as i64,
            asset.conversation_id,
            asset.title,
            asset.category,
            asset.kind,
            asset.time,
            asset.prompt,
            asset.ratio,
            asset.quality,
            asset.model,
            asset.origin,
            asset.width,
            asset.height,
            asset.source_path,
            asset.cutout_done,
            asset.remove_black_done,
            asset.upscale_done,
        ])?;
        transaction.execute(
            "DELETE FROM asset_references WHERE user_public_id = ?1 AND collection = ?2 AND asset_id = ?3
             AND position >= ?4",
            params![user_public_id, collection, asset.id, asset.reference_paths.len() as i64],
        )?;
        for (reference_position, path) in asset.reference_paths.iter().enumerate() {
            transaction.execute(
                "INSERT INTO asset_references(user_public_id, collection, asset_id, position, path)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(user_public_id, collection, asset_id, position) DO UPDATE SET path=excluded.path
                 WHERE user_public_id = ?1 AND (path != excluded.path)",
                params![user_public_id, collection, asset.id, reference_position as i64, path],
            )?;
        }
    }
    Ok(())
}

fn write_notifications(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    notifications: &[NotificationData],
) -> Result<()> {
    delete_missing_ids(
        transaction,
        user_public_id,
        "notifications",
        notifications.iter().map(|item| item.id.as_str()),
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO notifications(user_public_id, id, position, title, model, time, reason, success, read)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(user_public_id, id) DO UPDATE SET position=excluded.position, title=excluded.title,
            model=excluded.model, time=excluded.time, reason=excluded.reason,
            success=excluded.success, read=excluded.read
         WHERE user_public_id = ?1 AND (position != excluded.position OR title != excluded.title OR model != excluded.model
            OR time != excluded.time OR reason != excluded.reason
            OR success != excluded.success OR read != excluded.read)",
    )?;
    for (position, item) in notifications.iter().enumerate() {
        statement.execute(params![
            user_public_id,
            item.id,
            position as i64,
            item.title,
            item.model,
            item.time,
            item.reason,
            item.success,
            item.read,
        ])?;
    }
    Ok(())
}

fn write_canvas_nodes(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    nodes: &[CanvasNoteData],
) -> Result<()> {
    delete_missing_ids(
        transaction,
        user_public_id,
        "canvas_nodes",
        nodes.iter().map(|item| item.id.as_str()),
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO canvas_nodes(user_public_id,
            id, position, kind, content, x, y, width, height, parent_group_id,
            z_index, image_path, font_size
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(user_public_id, id) DO UPDATE SET position=excluded.position, kind=excluded.kind,
            content=excluded.content, x=excluded.x, y=excluded.y, width=excluded.width,
            height=excluded.height, parent_group_id=excluded.parent_group_id,
            z_index=excluded.z_index, image_path=excluded.image_path,
            font_size=excluded.font_size
         WHERE user_public_id = ?1 AND (position != excluded.position OR kind != excluded.kind OR content != excluded.content
            OR x != excluded.x OR y != excluded.y OR width != excluded.width
            OR height != excluded.height OR parent_group_id != excluded.parent_group_id
            OR z_index != excluded.z_index OR image_path != excluded.image_path
            OR font_size != excluded.font_size)",
    )?;
    for (position, item) in nodes.iter().enumerate() {
        statement.execute(params![
            user_public_id,
            item.id,
            position as i64,
            item.kind,
            item.content,
            item.x,
            item.y,
            item.width,
            item.height,
            item.parent_group_id,
            item.z_index,
            item.image_path,
            item.font_size,
        ])?;
    }
    Ok(())
}

fn write_canvas_links(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    links: &[CanvasLinkData],
) -> Result<()> {
    delete_missing_ids(
        transaction,
        user_public_id,
        "canvas_links",
        links.iter().map(|item| item.id.as_str()),
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO canvas_links(user_public_id, id, position, source_id, target_id, flow_reversed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(user_public_id, id) DO UPDATE SET position=excluded.position,
            source_id=excluded.source_id, target_id=excluded.target_id,
            flow_reversed=excluded.flow_reversed
         WHERE user_public_id = ?1 AND (position != excluded.position OR source_id != excluded.source_id
            OR target_id != excluded.target_id OR flow_reversed != excluded.flow_reversed)",
    )?;
    for (position, item) in links.iter().enumerate() {
        statement.execute(params![
            user_public_id,
            item.id,
            position as i64,
            item.source_id,
            item.target_id,
            item.flow_reversed,
        ])?;
    }
    Ok(())
}

fn write_custom_prompts(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    data: &LocalStoreData,
) -> Result<()> {
    delete_missing_text_values(
        transaction,
        user_public_id,
        "custom_prompts",
        "prompt",
        &data.custom_prompts,
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO custom_prompts(user_public_id, prompt, position, created_at, profile_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(user_public_id, prompt) DO UPDATE SET position=excluded.position,
            created_at=excluded.created_at, profile_json=excluded.profile_json
         WHERE user_public_id = ?1 AND (position != excluded.position OR created_at != excluded.created_at
            OR profile_json != excluded.profile_json)",
    )?;
    for (position, prompt) in data.custom_prompts.iter().enumerate() {
        let created_at = data
            .custom_prompt_times
            .get(prompt)
            .cloned()
            .unwrap_or_default();
        let profile = data
            .custom_prompt_profiles
            .get(prompt)
            .cloned()
            .unwrap_or_default();
        statement.execute(params![
            user_public_id,
            prompt,
            position as i64,
            created_at,
            serde_json::to_string(&profile)?,
        ])?;
    }
    Ok(())
}

fn delete_missing_ids<'a>(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    table: &str,
    ids: impl Iterator<Item = &'a str>,
) -> Result<()> {
    let ids = ids.collect::<Vec<_>>();
    delete_missing_text_values(transaction, user_public_id, table, "id", &ids)
}

fn delete_missing_text_values<T: AsRef<str>>(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    table: &str,
    column: &str,
    values: &[T],
) -> Result<()> {
    let sql =
        format!("DELETE FROM {table} WHERE user_public_id = ?1 AND {column} NOT IN (SELECT value FROM json_each(?2))");
    let values = values
        .iter()
        .map(|value| value.as_ref())
        .collect::<Vec<_>>();
    transaction.execute(
        &sql,
        params![user_public_id, serde_json::to_string(&values)?],
    )?;
    Ok(())
}

fn read_local_store_transaction(
    transaction: &Transaction<'_>,
    user_public_id: &str,
) -> Result<LocalStoreData> {
    let custom_prompt_rows = read_custom_prompts(transaction, user_public_id)?;
    let mut custom_prompts = Vec::with_capacity(custom_prompt_rows.len());
    let mut custom_prompt_times = BTreeMap::new();
    let mut custom_prompt_profiles = BTreeMap::new();
    for (prompt, created_at, profile) in custom_prompt_rows {
        custom_prompt_times.insert(prompt.clone(), created_at);
        custom_prompt_profiles.insert(prompt.clone(), profile);
        custom_prompts.push(prompt);
    }
    Ok(LocalStoreData {
        video_outputs: read_setting_json_or_default(transaction,user_public_id,"video_outputs")?,
        references: read_setting_json_or_default(transaction, user_public_id, "references")?,
        pending_credit_redemptions_by_owner: read_setting_json_or_default(transaction, user_public_id, "pending_credit_redemptions_by_owner")?,
        generations: read_assets(transaction, user_public_id, "generation")?,
        assets: read_assets(transaction, user_public_id, "asset")?,
        notifications: read_notifications(transaction, user_public_id)?,
        image_model: read_setting_json_or_default(transaction, user_public_id, "image_model")?,
        reasoning_model: read_setting_json_or_default(
            transaction,
            user_public_id,
            "reasoning_model",
        )?,
        video_model: read_setting_json_or_default(transaction, user_public_id, "video_model")?,
        prompt_drafts: read_setting_json_or_default(transaction, user_public_id, "prompt_drafts")?,
        dismissed_prompt_history: read_setting_json_or_default(
            transaction,
            user_public_id,
            "dismissed_prompt_history",
        )?,
        custom_prompts,
        selected_custom_prompts: read_setting_json_or_default(
            transaction,
            user_public_id,
            "selected_custom_prompts",
        )?,
        custom_prompt_times,
        custom_prompt_profiles,
        canvas_notes: read_canvas_nodes(transaction, user_public_id)?,
        canvas_links: read_canvas_links(transaction, user_public_id)?,
        active_canvas_workspace_id: read_setting_json_or_default(
            transaction,
            user_public_id,
            "active_canvas_workspace_id",
        )?,
        canvas_workspaces: read_setting_json_or_default(
            transaction,
            user_public_id,
            "canvas_workspaces",
        )?,
        deep_prompt_job_id: read_setting_json_or_default(
            transaction,
            user_public_id,
            "deep_prompt_job_id",
        )?,
        deep_prompt_jobs_by_owner: read_setting_json_or_default(
            transaction,
            user_public_id,
            "deep_prompt_jobs_by_owner",
        )?,
        deep_prompt_pending_requests_by_owner: read_setting_json_or_default(
            transaction,
            user_public_id,
            "deep_prompt_pending_requests_by_owner",
        )?,
        deep_prompt_bindings: read_setting_json_or_default(
            transaction,
            user_public_id,
            "deep_prompt_bindings",
        )?,
        contact_popup_dismissed: read_setting_json_or_default(
            transaction,
            user_public_id,
            "contact_popup_dismissed",
        )?,
    })
}

fn read_assets(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    collection: &str,
) -> Result<Vec<StoredAssetData>> {
    let mut statement = transaction.prepare(
        "SELECT id, conversation_id, title, category, kind, time, prompt, ratio,
                quality, model, origin, width, height, source_path,
                cutout_done, remove_black_done, upscale_done
         FROM assets WHERE user_public_id = ?1 AND collection = ?2 ORDER BY position, id",
    )?;
    let mut rows = statement.query(params![user_public_id, collection])?;
    let mut assets = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        assets.push(StoredAssetData {
            id: id.clone(),
            conversation_id: row.get(1)?,
            title: row.get(2)?,
            category: row.get(3)?,
            kind: row.get(4)?,
            time: row.get(5)?,
            prompt: row.get(6)?,
            ratio: row.get(7)?,
            quality: row.get(8)?,
            model: row.get(9)?,
            origin: row.get(10)?,
            width: row.get(11)?,
            height: row.get(12)?,
            source_path: row.get(13)?,
            reference_paths: read_asset_references(transaction, user_public_id, collection, &id)?,
            cutout_done: row.get(14)?,
            remove_black_done: row.get(15)?,
            upscale_done: row.get(16)?,
        });
    }
    Ok(assets)
}

fn read_asset_references(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    collection: &str,
    asset_id: &str,
) -> Result<Vec<String>> {
    let mut statement = transaction.prepare_cached(
        "SELECT path FROM asset_references WHERE user_public_id = ?1 AND collection = ?2 AND asset_id = ?3 ORDER BY position",
    )?;
    let values = statement
        .query_map(params![user_public_id, collection, asset_id], |row| {
            row.get(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(values)
}

fn read_notifications(
    transaction: &Transaction<'_>,
    user_public_id: &str,
) -> Result<Vec<NotificationData>> {
    let mut statement = transaction.prepare(
        "SELECT id, title, model, time, reason, success, read
         FROM notifications WHERE user_public_id = ?1 ORDER BY position, id",
    )?;
    let values = statement
        .query_map(params![user_public_id], |row| {
            Ok(NotificationData {
                id: row.get(0)?,
                title: row.get(1)?,
                model: row.get(2)?,
                time: row.get(3)?,
                reason: row.get(4)?,
                success: row.get(5)?,
                read: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(values)
}

fn read_canvas_nodes(
    transaction: &Transaction<'_>,
    user_public_id: &str,
) -> Result<Vec<CanvasNoteData>> {
    let mut statement = transaction.prepare(
        "SELECT id, kind, content, x, y, width, height, parent_group_id,
                z_index, image_path, font_size
         FROM canvas_nodes WHERE user_public_id = ?1 ORDER BY position, id",
    )?;
    let values = statement
        .query_map(params![user_public_id], |row| {
            Ok(CanvasNoteData {
                id: row.get(0)?,
                kind: row.get(1)?,
                content: row.get(2)?,
                x: row.get(3)?,
                y: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                parent_group_id: row.get(7)?,
                z_index: row.get(8)?,
                image_path: row.get(9)?,
                font_size: row.get(10)?,
                selected: false,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(values)
}

fn read_canvas_links(
    transaction: &Transaction<'_>,
    user_public_id: &str,
) -> Result<Vec<CanvasLinkData>> {
    let mut statement = transaction.prepare(
        "SELECT id, source_id, target_id, flow_reversed
         FROM canvas_links WHERE user_public_id = ?1 ORDER BY position, id",
    )?;
    let values = statement
        .query_map(params![user_public_id], |row| {
            Ok(CanvasLinkData {
                id: row.get(0)?,
                source_id: row.get(1)?,
                target_id: row.get(2)?,
                flow_reversed: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(values)
}

fn read_custom_prompts(
    transaction: &Transaction<'_>,
    user_public_id: &str,
) -> Result<Vec<(String, String, CustomPromptProfile)>> {
    let mut statement = transaction.prepare(
        "SELECT prompt, created_at, profile_json
         FROM custom_prompts WHERE user_public_id = ?1 ORDER BY position, prompt",
    )?;
    let mut rows = statement.query(params![user_public_id])?;
    let mut prompts = Vec::new();
    while let Some(row) = rows.next()? {
        let profile_json: String = row.get(2)?;
        prompts.push((
            row.get(0)?,
            row.get(1)?,
            serde_json::from_str(&profile_json)?,
        ));
    }
    Ok(prompts)
}

fn write_setting_json(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    key: &str,
    value: &impl Serialize,
) -> Result<()> {
    let value = serde_json::to_string(value)?;
    transaction.execute(
        "INSERT INTO user_settings(user_public_id, key, value_json) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_public_id, key) DO UPDATE SET value_json=excluded.value_json
         WHERE user_public_id = ?1 AND (value_json != excluded.value_json)",
        params![user_public_id, key, value],
    )?;
    Ok(())
}

fn read_setting_json<T: DeserializeOwned>(
    connection: &Connection,
    user_public_id: &str,
    key: &str,
) -> Result<T> {
    let value: String = connection
        .query_row(
            "SELECT value_json FROM user_settings WHERE user_public_id = ?1 AND key = ?2",
            params![user_public_id, key],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow!("本地数据库缺少设置项 {key}"))?;
    Ok(serde_json::from_str(&value)?)
}

fn read_setting_json_or_default<T: DeserializeOwned + Default>(
    connection: &Connection,
    user_public_id: &str,
    key: &str,
) -> Result<T> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value_json FROM user_settings WHERE user_public_id = ?1 AND key = ?2",
            params![user_public_id, key],
            |row| row.get(0),
        )
        .optional()?;
    value
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .unwrap_or_else(|| Ok(T::default()))
}

fn write_meta(
    transaction: &Transaction<'_>,
    user_public_id: &str,
    key: &str,
    value: &str,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO user_meta(user_public_id, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_public_id, key) DO UPDATE SET value=excluded.value WHERE user_public_id = ?1",
        params![user_public_id, key, value],
    )?;
    Ok(())
}

fn read_meta(connection: &Connection, user_public_id: &str, key: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT value FROM user_meta WHERE user_public_id = ?1 AND key = ?2",
            params![user_public_id, key],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Condvar,
    };
    const USER_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const USER_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const GROUP_A: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    const GROUP_B: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
    const DELIVERY_TASK: &str = "11111111-1111-4111-8111-111111111111";
    const DELIVERY_FILE: &str = "22222222-2222-4222-8222-222222222222";
    // Hand-known 1x1 opaque black grayscale+alpha PNG (68 encoded bytes).
    const DELIVERY_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    fn run_owned_worker<T: Send>(operation: impl FnOnce() -> T + Send) -> T {
        std::thread::scope(|scope| scope.spawn(operation).join().unwrap())
    }
    struct DeliveryFixture {
        authority: Arc<NamespaceStorageAuthority>,
        session: Arc<SessionManager>,
        client: ApiClient,
        activity: UserActivityGate,
        active_namespace: Arc<Mutex<Option<NamespaceLease>>>,
        api: GenerationApi,
        index: FileIndex,
        record: PendingGenerationRecord,
        scope: BillingScope,
        repo: Fixture,
    }
    impl DeliveryFixture {
        fn new(url: &str) -> Self {
            Self::new_with_canvas_source(url, "")
        }
        fn new_with_canvas_source(url: &str, canvas_source: &str) -> Self {
            Self::new_with_canvas_count(url,canvas_source,1)
        }
        fn new_with_canvas_count(url: &str, canvas_source: &str, count:i32) -> Self {
            let repo = test_repository_v2();
            let session = Arc::new(SessionManager::new(Arc::new(
                crate::runtime::test_support::MemoryRefreshTokenStore::default(),
            )));
            let session_scope = session
                .install_tokens_for_user(
                    &TokenSet {
                        access_token: "delivery-fixture-access".into(),
                        access_expires_in_seconds: 1800,
                        refresh_token: "delivery-fixture-refresh".into(),
                        refresh_expires_at: "2099-01-01T00:00:00Z".into(),
                        token_type: "X-Token".into(),
                    },
                    USER_A,
                )
                .unwrap();
            let scope = BillingScope {
                request: GroupRequestScope {
                    session: session_scope.clone(),
                    account_group_id: GROUP_A.into(),
                },
                context_epoch: 1,
            };
            let lease = repo.lease(USER_A, session_scope.auth_epoch, 1);
            let authority = Arc::new(
                NamespaceStorageAuthority::open(repo.data_root_capability_arc(), &lease).unwrap(),
            );
            repo.activate(lease.clone()).unwrap();
            let client = ApiClient::new(
                    ApiClientConfig {
                        base_url: reqwest::Url::parse(url).unwrap(),
                        app_version: "fixture".into(),
                        timeout: Duration::from_secs(3),
                    },
                    DeviceIdentity {
                        id: USER_B.into(),
                        name: "fixture".into(),
                        platform: "macos".into(),
                    },
                    session.clone(),
                )
                .unwrap();
            let activity = UserActivityGate::default();
            activity.activate(lease.clone()).unwrap();
            let active_namespace=Arc::new(Mutex::new(Some(lease)));
            client.bind_user_work(UserWorkAdmission::new(active_namespace.clone(), activity.clone())).unwrap();
            let api = GenerationApi::new(client.clone());
            let index = FileIndex::initialize(repo.directory.path().join("index.sqlite3")).unwrap();
            let record: PendingGenerationRecord = serde_json::from_value(serde_json::json!({
                "schema_version":2, "created_at_epoch_ms":1,
                "client_request_id":"delivery-request", "owner_user_id":USER_A,
                "billing_account_group_id":GROUP_A, "auth_epoch":session_scope.auth_epoch,
                "local_task_id":"local-delivery", "server_task_id":DELIVERY_TASK,
                "raw_prompt":"raw prompt", "generation_prompt":"generated prompt",
                "task_type":"image_generation", "category":"scene", "mode":"game",
                "ratio":"1:1", "quality":"1K", "model_code":"image-model",
                "conversation_id":"delivery-conversation", "count":count,
                "canvas_source_node_id":canvas_source,
                "create_conversation":false, "lineage_reference_paths":["captured/reference.png"]
            }))
            .unwrap();
            upsert_pending_generation_for_namespace(&authority, &scope, record.clone()).unwrap();
            Self {
                repo,
                authority,
                session,
                client, activity, active_namespace,
                api,
                index,
                record,
                scope,
            }
        }
        fn bound_context(&self) -> AppContext {
            let context=AppContext {
                backend:Some(Arc::new(BackendRuntime{api:self.client.clone()})),
                data_root_capability:Some(self.repo.data_root_capability_arc()),file_index:Some(self.index.clone()),
                user_activity:self.activity.clone(),active_namespace:self.active_namespace.clone(),
                current_user_id:Arc::new(Mutex::new(Some(USER_A.into()))),
                account_snapshot_scope:Arc::new(Mutex::new(Some(self.scope.request.session.clone()))),
                ..Default::default()
            };
            let transition=context.namespace_operations.try_begin_transition().unwrap();
            let phase=transition.begin_prepublication_recovery(self.authority.lease()).unwrap();
            phase.verify_no_unsupported_imports(&self.authority).unwrap();
            let proof=phase.finish().unwrap();
            transition.prepare_publication(self.authority.lease(),proof).unwrap().publish();
            context.store.borrow_mut().private_persistence=Some(PrivatePersistence::for_test_with_storage(
                self.repo.writer.clone(),self.authority.lease().clone(),self.activity.clone(),self.client.upgrade_latch().clone(),
                self.repo.data_root_capability_arc(),self.client.clone(),self.index.clone()));
            context
        }
        fn prepare(&self) -> std::result::Result<PreparedNamespaceDelivery, DeliveryRetryError> {
            run_owned_worker(|| {
                prepare_namespace_delivery(
                    &self.api,
                    self.authority.clone(),
                    self.index.clone(),
                    &self.record.identity(),
                    0,
                )
            })
        }
        fn recovery(&self) -> serde_json::Value {
            serde_json::to_value(load_pending_generations_for_namespace(&self.authority).unwrap())
                .unwrap()
        }
        fn output(&self) -> PathBuf {
            self.authority
                .lease()
                .namespace
                .path(ManagedUserArea::Output)
                .join(format!("{DELIVERY_FILE}.png"))
        }
    }
    struct DeliveryServer {
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<Vec<(String, bool)>>>,
    }
    impl DeliveryServer {
        fn start(
            listener: std::net::TcpListener,
            base: &str,
            fixture: &DeliveryFixture,
            mut detail: serde_json::Value,
            blob: Vec<u8>,
            ack_ok: bool,
        ) -> Self {
            Self::start_controlled(listener,base,fixture,detail,blob,ack_ok,2,None)
        }
        fn start_controlled(
            listener: std::net::TcpListener, base: &str, fixture: &DeliveryFixture,
            mut detail: serde_json::Value, blob: Vec<u8>, ack_ok: bool,
            expected_assets: i64, mut before_ack_response: Option<Box<dyn FnOnce()+Send>>,
        ) -> Self {
            use std::io::Write;
            let winner = detail.as_object_mut().unwrap().remove("_fixture_winner");
            detail["items"][0]["file"]["download_url"] = format!("{base}blob").into();
            let db = fixture.repo.path.clone();
            let authority = fixture.authority.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let worker = std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                let mut requests = Vec::new();
                let mut ack_attempts = 0;
                while !stopped.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                    let mut stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(e) => panic!("fixture accept failed: {e}"),
                    };
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let request = String::from_utf8(
                        backend_generation::billing_capture_test_support::read_request_bytes(
                            &mut stream,
                        ),
                    )
                    .unwrap();
                    let is_blob = request.starts_with("GET /blob ");
                    let is_ack = request.starts_with("POST ");
                    if is_ack {
                        ack_attempts += 1;
                    }
                    if is_blob {
                        if let Some(winner) = &winner {
                            let key = ManagedFileKey::new(
                                ManagedUserArea::Output,
                                &format!("{DELIVERY_FILE}.png"),
                            )
                            .unwrap();
                            let mut file = authority.create_new_regular(&key).unwrap();
                            let bytes: &[u8] = if winner == "matching" {
                                DELIVERY_PNG
                            } else {
                                b"collision sentinel"
                            };
                            authority
                                .write_new_regular_from(&mut file, &mut &bytes[..])
                                .unwrap();
                            authority.sync_regular(&mut file).unwrap();
                        }
                    }
                    let durable = if is_ack {
                        let connection = open_client_state_connection(&db).unwrap();
                        let assets: i64 = connection
                            .query_row(
                                "SELECT COUNT(*) FROM assets WHERE user_public_id=?1",
                                [USER_A],
                                |row| row.get(0),
                            )
                            .unwrap();
                        let notifications: i64 = connection
                            .query_row(
                                "SELECT COUNT(*) FROM notifications WHERE user_public_id=?1",
                                [USER_A],
                                |row| row.get(0),
                            )
                            .unwrap();
                        let records = load_pending_generations_for_namespace(&authority).unwrap();
                        assets == expected_assets
                            && notifications == 1
                            && records.len() == 1
                            && records[0].billing_account_group_id == GROUP_A
                            && records[0].deliveries.len() == 1
                            && !records[0].deliveries[0].local_path.is_empty()
                            && !records[0].deliveries[0].acknowledged
                    } else {
                        false
                    };
                    let body = if is_blob {
                        blob.clone()
                    } else {
                        serde_json::to_vec(&serde_json::json!({
                            "request_id":"fixture", "data":if is_ack { serde_json::json!({}) } else { detail.clone() },
                            "error":null, "meta":null
                        })).unwrap()
                    };
                    let status = if is_ack && !ack_ok && ack_attempts == 1 {
                        "503 Service Unavailable"
                    } else {
                        "200 OK"
                    };
                    let content_length = if is_blob {
                        detail["items"][0]["file"]["size_bytes"]
                            .as_str()
                            .and_then(|size| size.parse::<usize>().ok())
                            .unwrap_or(0)
                            .max(body.len())
                    } else {
                        body.len()
                    };
                    if is_ack { if let Some(hook)=before_ack_response.take(){hook();} }
                    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n").unwrap();
                    // An invalidating client may close early; teardown must still join.
                    let _ = stream.write_all(&body);
                    requests.push((request, durable));
                }
                requests
            });
            Self {
                stop,
                worker: Some(worker),
            }
        }
        fn finish(mut self) -> Vec<(String, bool)> {
            self.stop.store(true, Ordering::SeqCst);
            self.worker.take().unwrap().join().unwrap()
        }
    }
    impl Drop for DeliveryServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }
// Appended to client_state::tests. Uses the existing held namespace, actual
// SQLite writer, controlled delivery HTTP and joined family fixtures.
fn cutout_delivery_fixture(url: &str, subject: &str) -> DeliveryFixture {
    use sha2::Digest;
    let mut f = DeliveryFixture::new(url);
    f.authority=Arc::new(NamespaceStorageAuthority::open_active(f.repo.data_root_capability_arc(),
        f.authority.lease(),f.client.clone(),f.index.clone()).unwrap());
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(80,80,image::Rgba([17,41,89,255])));
    let source = persist_reference_image_for_namespace(&f.authority,&image).unwrap();
    let bytes = f.authority.read_image_source(&source,100*1024*1024).unwrap();
    f.record.task_type="image_cutout".into();f.record.quality=subject.into();
    f.record.category="other".into();f.record.raw_prompt="original source".into();
    f.record.reference_paths=vec![source.to_str().unwrap().into()];
    f.record.reference_sha256=vec![format!("{:x}",sha2::Sha256::digest(&bytes))];
    f.record.reference_size_bytes=vec![bytes.len() as u64];
    f.record.lineage_reference_paths=f.record.reference_paths.clone();
    upsert_pending_generation_for_namespace(&f.authority,&f.scope,f.record.clone()).unwrap();
    f
}
fn cutout_mask() -> Vec<u8> {
    let image=image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(80,80,image::Luma([127])));
    let mut output=std::io::Cursor::new(Vec::new());image.write_to(&mut output,image::ImageFormat::Png).unwrap();output.into_inner()
}
fn cutout_detail(bytes:&[u8])->serde_json::Value {
    use sha2::Digest;
    let mut detail=delivery_detail();detail["type"]="image_cutout".into();
    detail["items"][0]["file"]["sha256"]=format!("{:x}",sha2::Sha256::digest(bytes)).into();
    detail["items"][0]["file"]["size_bytes"]=bytes.len().to_string().into();detail
}
fn prepare_cutout_fixture(f:&DeliveryFixture)->std::result::Result<PreparedNamespaceDelivery,DeliveryRetryError>{
    run_owned_worker(||prepare_namespace_cutout_delivery(&f.api,f.authority.clone(),f.index.clone(),&f.record.identity(),0))
}
#[test]
fn core_cutout_derived_pixels_and_store_ack_keep_original_remote_confirmation(){
    use sha2::Digest;
    i_slint_backend_testing::init_no_event_loop();let app=AppWindow::new().unwrap();
    let(listener,url)=backend_generation::billing_capture_test_support::listener();
    let f=cutout_delivery_fixture(&url,"skin");let context=f.bound_context();let mask=cutout_mask();
    let original_hash=format!("{:x}",sha2::Sha256::digest(&mask));
    let server=DeliveryServer::start_controlled(listener,&url,&f,cutout_detail(&mask),mask.clone(),true,1,None);
    let _drain=DeliveryFamilyDrain;
    let prepared=prepare_cutout_fixture(&f).unwrap();let path=prepared.source_path().to_owned();
    assert_ne!(path,f.output().to_string_lossy());assert!(path.ends_with("-cutout-v1.png"));
    assert_eq!(prepared.confirmation().sha256,original_hash);assert_eq!(prepared.confirmation().size_bytes,mask.len() as u64);
    let derived=std::fs::read(&path).unwrap();assert_ne!(format!("{:x}",sha2::Sha256::digest(&derived)),original_hash);
    let pixels=image::load_from_memory(&derived).unwrap().to_rgba8();assert_eq!(pixels.get_pixel(1,1).0,[17,41,89,127]);
    f.repo.private_paused.store(true,Ordering::SeqCst);let _resume=DeliveryWriterResume(f.repo.private_paused.clone());
    let done=Rc::new(Cell::new(false));let observed=done.clone();
    start_image_delivery_commit(&app,context.clone(),prepared,"now".into(),move|_,result|{assert!(result.unwrap().2);observed.set(true);});
    assert!(!done.get());assert!(f.repo.load_client_state_for_namespace(f.authority.lease()).unwrap().is_none());
    assert_eq!(context.store.borrow().assets[0].source_path,path);assert!(context.store.borrow().generations.is_empty());
    f.repo.resume_private_writer();pump_delivery_fixture(||done.get());
    let saved=f.repo.load_client_state_for_namespace(f.authority.lease()).unwrap().unwrap();
    assert_eq!(saved.assets.len(),1);assert_eq!(saved.assets[0].source_path,path);assert!(saved.assets[0].cutout_done);
    assert_eq!(saved.assets[0].origin,"image_cutout");assert_eq!(f.recovery(),serde_json::json!([]));
    assert_eq!(std::fs::read(f.output()).unwrap(),mask);assert!(Path::new(&f.record.reference_paths[0]).exists());
    let requests=server.finish();let acknowledgments=requests.iter().filter(|request|request.0.starts_with("POST ")).collect::<Vec<_>>();
    assert_eq!(acknowledgments.len(),1);assert!(acknowledgments[0].1,"real SQLite state must precede remote ack");
    let body:serde_json::Value=serde_json::from_str(acknowledgments[0].0.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["sha256"],original_hash);assert_eq!(body["size_bytes"],mask.len() as u64);
}
#[test]
fn core_cutout_derived_index_crash_window_reuses_verified_files_without_redownload(){
    let(listener,url)=backend_generation::billing_capture_test_support::listener();
    let f=cutout_delivery_fixture(&url,"sky");let mask=cutout_mask();
    let server=DeliveryServer::start_controlled(listener,&url,&f,cutout_detail(&mask),mask,true,1,None);
    let index=Connection::open(f.repo.directory.path().join("index.sqlite3")).unwrap();
    index.execute_batch("CREATE TRIGGER fail_cutout_index BEFORE INSERT ON managed_files WHEN NEW.path LIKE '%-cutout-v1.png' BEGIN SELECT RAISE(FAIL,'controlled derived index failure'); END;").unwrap();
    let before=f.recovery();assert!(prepare_cutout_fixture(&f).is_err());assert_eq!(f.recovery(),before);
    let derived=f.authority.lease().namespace.path(ManagedUserArea::Output).join(format!("{DELIVERY_FILE}-cutout-v1.png"));
    let bytes=std::fs::read(&derived).unwrap();let mask_bytes=std::fs::read(f.output()).unwrap();
    index.execute_batch("DROP TRIGGER fail_cutout_index").unwrap();
    let prepared=prepare_cutout_fixture(&f).unwrap();assert_eq!(prepared.source_path(),derived.to_str().unwrap());
    assert_eq!(std::fs::read(&derived).unwrap(),bytes);assert_eq!(std::fs::read(f.output()).unwrap(),mask_bytes);
    let requests=server.finish();assert_eq!(requests.iter().filter(|r|r.0.starts_with("GET /blob ")).count(),1);
    assert!(!requests.iter().any(|r|r.0.starts_with("POST ")));assert_eq!(f.recovery(),before);
}
#[test]
fn core_cutout_stale_input_and_derived_collision_preserve_original_files_and_row(){
    for collision in [false,true] {
        let(listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=cutout_delivery_fixture(&url,"skin");let mask=cutout_mask();
        let server=DeliveryServer::start_controlled(listener,&url,&f,cutout_detail(&mask),mask,true,1,None);
        let before=f.recovery();let derived=f.authority.lease().namespace.path(ManagedUserArea::Output).join(format!("{DELIVERY_FILE}-cutout-v1.png"));
        let changed=if collision{derived}else{PathBuf::from(&f.record.reference_paths[0])};
        std::fs::write(&changed,b"preserved replacement sentinel").unwrap();
        assert!(prepare_cutout_fixture(&f).is_err());assert_eq!(std::fs::read(&changed).unwrap(),b"preserved replacement sentinel");
        assert_eq!(f.recovery(),before);assert!(!server.finish().iter().any(|r|r.0.starts_with("POST ")));
    }
}

#[cfg(unix)]
struct CutoutRecoveryPermissions { path:PathBuf, permissions:std::fs::Permissions }
#[cfg(unix)]
impl Drop for CutoutRecoveryPermissions { fn drop(&mut self) {
    if let Err(error)=std::fs::set_permissions(&self.path,self.permissions.clone()){
        if std::thread::panicking(){eprintln!("controlled Recovery fixture permission restoration failed during unwind: {error}");}
        else{panic!("controlled Recovery fixture permission restoration failed: {error}");}
    }
} }

#[cfg(unix)]
fn assert_cutout_atomic_settle_rejection(case:&'static str){
    use std::os::unix::fs::PermissionsExt;
    i_slint_backend_testing::init_no_event_loop();let app=AppWindow::new().unwrap();
    let(listener,url)=backend_generation::billing_capture_test_support::listener();
    let f=cutout_delivery_fixture(&url,"skin");let context=f.bound_context();let mask=cutout_mask();
    let recovery_dir=f.authority.lease().namespace.path(ManagedUserArea::Recovery);
    let recovery_path=recovery_dir.join("pending-generations.json");
    let permissions=CutoutRecoveryPermissions{path:recovery_dir.clone(),permissions:std::fs::metadata(&recovery_dir).unwrap().permissions()};
    let expected=Arc::new(Mutex::new(None::<Vec<u8>>));let observed=expected.clone();
    let captured_path=recovery_path.clone();
    let hook=Box::new(move||{
        let mut bytes=std::fs::read(&captured_path).unwrap();
        if case=="write-failure" {std::fs::set_permissions(&recovery_dir,std::fs::Permissions::from_mode(0o500)).unwrap();}
        else {
            let mut document:serde_json::Value=serde_json::from_slice(&bytes).unwrap();let row=&mut document["generations"][0];
            match case {
                "stale-input"=>row["reference_sha256"][0]="00".repeat(32).into(),
                "wrong-confirmation"=>row["deliveries"][0]["sha256"]="00".repeat(32).into(),
                "wrong-path"=>row["deliveries"][0]["local_path"]="different nonempty untrusted display path".into(),
                "extra-delivery"=>{let mut extra=row["deliveries"][0].clone();extra["file_id"]="44444444-4444-4444-8444-444444444444".into();extra["item_index"]=1.into();row["deliveries"].as_array_mut().unwrap().push(extra);},
                "duplicate-delivery"=>{let duplicate=row["deliveries"][0].clone();row["deliveries"].as_array_mut().unwrap().push(duplicate);},
                _=>panic!("unknown controlled cutout case"),
            }
            bytes=serde_json::to_vec_pretty(&document).unwrap();std::fs::write(&captured_path,&bytes).unwrap();
        }
        *observed.lock().unwrap()=Some(bytes);
    });
    let server=DeliveryServer::start_controlled(listener,&url,&f,cutout_detail(&mask),mask,true,1,Some(hook));
    let _drain=DeliveryFamilyDrain;
    let prepared=prepare_cutout_fixture(&f).unwrap();let output=prepared.source_path().to_owned();
    let done=Rc::new(Cell::new(false));let observed=done.clone();
    start_image_delivery_commit(&app,context.clone(),prepared,"now".into(),move|_,result|{
        assert!(!result.unwrap().2,"rejected atomic settle must not claim completed cleanup");observed.set(true);
    });
    pump_delivery_fixture(||done.get());
    let expected=expected.lock().unwrap().clone().expect("real remote ack hook must execute");
    assert_eq!(std::fs::read(&recovery_path).unwrap(),expected,"failed atomic settle changed retained bytes");
    assert!(Path::new(&f.record.reference_paths[0]).exists());assert!(Path::new(&output).exists());assert!(f.output().exists());
    assert_eq!(context.store.borrow().assets.len(),1);assert!(context.store.borrow().generations.is_empty());
    drop(permissions);
    if case=="write-failure" {
        let prepared=prepare_cutout_fixture(&f).unwrap();
        let retry=Rc::new(Cell::new(false));let observed=retry.clone();
        start_image_delivery_commit(&app,context.clone(),prepared,"retry".into(),move|_,result|{assert!(result.unwrap().2);observed.set(true);});
        pump_delivery_fixture(||retry.get());
        assert_eq!(f.recovery(),serde_json::json!([]));assert_eq!(context.store.borrow().assets.len(),1);
    }
    let requests=server.finish();let acknowledgments=requests.iter().filter(|r|r.0.starts_with("POST ")).collect::<Vec<_>>();
    assert_eq!(acknowledgments.len(),if case=="write-failure"{2}else{1});
    assert!(acknowledgments.iter().all(|ack|ack.1),"real Store ack must precede original remote ack");
}
#[cfg(unix)]
#[test]
fn core_cutout_atomic_settle_rejects_changed_original_input_after_real_remote_ack(){assert_cutout_atomic_settle_rejection("stale-input");}
#[cfg(unix)]
#[test]
fn core_cutout_atomic_settle_rejects_wrong_original_confirmation_after_real_remote_ack(){assert_cutout_atomic_settle_rejection("wrong-confirmation");}
#[cfg(unix)]
#[test]
fn core_cutout_atomic_settle_rejects_duplicate_delivery_after_real_remote_ack(){assert_cutout_atomic_settle_rejection("duplicate-delivery");}
#[cfg(unix)]
#[test]
fn core_cutout_atomic_settle_disk_failure_preserves_all_input_and_delivery_fields(){assert_cutout_atomic_settle_rejection("write-failure");}
#[cfg(unix)]
#[test]
fn core_cutout_atomic_settle_rejects_changed_original_mask_path_after_real_remote_ack(){assert_cutout_atomic_settle_rejection("wrong-path");}
#[cfg(unix)]
#[test]
fn core_cutout_atomic_settle_rejects_extra_out_of_count_delivery_after_real_remote_ack(){assert_cutout_atomic_settle_rejection("extra-delivery");}

    fn delivery_detail() -> serde_json::Value {
        serde_json::json!({
            "id":DELIVERY_TASK, "billing_account_group_id":GROUP_A, "status":"completed",
            "progress_percent":100, "success_count":1, "failure_count":0, "failure":null,
            "prompt":"server prompt", "result_prompt":null, "request":{}, "model":null,
            "quality":"1K", "requested_count":1, "type":"image_generation",
            "items":[{"index":0,"status":"succeeded","credit_cost":"1","failure":null,
                "file":{"id":DELIVERY_FILE,"status":"available","mime_type":"image/png",
                    "size_bytes":"68","sha256":"431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460",
                    "width":999,"height":999,"download_url":null}}]
        })
    }
    #[test]
    fn namespace_delivery_rejects_wrong_saved_payer_or_task_before_blob() {
        for field in ["billing_account_group_id", "id"] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let f = DeliveryFixture::new(&url);
            let before = f.recovery();
            let mut detail = delivery_detail();
            detail[field] = GROUP_B.into();
            let server =
                DeliveryServer::start(listener, &url, &f, detail, DELIVERY_PNG.to_vec(), true);
            let result = f.prepare();
            let requests = server.finish();
            assert!(result.is_err());
            assert_eq!(requests.len(), 1);
            assert!(requests[0].0.starts_with("GET /v1/generation/tasks/"));
            assert_eq!(f.recovery(), before);
            assert!(!f.output().exists());
            assert!(f
                .authority
                .enumerate_regular_names(ManagedUserArea::Output)
                .unwrap()
                .is_empty());
        }
    }
    #[test]
    fn namespace_delivery_bad_downloads_never_publish_or_index() {
        for case in [
            "zero",
            "leading-zero",
            "hash-format",
            "truncated",
            "oversized",
            "wrong-hash",
            "invalid-image",
            "large-image",
            "collision",
        ] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let f = DeliveryFixture::new(&url);
            let before = f.recovery();
            let mut detail = delivery_detail();
            let mut blob = DELIVERY_PNG.to_vec();
            match case {
                "zero" => detail["items"][0]["file"]["size_bytes"] = "0".into(),
                "leading-zero" => detail["items"][0]["file"]["size_bytes"] = "068".into(),
                "hash-format" => detail["items"][0]["file"]["sha256"] = "not-a-hash".into(),
                "truncated" => {
                    blob.pop();
                }
                "oversized" => blob.push(0),
                "wrong-hash" => detail["items"][0]["file"]["sha256"] = "0".repeat(64).into(),
                "invalid-image" => {
                    use sha2::Digest;
                    blob = b"verified but not an image".to_vec();
                    detail["items"][0]["file"]["size_bytes"] = blob.len().to_string().into();
                    detail["items"][0]["file"]["sha256"] =
                        format!("{:x}", sha2::Sha256::digest(&blob)).into();
                }
                "large-image" => {
                    // Header-only 100001 x 100001 BMP; a valid hash cannot bypass
                    // the decoder's source-pixel/allocation policy.
                    use sha2::Digest;
                    blob = vec![0; 54];
                    blob[0..2].copy_from_slice(b"BM");
                    blob[2..6].copy_from_slice(&54u32.to_le_bytes());
                    blob[10..14].copy_from_slice(&54u32.to_le_bytes());
                    blob[14..18].copy_from_slice(&40u32.to_le_bytes());
                    blob[18..22].copy_from_slice(&100001u32.to_le_bytes());
                    blob[22..26].copy_from_slice(&100001u32.to_le_bytes());
                    blob[26..28].copy_from_slice(&1u16.to_le_bytes());
                    blob[28..30].copy_from_slice(&24u16.to_le_bytes());
                    detail["items"][0]["file"]["size_bytes"] = "54".into();
                    detail["items"][0]["file"]["sha256"] =
                        format!("{:x}", sha2::Sha256::digest(&blob)).into();
                }
                "collision" => {
                    std::fs::write(f.output(), b"sentinel").unwrap();
                }
                _ => unreachable!(),
            }
            let server = DeliveryServer::start(listener, &url, &f, detail, blob, true);
            let result = f.prepare();
            let requests = server.finish();
            assert!(result.is_err(), "{case}");
            assert!(!requests.iter().any(|r| r.0.starts_with("POST ")), "{case}");
            assert_eq!(f.recovery(), before, "{case}");
            let names = f
                .authority
                .enumerate_regular_names(ManagedUserArea::Output)
                .unwrap();
            if case == "collision" {
                assert_eq!(std::fs::read(f.output()).unwrap(), b"sentinel");
                assert_eq!(names.len(), 1);
            } else {
                assert!(names.is_empty(), "{case}");
            }
            assert!(f
                .index
                .find_file_by_path_for_namespace(
                    &f.authority,
                    ManagedUserArea::Output,
                    &format!("{DELIVERY_FILE}.png")
                )
                .unwrap()
                .is_none());
            assert!(durable_json(&f.repo, f.authority.lease()).is_none());
            assert!(f
                .authority
                .enumerate_regular_names(ManagedUserArea::Previews)
                .unwrap()
                .is_empty());
        }
    }
    #[test]
    fn namespace_delivery_invalid_record_and_session_make_no_requests() {
        for case in [
            "stale-session",
            "foreign-owner",
            "foreign-auth",
            "missing",
            "mismatched",
            "missing-task",
            "video",
            "invalid-canvas-task",
            "invalid-index",
        ] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let mut f = DeliveryFixture::new(&url);
            let mut expected = f.record.identity();
            let mut item_index = 0;
            match case {
                "stale-session" => f.session.clear().unwrap(),
                "foreign-owner" => {
                    let mut row = f.record.clone();
                    row.owner_user_id = USER_B.into();
                    expected = row.identity();
                }
                "foreign-auth" => {
                    let mut row = f.record.clone();
                    row.auth_epoch += 1;
                    expected = row.identity();
                }
                "missing" => {
                    remove_pending_generation_for_namespace(&f.authority, &expected).unwrap();
                }
                "mismatched" => {
                    let mut row = f.record.clone();
                    row.billing_account_group_id = GROUP_B.into();
                    expected = row.identity();
                }
                "missing-task" => f.record.server_task_id.clear(),
                "video" => f.record.task_type = "video_generation".into(),
                "invalid-canvas-task" => {
                    // Canvas output is supported only for image_generation.
                    f.record.task_type = "image_edit".into();
                    f.record.canvas_source_node_id = "node".into();
                },
                "invalid-index" => item_index = 1,
                _ => unreachable!(),
            }
            if matches!(case, "missing-task" | "video" | "invalid-canvas-task") {
                upsert_pending_generation_for_namespace(&f.authority, &f.scope, f.record.clone())
                    .unwrap();
            }
            let before = f.recovery();
            let server = DeliveryServer::start(
                listener,
                &url,
                &f,
                delivery_detail(),
                DELIVERY_PNG.to_vec(),
                true,
            );
            let result = run_owned_worker(|| {
                prepare_namespace_delivery(
                    &f.api,
                    f.authority.clone(),
                    f.index.clone(),
                    &expected,
                    item_index,
                )
            });
            let requests = server.finish();
            assert!(result.is_err(), "{case}");
            assert!(requests.is_empty(), "{case}");
            assert_eq!(f.recovery(), before);
            assert!(f
                .authority
                .enumerate_regular_names(ManagedUserArea::Output)
                .unwrap()
                .is_empty());
        }
    }
    #[test]
    fn namespace_delivery_failed_card_keeps_identity_and_refuses_ambiguity() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        for case in ["success", "missing", "ambiguous", "mismatched"] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let mut f = DeliveryFixture::new(&url);
            f.record.deliveries = vec![PendingDeliveryRecord {
                item_index: 0,
                file_id: DELIVERY_FILE.into(),
                sha256: delivery_detail()["items"][0]["file"]["sha256"]
                    .as_str()
                    .unwrap()
                    .into(),
                size_bytes: 68,
                failed_asset_id: "failed-card".into(),
                ..Default::default()
            }];
            upsert_pending_generation_for_namespace(&f.authority, &f.scope, f.record.clone())
                .unwrap();
            let mut card = checked_asset("failed-card", "failed", "user edited failed prompt");
            card.conversation_id = "delivery-conversation".into();
            card.quality = "user edited quality".into();
            card.model = "image-model".into();
            card.reference_paths = vec!["user/edited/reference.png".into()];
            card.delivery_recoverable = true;
            card.delivery_downloading = true;
            let original = serde_json::to_value(stored_asset_from(&card)).unwrap();
            let mut store = Store::default();
            if case != "missing" {
                store.generations.push(card.clone());
            }
            if case == "ambiguous" {
                store.generations.push(card.clone());
            }
            if case == "mismatched" {
                store.generations[0].conversation_id = "other-conversation".into();
            }
            let before = replacement_memory_snapshot(&store);
            let server = DeliveryServer::start(
                listener,
                &url,
                &f,
                delivery_detail(),
                DELIVERY_PNG.to_vec(),
                true,
            );
            let result = (|| -> Result<_> {
                let prepared = f.prepare()?;
                let (_, id, committed) = persist_namespace_delivery(
                    &app,
                    &mut store,
                    &f.repo.writer,
                    prepared,
                    "fixture",
                )?;
                if case == "success" {
                    // A duplicate callback after replacement still uses the recorded failed-card ID.
                    drop(committed);
                    let (_, again, receipt) = persist_namespace_delivery(
                        &app,
                        &mut store,
                        &f.repo.writer,
                        f.prepare()?,
                        "retry",
                    )?;
                    run_owned_worker(|| acknowledge_namespace_delivery(receipt))?;
                    return Ok((id, Some(again)));
                }
                Ok((id, None))
            })();
            let requests = server.finish();
            if case == "success" {
                assert_eq!(
                    result.unwrap(),
                    ("failed-card".into(), Some("failed-card".into()))
                );
                assert_eq!(
                    (
                        store.assets.len(),
                        store.generations.len(),
                        store.notifications.len()
                    ),
                    (1, 1, 1)
                );
                let completed =
                    serde_json::to_value(stored_asset_from(&store.generations[0])).unwrap();
                for field in [
                    "id",
                    "prompt",
                    "conversation_id",
                    "reference_paths",
                    "cutout_done",
                    "remove_black_done",
                    "upscale_done",
                    "title",
                    "category",
                    "kind",
                    "quality",
                    "model",
                    "origin",
                ] {
                    assert_eq!(completed[field], original[field], "{field}");
                }
                assert!(
                    !store.generations[0].delivery_recoverable
                        && !store.generations[0].delivery_downloading
                );
            } else {
                assert!(result.is_err(), "{case}");
                assert_eq!(replacement_memory_snapshot(&store), before);
                assert!(!requests.iter().any(|r| r.0.starts_with("POST ")));
            }
        }
    }



    fn assert_canvas_owned_output(reject_first:bool) {
        i_slint_backend_testing::init_no_event_loop();let app=AppWindow::new().unwrap();
        let (listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new_with_canvas_source(&url,"original-canvas-source");let context=f.bound_context();
        let persistence=context.store.borrow().private_persistence.clone().unwrap();
        struct Drain(PrivatePersistence,AppContext);impl Drop for Drain{fn drop(&mut self){
            {let mut active=self.1.active_namespace.lock().unwrap();if active.as_ref()==Some(self.0.lease()){*active=None;}}
            let delivery=drain_delivery_commit_workers_for_lease_for_test(self.0.lease());
            let preview=drain_activation_preview_workers_for_lease_for_test(self.0.lease());
            let quiet=self.1.user_activity.begin_quiesce(self.0.lease());
            if !std::thread::panicking(){delivery.unwrap();preview.unwrap();quiet.unwrap();}
        }}
        let _drain=Drain(persistence.clone(),context.clone());
        {
            let mut store=context.store.borrow_mut();store.active_canvas_workspace_id="canvas-A".into();
            store.canvas_notes.push(CanvasNoteData{id:"original-canvas-source".into(),kind:"note".into(),content:"original source".into(),
                x:10.0,y:20.0,width:200.0,height:150.0,..Default::default()});
        }
        app.global::<AppState>().set_page("canvas".into());
        persistence.save_store(local_store_data(&app,&context.store.borrow())).unwrap();
        let writer=f.repo.writer.clone();let lease=persistence.lease().clone();let acked=Arc::new(AtomicBool::new(false));let seen=acked.clone();
        let server=DeliveryServer::start_controlled(listener,&url,&f,delivery_detail(),DELIVERY_PNG.to_vec(),true,1,Some(Box::new(move||{
            let saved=writer.load_client_state_for_namespace(&lease).unwrap().unwrap();
            assert_eq!(saved.active_canvas_workspace_id,"canvas-B");
            assert_eq!(saved.assets.len(),1);assert_eq!(saved.assets[0].category,"other");assert!(saved.generations.is_empty());
            assert_eq!(saved.canvas_workspaces["canvas-A"].notes.len(),2);
            assert!(saved.canvas_workspaces["canvas-A"].notes.iter().any(|note|note.image_path==saved.assets[0].source_path));
            assert_eq!(saved.canvas_notes.len(),1);assert_eq!(saved.canvas_notes[0].id,"unrelated-B");
            seen.store(true,Ordering::Release);
        })));
        let authority=persistence.storage_authority().unwrap();
        let prepared=run_owned_worker(||prepare_runtime_image_delivery(&f.api,authority,&f.record.client_request_id,0))
            .unwrap().expect("actual Canvas output must use namespace proof, not raw staging fallback");
        {
            let mut store=context.store.borrow_mut();switch_canvas_workspace(&mut store,"original prompt","canvas-B");
            store.canvas_notes.push(CanvasNoteData{id:"unrelated-B".into(),content:"later workspace".into(),..Default::default()});
        }
        if reject_first {f.repo.connection().execute_batch("CREATE TRIGGER reject_canvas_output BEFORE INSERT ON assets BEGIN SELECT RAISE(ABORT,'controlled output failure'); END;").unwrap();}
        let completed=Rc::new(RefCell::new(None));let observed=completed.clone();
        start_image_delivery_commit(&app,context.clone(),prepared,"fixture".into(),move|_,result|{*observed.borrow_mut()=Some(result.is_ok());});
        pump_delivery_fixture(||completed.borrow().is_some());
        if reject_first {
            assert_eq!(*completed.borrow(),Some(false));assert!(!acked.load(Ordering::Acquire));
            let saved=f.repo.load_client_state_for_namespace(persistence.lease()).unwrap().unwrap();
            assert!(saved.assets.is_empty());assert_eq!(saved.canvas_notes.len(),1);
            assert_eq!(context.store.borrow().canvas_workspaces["canvas-A"].notes.len(),2,"failed ack retains exact staged node");
            assert!(f.output().is_file());
            f.repo.connection().execute_batch("DROP TRIGGER reject_canvas_output").unwrap();
            context.store.borrow_mut().custom_prompts.push("later edit survives retry".into());
            let authority=persistence.storage_authority().unwrap();
            let prepared=run_owned_worker(||prepare_runtime_image_delivery(&f.api,authority,&f.record.client_request_id,0)).unwrap().unwrap();
            *completed.borrow_mut()=None;let observed=completed.clone();
            start_image_delivery_commit(&app,context.clone(),prepared,"retry".into(),move|_,result|{*observed.borrow_mut()=Some(result.is_ok());});
            pump_delivery_fixture(||completed.borrow().is_some());
            assert!(f.repo.load_client_state_for_namespace(persistence.lease()).unwrap().unwrap().custom_prompts.contains(&"later edit survives retry".into()));
        }
        assert_eq!(*completed.borrow(),Some(true));assert!(acked.load(Ordering::Acquire));
        assert_eq!(context.store.borrow().canvas_workspaces["canvas-A"].notes.len(),2);
        assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        assert_eq!(std::fs::read(f.output()).unwrap(),DELIVERY_PNG);
        let requests=server.finish();assert_eq!(requests.iter().filter(|(head,_)|head.starts_with("POST ")).count(),1);
    }
    #[test]
    fn core_canvas_actual_owned_output_commits_original_workspace_before_remote_ack(){assert_canvas_owned_output(false);}
    #[test]
    fn core_canvas_actual_owned_output_failed_ack_retries_current_store_without_duplicate_nodes(){assert_canvas_owned_output(true);}

    #[test]
    fn core_canvas_first_success_item_one_fills_original_placeholder_before_item_zero() {
        i_slint_backend_testing::init_no_event_loop();let app=AppWindow::new().unwrap();
        let (listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new_with_canvas_count(&url,"original-canvas-source",2);let context=f.bound_context();
        let persistence=context.store.borrow().private_persistence.clone().unwrap();
        struct Drain(PrivatePersistence,AppContext);impl Drop for Drain{fn drop(&mut self){
            {let mut active=self.1.active_namespace.lock().unwrap();if active.as_ref()==Some(self.0.lease()){*active=None;}}
            let delivery=drain_delivery_commit_workers_for_lease_for_test(self.0.lease());
            let preview=drain_activation_preview_workers_for_lease_for_test(self.0.lease());
            let quiet=self.1.user_activity.begin_quiesce(self.0.lease());
            if !std::thread::panicking(){delivery.unwrap();preview.unwrap();quiet.unwrap();}
        }}
        let _drain=Drain(persistence.clone(),context.clone());
        context.store.borrow_mut().canvas_notes.push(CanvasNoteData{id:"original-canvas-source".into(),kind:"image".into(),
            x:10.0,y:20.0,width:340.0,height:250.0,..Default::default()});
        context.generations.active.borrow_mut().insert("scene".into(),ActiveGeneration{
            task_id:f.record.local_task_id.clone(),destination:GenerationDestination::Canvas{source_node_id:"original-canvas-source".into()},
            session_scope:f.scope.request.session.clone(),..Default::default()});
        let state=app.global::<AppState>();state.set_page("canvas".into());state.set_canvas_generation_loading_node_id("original-canvas-source".into());
        persistence.save_store(local_store_data(&app,&context.store.borrow())).unwrap();
        const OTHER_FILE:&str="33333333-3333-4333-8333-333333333333";
        let mut detail=delivery_detail();detail["requested_count"]=2.into();detail["success_count"]=2.into();
        let mut second=detail["items"][0].clone();second["index"]=1.into();second["file"]["id"]=OTHER_FILE.into();
        second["file"]["download_url"]=format!("{url}blob").into();detail["items"].as_array_mut().unwrap().push(second);
        let server=DeliveryServer::start_controlled(listener,&url,&f,detail,DELIVERY_PNG.to_vec(),true,2,None);
        let first_path=f.authority.lease().namespace.path(ManagedUserArea::Output).join(format!("{OTHER_FILE}.png"));
        for index in [1,0] {
            let prepared=run_owned_worker(||prepare_runtime_image_delivery(&f.api,persistence.storage_authority().unwrap(),&f.record.client_request_id,index)).unwrap().unwrap();
            let done=Rc::new(RefCell::new(None));let observed=done.clone();
            start_image_delivery_commit_captured(&app,context.clone(),persistence.clone(),prepared,"fixture".into(),
                move|_,result|{*observed.borrow_mut()=Some(result.map(|(_,_,ack)|ack));});
            pump_delivery_fixture(||done.borrow().is_some());
            assert_eq!(done.borrow_mut().take().unwrap().unwrap(),true);
            let saved=f.repo.load_client_state_for_namespace(persistence.lease()).unwrap().unwrap();
            assert_eq!(saved.canvas_notes.len(),if index==1{1}else{2});
            assert!(saved.canvas_notes.iter().all(|note|note.kind!="image" || !note.image_path.is_empty()));
            assert_eq!(saved.canvas_notes.iter().find(|note|note.id=="original-canvas-source").unwrap().image_path,first_path.to_str().unwrap());
            assert_eq!(state.get_canvas_generation_loading_node_id(),"");
        }
        let saved=f.repo.load_client_state_for_namespace(persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.assets.len(),2);assert!(saved.assets.iter().all(|asset|asset.category=="other"));assert!(saved.generations.is_empty());
        assert_eq!(saved.canvas_links.len(),1);assert_eq!(std::fs::read(first_path).unwrap(),DELIVERY_PNG);
        assert_eq!(std::fs::read(f.output()).unwrap(),DELIVERY_PNG);assert!(load_pending_generations_for_namespace(&f.authority).unwrap().is_empty());
        let requests=server.finish();assert_eq!(requests.iter().filter(|(request,_)|request.starts_with("POST ")).count(),2);
    }

    struct DeliveryWriterResume(Arc<AtomicBool>);
    impl Drop for DeliveryWriterResume {
        fn drop(&mut self){self.0.store(false,Ordering::SeqCst);}
    }
    struct DeliveryFamilyDrain;
    impl Drop for DeliveryFamilyDrain {
        fn drop(&mut self){
            let delivery=drain_delivery_commit_workers_for_shutdown();
            let previews=drain_activation_preview_workers_for_shutdown();
            if !std::thread::panicking(){
                assert!(delivery.is_ok(),"delivery fixture workers failed to join");
                assert!(previews.is_ok(),"delivery preview fixture workers failed to join");
            }
        }
    }
    fn pump_delivery_fixture(mut ready:impl FnMut()->bool) {
        let deadline=Instant::now()+Duration::from_secs(5);
        while !ready() && Instant::now()<deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ready(),"actual delivery worker completion missing");
    }

    #[test]
    fn core_delivery_registered_worker_panic_is_sticky_after_actual_reap_and_empty_shutdown() {
        let (_listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new(&url);let context=f.bound_context();
        let persistence=context.store.borrow().private_persistence.clone().unwrap();
        let (cancel,receiver)=spawn_delivery_preparation::<()>(&persistence,|_,_,_|panic!("controlled delivery worker panic")).unwrap();
        assert!(receiver.recv_timeout(Duration::from_secs(3)).is_err());
        let deadline=Instant::now()+Duration::from_secs(3);
        while delivery_preparation_pending(&cancel) && Instant::now()<deadline{std::thread::yield_now();}
        assert!(!delivery_preparation_pending(&cancel));
        assert!(drain_delivery_commit_workers_for_shutdown().is_err());
        assert!(drain_delivery_commit_workers_for_shutdown().is_err());
    }
    #[test]
    fn core_delivery_after_send_panic_rejects_new_work_after_actual_reap() {
        let (_listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new(&url);let context=f.bound_context();
        let persistence=context.store.borrow().private_persistence.clone().unwrap();
        set_delivery_preparation_after_send_for_test(||panic!("controlled delivery after-send panic"));
        let (cancel,receiver)=spawn_delivery_preparation(&persistence,|_,_,_|Ok(7usize)).unwrap();
        assert_eq!(receiver.recv_timeout(Duration::from_secs(3)).unwrap().unwrap(),7);
        let deadline=Instant::now()+Duration::from_secs(3);
        while delivery_preparation_pending(&cancel) && Instant::now()<deadline {std::thread::yield_now();}
        assert!(!delivery_preparation_pending(&cancel));
        let successor=spawn_delivery_preparation::<()>(&persistence,|_,_,_|Ok(()));
        let admitted=successor.is_ok();
        drop(successor);
        // Join every actual handle before any assertion can unwind the fixture.
        let drained=drain_delivery_commit_workers_for_lease_for_test(persistence.lease());
        assert!(drained.is_err(),"actual after-send panic must remain sticky");
        assert!(finish_delivery_preparation(&cancel).is_err(),"sent success cannot hide the actual worker failure");
        assert!(!admitted,"a reaped after-send panic must reject new preparation work");
    }
    #[test]
    fn core_delivery_after_send_success_remains_pending_until_real_worker_exit() {
        let (_listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new(&url);let context=f.bound_context();
        let persistence=context.store.borrow().private_persistence.clone().unwrap();
        let (entered_tx,entered_rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();
        struct ReleaseAndDrain(Option<mpsc::Sender<()>>,NamespaceLease);
        impl Drop for ReleaseAndDrain {fn drop(&mut self) {
            if let Some(release)=self.0.take(){let _=release.send(());}
            let joined=drain_delivery_commit_workers_for_lease_for_test(&self.1);
            if !std::thread::panicking(){joined.unwrap();}
        }}
        let _release=ReleaseAndDrain(Some(release_tx),persistence.lease().clone());
        set_delivery_preparation_after_send_for_test(move|| {
            entered_tx.send(()).unwrap();let _=release_rx.recv();
        });
        let(cancel,receiver)=spawn_delivery_preparation(&persistence,|_,_,_|Ok(9usize)).unwrap();
        assert_eq!(receiver.recv_timeout(Duration::from_secs(3)).unwrap().unwrap(),9);
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(delivery_preparation_pending(&cancel),"sent success is not a joined worker");
        assert!(finish_delivery_preparation(&cancel).unwrap(),"checked completion still owns the held worker");
    }
    #[test]
    fn core_delivery_window_loss_still_drains_actual_writer_ack_worker_before_owned_root_drop() {
        i_slint_backend_testing::init_no_event_loop();
        let app=AppWindow::new().unwrap();
        let (listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new(&url);let context=f.bound_context();
        let server=DeliveryServer::start(listener,&url,&f,delivery_detail(),DELIVERY_PNG.to_vec(),true);
        let _drain=DeliveryFamilyDrain;
        let prepared=f.prepare().unwrap();
        f.repo.private_paused.store(true,Ordering::SeqCst);
        let _resume=DeliveryWriterResume(f.repo.private_paused.clone());
        let visible=Rc::new(Cell::new(false));let observed=visible.clone();
        start_image_delivery_commit(&app,context.clone(),prepared,"fixture".into(),move|_,_|observed.set(true));
        drop(app);
        f.repo.resume_private_writer();
        drain_delivery_commit_workers_for_shutdown().unwrap();
        assert!(!visible.get());
        assert!(f.repo.load_client_state_for_namespace(f.authority.lease()).unwrap().is_some());
        assert_eq!(std::fs::read(f.output()).unwrap(),DELIVERY_PNG);
        server.finish();
    }
    #[test]
    fn core_delivery_actual_ordered_worker_waits_for_ack_and_preserves_newer_store_snapshot() {
        i_slint_backend_testing::init_no_event_loop();
        let app=AppWindow::new().unwrap();
        let (listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new(&url);let context=f.bound_context();
        let server=DeliveryServer::start(listener,&url,&f,delivery_detail(),DELIVERY_PNG.to_vec(),true);
        let _drain=DeliveryFamilyDrain;
        let prepared=f.prepare().unwrap();
        f.repo.private_paused.store(true,Ordering::SeqCst);
        let _resume=DeliveryWriterResume(f.repo.private_paused.clone());
        let completed=Rc::new(Cell::new(false));let observed=completed.clone();
        start_image_delivery_commit(&app,context.clone(),prepared,"first".into(),move|_,result|{
            assert!(result.unwrap().2,"remote ack must follow actual Store ack");observed.set(true);
        });
        assert!(!completed.get());
        assert_eq!(context.store.borrow().assets.len(),1);
        assert!(f.repo.load_client_state_for_namespace(f.authority.lease()).unwrap().is_none());
        let persistence=context.store.borrow().private_persistence.clone().unwrap();
        let mut write=Some(persistence.prepare_ordered_save().unwrap());
        let later=context.apply_user_completion(persistence.lease(),||{
            context.store.borrow_mut().assets[0].title="newer edit".into();
            write.take().unwrap().enqueue(local_store_data(&app,&context.store.borrow()))
        }).unwrap().unwrap();
        drop(later);
        f.repo.resume_private_writer();
        pump_delivery_fixture(||completed.get());
        f.repo.flush(f.authority.lease()).unwrap();
        let saved=f.repo.load_client_state_for_namespace(f.authority.lease()).unwrap().unwrap();
        assert_eq!(saved.assets.iter().find(|asset|asset.id==DELIVERY_FILE).unwrap().title,"newer edit");
        // The exact terminal row is removed only after the real remote ack.
        assert_eq!(f.recovery(), serde_json::json!([]));
        let requests=server.finish();
        let acknowledgments=requests.iter().filter(|request|request.0.starts_with("POST ")).collect::<Vec<_>>();
        assert_eq!(acknowledgments.len(),1);
        assert!(acknowledgments[0].1);
        drain_delivery_commit_workers_for_shutdown().unwrap();
    }
    fn delivery_index_rows(f: &DeliveryFixture, sql: &str) -> Vec<Vec<rusqlite::types::Value>> {
        let c = Connection::open(f.repo.directory.path().join("index.sqlite3")).unwrap();
        let mut statement = c.prepare(sql).unwrap();
        let count = statement.column_count();
        statement.query_map([], |row| (0..count).map(|column| row.get(column)).collect())
            .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }
    const DELIVERY_INDEX_ROWS: &str = "SELECT id,user_public_id,managed_area,path,physical_identity,kind,byte_size,managed,retention_policy,created_at,last_accessed_at,pending_delete FROM managed_files ORDER BY id";
    #[test]
    fn multi_image_delivery_reconciliation_allows_sibling_acknowledgment() {
        let (listener, url) = backend_generation::billing_capture_test_support::listener();
        let f = DeliveryFixture::new_with_canvas_count(&url, "", 2);
        let mut detail = delivery_detail();
        detail["requested_count"] = 2.into();
        detail["success_count"] = 2.into();
        let mut sibling = detail["items"][0].clone();
        sibling["index"] = 1.into();
        sibling["file"]["id"] = USER_B.into();
        detail["items"].as_array_mut().unwrap().push(sibling);
        let server = DeliveryServer::start(listener, &url, &f, detail, DELIVERY_PNG.to_vec(), true);
        let result = run_owned_worker(|| {
            let authority = f.authority.clone();
            let identity = f.record.identity();
            file_index::FileIndex::delivery_reconcile_after_discovery_for_test(move || {
                let sibling = DeliveryConfirmation {
                    client_request_id: "delivery-request".into(), item_index: 1,
                    task_id: DELIVERY_TASK.into(), file_id: USER_B.into(),
                    sha256: "431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460".into(),
                    size_bytes: 68, failed_asset_id: None,
                };
                assert!(pending_delivery_saved_for_namespace(&authority, &identity, &sibling, "fixture/sibling.png").unwrap());
                assert!(apply_generation_patch_for_namespace(&authority, &identity,
                    GenerationRecoveryPatch::Terminal { expected_success_count: 2 }).unwrap());
                assert!(pending_delivery_acknowledged_for_namespace(&authority, &identity, USER_B).unwrap());
            });
            prepare_namespace_delivery(&f.api, f.authority.clone(), f.index.clone(), &f.record.identity(), 0)
        });
        let requests = server.finish();
        let prepared = result.unwrap_or_else(|error| panic!("a sibling acknowledgment must not invalidate this image: {error}"));
        assert_eq!(prepared.confirmation().item_index, 0);
        assert_eq!(std::fs::read(f.output()).unwrap(), DELIVERY_PNG);
        assert_eq!(delivery_index_rows(&f, DELIVERY_INDEX_ROWS).len(), 1);
        let records = load_pending_generations_for_namespace(&f.authority).unwrap();
        assert_eq!(records[0].deliveries.len(), 1);
        assert!(records[0].deliveries[0].acknowledged);
        assert!(!requests.iter().any(|request| request.0.starts_with("POST ")));
    }
    #[test]
    fn multi_image_delivery_reconciliation_rejects_current_item_and_task_changes() {
        for change in ["current-item", "current-item-path", "prompt", "payer"] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let f = DeliveryFixture::new(&url);
            let server = DeliveryServer::start(listener, &url, &f, delivery_detail(), DELIVERY_PNG.to_vec(), true);
            let result = run_owned_worker(|| {
                let authority = f.authority.clone();
                let identity = f.record.identity();
                file_index::FileIndex::delivery_reconcile_after_discovery_for_test(move || {
                    if change.starts_with("current-item") {
                        let replacement = DeliveryConfirmation {
                            client_request_id: "delivery-request".into(), item_index: 0,
                            task_id: DELIVERY_TASK.into(), file_id: if change == "current-item-path" { DELIVERY_FILE } else { USER_B }.into(),
                            sha256: "431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460".into(),
                            size_bytes: 68, failed_asset_id: None,
                        };
                        assert!(pending_delivery_saved_for_namespace(&authority, &identity, &replacement, "fixture/replacement.png").unwrap());
                    } else {
                        let file = authority.lease().namespace.path(ManagedUserArea::Recovery).join("pending-generations.json");
                        let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
                        if change == "prompt" { document["generations"][0]["raw_prompt"] = "changed fixture prompt".into(); }
                        else { document["generations"][0]["billing_account_group_id"] = GROUP_B.into(); }
                        std::fs::write(&file, serde_json::to_vec(&document).unwrap()).unwrap();
                    }
                });
                prepare_namespace_delivery(&f.api, f.authority.clone(), f.index.clone(), &f.record.identity(), 0)
            });
            let requests = server.finish();
            assert!(result.is_err(), "{change} must invalidate the prepared image");
            assert!(delivery_index_rows(&f, DELIVERY_INDEX_ROWS).is_empty());
            assert!(!requests.iter().any(|request| request.0.starts_with("POST ")));
        }
    }
    #[test]
    fn core_delivery_index_interrupted_publication_reconciles_original_content_without_losing_links() {
        let (listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new(&url);
        let server=DeliveryServer::start(listener,&url,&f,delivery_detail(),DELIVERY_PNG.to_vec(),true);
        let original=f.prepare().unwrap(); // Keep its inode held while the directory entry is replaced.
        let original_record=f.index.find_file_by_path_for_namespace(&f.authority,ManagedUserArea::Output,
            &format!("{DELIVERY_FILE}.png")).unwrap().unwrap();
        let preview=run_owned_worker(|| {
            let key=ManagedFileKey::new(ManagedUserArea::Previews,"delivery-link-preview.png").unwrap();
            let mut file=f.authority.create_new_regular(&key).unwrap();
            f.authority.write_new_regular_from(&mut file,&mut &DELIVERY_PNG[..]).unwrap();
            f.authority.sync_regular(&mut file).unwrap();
            let registration=NamespacedManagedFileRegistration::new(&f.authority,file,"preview","cache").unwrap();
            f.index.register_file_for_namespace(&f.authority,&registration).unwrap()
        });
        let c=Connection::open(f.repo.directory.path().join("index.sqlite3")).unwrap();
        c.execute("INSERT INTO file_references(user_public_id,file_id,owner_type,owner_id,created_at) VALUES(?1,?2,'asset','original-asset',7)",
            params![USER_A,original_record.id.0]).unwrap();
        c.execute("INSERT INTO preview_cache(user_public_id,source_file_id,preview_file_id,purpose,longest_edge,source_size,source_mtime_ns,cache_version,status,last_accessed_at,created_at,updated_at) VALUES(?1,?2,?3,'gallery',64,68,1,1,'ready',11,12,13)",
            params![USER_A,original_record.id.0,preview.id.0]).unwrap();
        let before=delivery_index_rows(&f,DELIVERY_INDEX_ROWS);
        let references=delivery_index_rows(&f,"SELECT * FROM file_references ORDER BY file_id");
        let previews=delivery_index_rows(&f,"SELECT * FROM preview_cache ORDER BY id");
        let recovery=f.recovery();
        std::fs::remove_file(f.output()).unwrap();
        c.execute_batch("CREATE TRIGGER reject_delivery_reconcile BEFORE UPDATE OF physical_identity ON managed_files BEGIN SELECT RAISE(ABORT,'fixture interrupted index commit'); END;").unwrap();
        assert!(f.prepare().is_err());
        assert_eq!(std::fs::read(f.output()).unwrap(),DELIVERY_PNG,
            "verified publication must remain recoverable after failed index commit");
        assert_eq!(delivery_index_rows(&f,DELIVERY_INDEX_ROWS),before);
        c.execute_batch("DROP TRIGGER reject_delivery_reconcile").unwrap();
        let recovered=f.prepare().unwrap();
        let current=f.index.find_file_by_path_for_namespace(&f.authority,ManagedUserArea::Output,
            &format!("{DELIVERY_FILE}.png")).unwrap().unwrap();
        assert_eq!(current.id,original_record.id);
        assert_ne!(current.physical_identity,original_record.physical_identity);
        let mut after=delivery_index_rows(&f,DELIVERY_INDEX_ROWS);
        assert_eq!(after.len(),before.len());after[0][4]=before[0][4].clone();
        assert_eq!(after,before,"only physical identity changed; retention and timestamps are preserved");
        assert_eq!(delivery_index_rows(&f,"SELECT * FROM file_references ORDER BY file_id"),references);
        assert_eq!(delivery_index_rows(&f,"SELECT * FROM preview_cache ORDER BY id"),previews);
        assert_eq!(f.recovery(),recovery);
        drop(recovered);drop(original);
        let requests=server.finish();
        assert_eq!(requests.iter().filter(|request|request.0.starts_with("GET /blob ")).count(),2);
        assert!(!requests.iter().any(|request|request.0.starts_with("POST ")),
            "index reconciliation alone is not a Store or remote acknowledgment");
    }
    #[test]
    fn core_delivery_index_reconcile_refuses_pending_delete_foreign_kind_and_physical_alias() {
        for invalid in ["pending_delete=1","kind='reference'","retention_policy='cache'","managed=0"] {
            let (listener,url)=backend_generation::billing_capture_test_support::listener();
            let f=DeliveryFixture::new(&url);
            let server=DeliveryServer::start(listener,&url,&f,delivery_detail(),DELIVERY_PNG.to_vec(),true);
            let original=f.prepare().unwrap();
            let c=Connection::open(f.repo.directory.path().join("index.sqlite3")).unwrap();
            c.execute_batch(&format!("UPDATE managed_files SET {invalid}")).unwrap();
            let before=delivery_index_rows(&f,DELIVERY_INDEX_ROWS);
            assert!(f.prepare().is_err(),"{invalid} must not be implicitly repaired");
            assert_eq!(delivery_index_rows(&f,DELIVERY_INDEX_ROWS),before);
            drop(original);server.finish();
        }
        let (listener,url)=backend_generation::billing_capture_test_support::listener();
        let f=DeliveryFixture::new(&url);
        let server=DeliveryServer::start(listener,&url,&f,delivery_detail(),DELIVERY_PNG.to_vec(),true);
        let original=f.prepare().unwrap();
        let c=Connection::open(f.repo.directory.path().join("index.sqlite3")).unwrap();
        c.execute("UPDATE managed_files SET path='different-logical-file.png'",[]).unwrap();
        let before=delivery_index_rows(&f,DELIVERY_INDEX_ROWS);
        assert!(f.prepare().is_err(),"one physical identity cannot be rebound from another logical row");
        assert_eq!(delivery_index_rows(&f,DELIVERY_INDEX_ROWS),before);
        drop(original);server.finish();
    }
    #[test]
    fn core_delivery_index_reconcile_cas_refuses_changed_row_and_original_payer_or_stale_lease() {
        for change in ["row","payer","task","content","lease"] {
            let (listener,url)=backend_generation::billing_capture_test_support::listener();
            let f=DeliveryFixture::new(&url);
            let server=DeliveryServer::start(listener,&url,&f,delivery_detail(),DELIVERY_PNG.to_vec(),true);
            let original=f.prepare().unwrap();
            let before=delivery_index_rows(&f,DELIVERY_INDEX_ROWS);
            std::fs::remove_file(f.output()).unwrap();
            let path=f.repo.directory.path().join("index.sqlite3");
            let authority=f.authority.clone();
            let session=f.session.clone();
            let result=run_owned_worker(|| {
                FileIndex::delivery_reconcile_after_discovery_for_test(move || {
                    match change {
                        "row" => {
                            let c=Connection::open(path).unwrap();
                            c.execute("UPDATE managed_files SET last_accessed_at=last_accessed_at+1",[]).unwrap();
                        }
                        "payer" | "task" => {
                            // Controlled corruption of this fixture's own retained document.
                            // No mutation authority is minted from the changed row.
                            let file=authority.lease().namespace.path(ManagedUserArea::Recovery)
                                .join("pending-generations.json");
                            let mut value:serde_json::Value=serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
                            if change=="payer" { value["generations"][0]["billing_account_group_id"]=GROUP_B.into(); }
                            else { value["generations"][0]["server_task_id"]=USER_B.into(); }
                            std::fs::write(&file,serde_json::to_vec(&value).unwrap()).unwrap();
                        }
                        "content" => {
                            let file=authority.lease().namespace.path(ManagedUserArea::Output).join(format!("{DELIVERY_FILE}.png"));
                            std::fs::write(file,vec![0_u8;DELIVERY_PNG.len()]).unwrap();
                        }
                        _ => { session.clear().unwrap(); }
                    }
                });
                prepare_namespace_delivery(&f.api,f.authority.clone(),f.index.clone(),&f.record.identity(),0)
            });
            assert!(result.is_err());
            let after=delivery_index_rows(&f,DELIVERY_INDEX_ROWS);
            assert_eq!(after[0][4],before[0][4],"CAS must not update the old physical row");
            if change=="row" {assert_ne!(after,before);} else {assert_eq!(after,before);}
            if change!="content" { assert_eq!(std::fs::read(f.output()).unwrap(),DELIVERY_PNG); }
            drop(original);server.finish();
        }
    }
    #[test]
    fn core_delivery_actual_retry_uses_owned_pipeline_for_existing_and_missing_output() {
        assert_actual_retry_owned_output(false);
    }
    #[test]
    fn core_delivery_actual_retry_replaces_missing_owned_output_without_rebinding_payer() {
        assert_actual_retry_owned_output(true);
    }
    fn assert_actual_retry_owned_output(missing:bool) {
        i_slint_backend_testing::init_no_event_loop();
        let app=AppWindow::new().unwrap();
        {
            let (listener,url)=backend_generation::billing_capture_test_support::listener();
            let mut f=DeliveryFixture::new(&url);
            f.record.deliveries=vec![PendingDeliveryRecord{item_index:0,file_id:DELIVERY_FILE.into(),
                sha256:"431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460".into(),
                size_bytes:68,failed_asset_id:"failed-card".into(),..Default::default()}];
            upsert_pending_generation_for_namespace(&f.authority,&f.scope,f.record.clone()).unwrap();
            let context=f.bound_context();
            let mut card=checked_asset("failed-card","failed","user edited prompt");
            card.conversation_id="delivery-conversation".into();card.model="image-model".into();
            card.delivery_recoverable=true;card.delivery_downloading=false;
            context.store.borrow_mut().generations.push(card);
            let server=DeliveryServer::start(listener,&url,&f,delivery_detail(),DELIVERY_PNG.to_vec(),true);
            let _drain=DeliveryFamilyDrain;
            drop(f.prepare().unwrap());
            if missing {std::fs::remove_file(f.output()).unwrap();}
            retry_failed_delivery(&app,context.clone(),"failed-card".into());
            pump_delivery_fixture(||f.recovery()==serde_json::json!([])
                && !context.store.borrow().generations[0].delivery_downloading);
            let store=context.store.borrow();
            assert_eq!(store.generations[0].id,"failed-card");
            assert_eq!(store.generations[0].prompt,"user edited prompt");
            assert_eq!(store.assets.len(),1);
            assert_eq!(std::fs::read(f.output()).unwrap(),DELIVERY_PNG);
            drop(store);
            let requests=server.finish();
            let acknowledgments=requests.iter().filter(|request|request.0.starts_with("POST ")).collect::<Vec<_>>();
            assert_eq!(acknowledgments.len(),1);
            assert!(acknowledgments[0].1);
        }
        drain_delivery_commit_workers_for_shutdown().unwrap();
    }
    #[test]
    fn core_prompt_checked_owned_upload_rejects_changed_retained_fingerprint_before_transport() {
        for wrong_size in [false, true] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let f = DeliveryFixture::new(&url);
            let server = DeliveryServer::start(listener, &url, &f, delivery_detail(), DELIVERY_PNG.to_vec(), true);
            drop(f.prepare().unwrap());
            let hash = if wrong_size { "431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460" }
                else { "0000000000000000000000000000000000000000000000000000000000000000" };
            let result = run_owned_worker(|| f.api.upload_reference_for_namespace_checked(
                &f.output(), &f.authority, &f.scope.request.session, false, hash, if wrong_size { 69 } else { 68 }));
            assert!(result.is_err());
            let requests = server.finish();
            assert!(!requests.iter().any(|request| request.0.starts_with("POST ")),
                "changed original bytes must be rejected before upload preparation");
            assert_eq!(std::fs::read(f.output()).unwrap(), DELIVERY_PNG);
        }
    }

    #[test]
    fn core_prompt_checked_owned_upload_keeps_normal_and_paired_bytes_and_exact_upgrade() {
        use std::io::Write;
        use sha2::Digest;
        for (paired,upgrade) in [(false,false),(true,false),(false,true)] {
            let (listener,url)=backend_generation::billing_capture_test_support::listener();
            let f=DeliveryFixture::new(&url);
            let key=ManagedFileKey::new(ManagedUserArea::Output,&format!("{DELIVERY_FILE}.png")).unwrap();
            let mut file=f.authority.create_new_regular(&key).unwrap();
            f.authority.write_new_regular_from(&mut file,&mut &DELIVERY_PNG[..]).unwrap();
            f.authority.sync_regular(&mut file).unwrap();drop(file);
            let (normalized,filename,mime)=prepare_reference_upload_bytes(DELIVERY_PNG.to_vec(),paired).unwrap();
            let base=url.clone();
            let stop=Arc::new(AtomicBool::new(false));let stopped=stop.clone();
            let worker=std::thread::spawn(move||{
                listener.set_nonblocking(true).unwrap();
                let deadline=Instant::now()+Duration::from_secs(10);let mut requests=Vec::new();
                while !stopped.load(Ordering::SeqCst) && Instant::now()<deadline {
                    let mut stream=match listener.accept(){
                        Ok((stream,_))=>stream,
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{std::thread::sleep(Duration::from_millis(2));continue;},
                        Err(error)=>panic!("fixture listener failed: {error}"),
                    };
                    let bytes=backend_generation::billing_capture_test_support::read_request_bytes(&mut stream);
                    let header_end=bytes.windows(4).position(|part|part==b"\r\n\r\n").unwrap()+4;
                    let headers=String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                    let prepare=headers.starts_with("POST /v1/uploads/references ");
                    let transfer=headers.starts_with("POST /fixture-upload ");
                    if prepare {
                        let request:serde_json::Value=serde_json::from_slice(&bytes[header_end..]).unwrap();
                        assert_eq!(request["filename"],filename);assert_eq!(request["mime_type"],mime);
                        assert_eq!(request["size_bytes"],normalized.len() as u64);
                        assert_eq!(request["sha256"],format!("{:x}",sha2::Sha256::digest(&normalized)));
                        assert!(!headers.to_ascii_lowercase().contains("x-account-group-id:"));
                    } else if transfer {
                        assert!(bytes[header_end..].windows(normalized.len()).any(|part|part==normalized.as_slice()));
                    } else {assert!(headers.starts_with(&format!("POST /v1/uploads/references/{DELIVERY_FILE}/complete ")));}
                    let (status,body)=if upgrade {
                        ("426 Upgrade Required",serde_json::json!({"data":null,"error":{"code":"client_upgrade_required","message":"upgrade"},"request_id":"fixture"}))
                    } else {
                        ("200 OK",serde_json::json!({"data":if prepare {serde_json::json!({"file":{"id":DELIVERY_FILE},
                            "upload":{"method":"POST","url":format!("{base}fixture-upload"),"fields":{},"file_field":"file"}})}
                            else {serde_json::json!({})},"error":null,"request_id":"fixture"}))
                    };
                    let body=serde_json::to_vec(&body).unwrap();
                    write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                    let _=stream.write_all(&body);requests.push((headers,false));
                }
                requests
            });
            let server=DeliveryServer{stop,worker:Some(worker)};
            let result=run_owned_worker(||f.api.upload_reference_for_namespace_checked(&f.output(),&f.authority,
                &f.scope.request.session,paired,"431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460",68));
            let requests=server.finish();
            if upgrade {
                assert!(result.unwrap_err().is_client_update_required());
                assert!(f.client.upgrade_latch().is_tripped());assert_eq!(requests.len(),1);
            } else {assert_eq!(result.unwrap(),DELIVERY_FILE);assert_eq!(requests.len(),3);}
            assert_eq!(std::fs::read(f.output()).unwrap(),DELIVERY_PNG);
        }
    }

    #[test]
    fn core_delivery_ordered_enqueue_rejects_missing_or_foreign_store_before_any_write() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        for foreign in [false, true] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let f = DeliveryFixture::new(&url);
            let activity = UserActivityGate::default();
            activity.activate(f.authority.lease().clone()).unwrap();
            let persistence = PrivatePersistence::for_test(f.repo.writer.clone(), f.authority.lease().clone(),
                activity, UpgradeLatch::default());
            let other = Fixture::new(false, false);
            let other_lease = other.lease(USER_B, 1, 1);
            other.activate(other_lease.clone()).unwrap();
            let other_activity = UserActivityGate::default();
            other_activity.activate(other_lease.clone()).unwrap();
            let mut store = Store::default();
            if foreign {
                store.private_persistence = Some(PrivatePersistence::for_test((*other).clone(),
                    other_lease.clone(), other_activity, UpgradeLatch::default()));
            }
            let before = replacement_memory_snapshot(&store);
            let server = DeliveryServer::start(listener, &url, &f, delivery_detail(), DELIVERY_PNG.to_vec(), true);
            let prepared = f.prepare().unwrap();
            let write = persistence.prepare_ordered_save().unwrap();
            // Pure enqueue is exercised under the real short completion latch;
            // the complete guard-owning failure is dropped after that latch.
            let result = persistence.upgrade_latch().apply_if_open(|| {
                write.enqueue_delivery(&app, &mut store, prepared, "fixture")
            });
            let result = result.unwrap();
            assert!(result.is_err(), "missing/foreign Store must not write A");
            drop(result);
            f.repo.flush(&f.authority.lease().clone()).unwrap();
            assert_eq!(replacement_memory_snapshot(&store), before);
            assert!(f.repo.load_client_state_for_namespace(f.authority.lease()).unwrap().is_none());
            assert!(other.load_client_state_for_namespace(&other_lease).unwrap().is_none());
            assert_eq!(std::fs::read(f.output()).unwrap(), DELIVERY_PNG);
            assert!(!server.finish().iter().any(|request| request.0.starts_with("POST ")));
        }
    }
    #[test]
    fn core_delivery_failed_sqlite_ack_preserves_staged_metadata_and_owned_file_for_retry() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (listener, url) = backend_generation::billing_capture_test_support::listener();
        let f = DeliveryFixture::new(&url);
        let before = f.recovery();
        let server = DeliveryServer::start(listener, &url, &f, delivery_detail(), DELIVERY_PNG.to_vec(), true);
        let mut store = Store::default();
        reject_notification_inserts(&f.repo);
        let result = (|| -> Result<_> {
            let prepared = f.prepare()?;
            let failed = persist_namespace_delivery(&app, &mut store, &f.repo.writer, prepared, "first");
            anyhow::ensure!(failed.is_err(), "fixture must refuse actual SQLite acknowledgment");
            // A different already-queued full snapshot may retain these references.
            // Failed acknowledgment cannot safely undo optimistic projection or remove bytes.
            let staged = store.assets.len() == 1 && store.generations.len() == 1 && store.notifications.len() == 1;
            let recovery = f.recovery();
            let bytes = std::fs::read(f.output())?;
            Ok((staged, recovery, bytes))
        })();
        let requests = server.finish();
        let (staged, recovery, bytes) = result.unwrap();
        assert!(staged, "ack failure must preserve staged Store metadata for an exact retry");
        assert_eq!(recovery, before);
        assert_eq!(bytes, DELIVERY_PNG);
        assert!(!requests.iter().any(|request| request.0.starts_with("POST ")));
    }
    #[test]
    fn namespace_delivery_sql_failure_retries_output_without_duplicate_metadata() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (listener, url) = backend_generation::billing_capture_test_support::listener();
        let f = DeliveryFixture::new(&url);
        let before = f.recovery();
        let server = DeliveryServer::start(
            listener,
            &url,
            &f,
            delivery_detail(),
            DELIVERY_PNG.to_vec(),
            true,
        );
        let mut store = Store::default();
        reject_notification_inserts(&f.repo);
        let result = (|| -> Result<_> {
            let prepared = f.prepare()?;
            let rejected =
                persist_namespace_delivery(&app, &mut store, &f.repo.writer, prepared, "first");
            let staged = rejected.is_err()
                && store.assets.len() == 1
                && store.generations.len() == 1
                && store.notifications.len() == 1;
            let recovery_after_failure = f.recovery();
            let published_after_failure = std::fs::read(f.output())?;
            f.repo
                .connection()
                .execute_batch("DROP TRIGGER reject_fixture_notification")?;
            let prepared = f.prepare()?;
            let (_, _, first) =
                persist_namespace_delivery(&app, &mut store, &f.repo.writer, prepared, "retry")?;
            store.assets[0].title = "user edit".into();
            store.notifications[0].read = true;
            let prepared = f.prepare()?;
            let (_, _, second) = persist_namespace_delivery(
                &app,
                &mut store,
                &f.repo.writer,
                prepared,
                "duplicate",
            )?;
            drop(first);
            run_owned_worker(|| acknowledge_namespace_delivery(second))?;
            Ok((staged, recovery_after_failure, published_after_failure))
        })();
        let requests = server.finish();
        let (staged, recovery_after_failure, published_after_failure) =
            result.expect("SQL failure must leave staged metadata and verified output reusable");
        assert!(staged);
        assert_eq!(recovery_after_failure, before);
        assert_eq!(published_after_failure, DELIVERY_PNG);
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.0.starts_with("GET /blob "))
                .count(),
            1
        );
        assert_eq!(
            requests.iter().filter(|r| r.0.starts_with("POST ")).count(),
            1
        );
        assert_eq!(
            (
                store.assets.len(),
                store.generations.len(),
                store.notifications.len()
            ),
            (1, 1, 1)
        );
        assert_eq!(store.assets[0].title, "user edit");
        assert!(store.notifications[0].read);
    }
    #[test]
    fn namespace_delivery_http_ack_retains_required_inputs() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (listener, url) = backend_generation::billing_capture_test_support::listener();
        let mut f = DeliveryFixture::new(&url);
        f.record.reference_paths = vec!["required-input".into()];
        f.record.reference_sha256 = vec!["input-hash".into()];
        f.record.reference_size_bytes = vec![7];
        upsert_pending_generation_for_namespace(&f.authority, &f.scope, f.record.clone()).unwrap();
        let server = DeliveryServer::start(
            listener,
            &url,
            &f,
            delivery_detail(),
            DELIVERY_PNG.to_vec(),
            true,
        );
        let mut store = Store::default();
        let result = (|| -> Result<_> {
            let (_, _, receipt) = persist_namespace_delivery(
                &app,
                &mut store,
                &f.repo.writer,
                f.prepare()?,
                "fixture",
            )?;
            Ok(run_owned_worker(|| {
                acknowledge_namespace_delivery(receipt)
            })?)
        })();
        let requests = server.finish();
        assert!(result.expect("input retention must not suppress HTTP ack"));
        assert_eq!(
            requests.iter().filter(|r| r.0.starts_with("POST ")).count(),
            1
        );
        let records = load_pending_generations_for_namespace(&f.authority).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].terminal && records[0].deliveries[0].acknowledged);
        assert_eq!(records[0].reference_paths, ["required-input"]);
        assert_eq!(records[0].reference_sha256, ["input-hash"]);
        assert_eq!(records[0].reference_size_bytes, [7]);
        assert_eq!(
            records[0].lineage_reference_paths,
            ["captured/reference.png"]
        );
    }
    #[test]
    fn namespace_delivery_ack_failure_and_partial_success_retain_saved_row() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        for partial in [false, true] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let mut f = DeliveryFixture::new(&url);
            let mut detail = delivery_detail();
            if partial {
                f.record.count = 2;
                upsert_pending_generation_for_namespace(&f.authority, &f.scope, f.record.clone())
                    .unwrap();
                detail["success_count"] = 2.into();
                detail["requested_count"] = 2.into();
                let mut second = detail["items"][0].clone();
                second["index"] = 1.into();
                second["file"]["id"] = "33333333-3333-4333-8333-333333333333".into();
                detail["items"].as_array_mut().unwrap().push(second);
            }
            let server =
                DeliveryServer::start(listener, &url, &f, detail, DELIVERY_PNG.to_vec(), partial);
            let mut store = Store::default();
            let result = (|| -> Result<_> {
                let (_, _, receipt) = persist_namespace_delivery(
                    &app,
                    &mut store,
                    &f.repo.writer,
                    f.prepare()?,
                    "fixture",
                )?;
                Ok(run_owned_worker(|| acknowledge_namespace_delivery(receipt)))
            })();
            let requests = server.finish();
            let ack = result.expect("preparation and metadata must succeed before ack outcome");
            if partial {
                assert!(ack.unwrap());
            } else {
                assert!(ack.is_err());
            }
            assert_eq!(
                requests.iter().filter(|r| r.0.starts_with("POST ")).count(),
                1
            );
            let records = load_pending_generations_for_namespace(&f.authority).unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].deliveries[0].acknowledged, partial);
            assert_eq!(
                records[0].deliveries[0].local_path,
                f.output().to_string_lossy()
            );
            assert_eq!(store.assets.len(), 1);
        }
    }
    #[test]
    fn namespace_delivery_ack_failure_retry_reuses_output_and_metadata() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (listener, url) = backend_generation::billing_capture_test_support::listener();
        let f = DeliveryFixture::new(&url);
        let server = DeliveryServer::start(
            listener,
            &url,
            &f,
            delivery_detail(),
            DELIVERY_PNG.to_vec(),
            false,
        );
        let mut store = Store::default();
        let result = (|| -> Result<_> {
            let (_, first_id, receipt) = persist_namespace_delivery(
                &app,
                &mut store,
                &f.repo.writer,
                f.prepare()?,
                "first",
            )?;
            let first = run_owned_worker(|| acknowledge_namespace_delivery(receipt));
            let after_failure = f.recovery();
            let (_, second_id, receipt) = persist_namespace_delivery(
                &app,
                &mut store,
                &f.repo.writer,
                f.prepare()?,
                "retry",
            )?;
            let second = run_owned_worker(|| acknowledge_namespace_delivery(receipt));
            Ok((first_id, second_id, first, second, after_failure))
        })();
        let requests = server.finish();
        let (first_id, second_id, first, second, after_failure) = result.unwrap();
        assert!(first.is_err() && second.unwrap());
        assert_eq!(first_id, second_id);
        assert_eq!(after_failure[0]["deliveries"][0]["acknowledged"], false);
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.0.starts_with("GET /blob "))
                .count(),
            1
        );
        assert_eq!(
            requests.iter().filter(|r| r.0.starts_with("POST ")).count(),
            2
        );
        assert_eq!(
            (
                store.assets.len(),
                store.generations.len(),
                store.notifications.len()
            ),
            (1, 1, 1)
        );
        assert_eq!(f.recovery(), serde_json::json!([]));
    }
    #[test]
    fn namespace_delivery_publication_race_revalidates_one_winner_without_overwrite() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        for winner in ["matching", "conflicting"] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let f = DeliveryFixture::new(&url);
            let mut detail = delivery_detail();
            detail["_fixture_winner"] = winner.into();
            let server =
                DeliveryServer::start(listener, &url, &f, detail, DELIVERY_PNG.to_vec(), true);
            let mut store = Store::default();
            let result = (|| -> Result<_> {
                let prepared = f.prepare()?;
                let (_, _, receipt) = persist_namespace_delivery(
                    &app,
                    &mut store,
                    &f.repo.writer,
                    prepared,
                    "fixture",
                )?;
                Ok(run_owned_worker(|| {
                    acknowledge_namespace_delivery(receipt)
                })?)
            })();
            let requests = server.finish();
            assert_eq!(
                f.authority
                    .enumerate_regular_names(ManagedUserArea::Output)
                    .unwrap()
                    .len(),
                1
            );
            if winner == "matching" {
                assert!(result.unwrap());
                assert_eq!(std::fs::read(f.output()).unwrap(), DELIVERY_PNG);
                assert_eq!(store.assets.len(), 1);
            } else {
                assert!(result.is_err());
                assert_eq!(std::fs::read(f.output()).unwrap(), b"collision sentinel");
                assert!(store.assets.is_empty());
                assert!(!requests.iter().any(|r| r.0.starts_with("POST ")));
            }
        }
    }
    #[test]
    fn namespace_delivery_saved_confirmation_and_terminal_counts_must_match() {
        for case in [
            "size",
            "hash",
            "index",
            "file",
            "ambiguous",
            "abandoned",
            "acknowledged",
            "negative",
            "impossible",
            "duplicate-item",
        ] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let mut f = DeliveryFixture::new(&url);
            let mut detail = delivery_detail();
            let mut delivery = PendingDeliveryRecord {
                item_index: 0,
                file_id: DELIVERY_FILE.into(),
                size_bytes: 68,
                sha256: detail["items"][0]["file"]["sha256"]
                    .as_str()
                    .unwrap()
                    .into(),
                ..Default::default()
            };
            match case {
                "size" => delivery.size_bytes = 7,
                "hash" => delivery.sha256 = "0".repeat(64),
                "index" => delivery.item_index = 1,
                "file" => delivery.file_id = USER_B.into(),
                "abandoned" => delivery.abandoned = true,
                "acknowledged" => delivery.acknowledged = true,
                "negative" => detail["success_count"] = (-1).into(),
                "impossible" => detail["success_count"] = 2.into(),
                "duplicate-item" => {
                    let first = detail["items"][0].clone();
                    detail["items"].as_array_mut().unwrap().push(first);
                }
                _ => {}
            }
            f.record.deliveries.push(delivery.clone());
            if case == "ambiguous" {
                f.record.deliveries.push(delivery);
            }
            upsert_pending_generation_for_namespace(&f.authority, &f.scope, f.record.clone())
                .unwrap();
            let before = f.recovery();
            let server =
                DeliveryServer::start(listener, &url, &f, detail, DELIVERY_PNG.to_vec(), true);
            let result = f.prepare();
            let requests = server.finish();
            assert!(result.is_err(), "{case}");
            assert_eq!(requests.len(), 1, "{case}");
            assert_eq!(f.recovery(), before);
            assert!(f
                .authority
                .enumerate_regular_names(ManagedUserArea::Output)
                .unwrap()
                .is_empty());
        }
    }
    #[test]
    fn namespace_delivery_retained_replacement_stale_writer_and_recovery_change_refuse_ack() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        for case in [
            "leaf-before-ui",
            "ancestor-before-ui",
            "stale-writer",
            "leaf-before-ack",
            "ancestor-before-ack",
            "missing-recovery",
            "recovery-save-failure",
            "changed-recovery",
        ] {
            let (listener, url) = backend_generation::billing_capture_test_support::listener();
            let f = DeliveryFixture::new(&url);
            let server = DeliveryServer::start(
                listener,
                &url,
                &f,
                delivery_detail(),
                DELIVERY_PNG.to_vec(),
                true,
            );
            let mut store = Store::default();
            let result = (|| -> Result<bool> {
                let prepared = f.prepare()?;
                let replace = || -> Result<()> {
                    if case.starts_with("leaf") {
                        std::fs::rename(f.output(), f.output().with_extension("retained"))?;
                        std::fs::write(f.output(), b"replacement")?;
                    } else {
                        let output = f.output().parent().unwrap().to_owned();
                        std::fs::rename(&output, output.with_file_name("retained-output"))?;
                        std::fs::create_dir(&output)?;
                        std::fs::write(f.output(), b"replacement")?;
                    }
                    Ok(())
                };
                if case.ends_with("before-ui") {
                    replace()?;
                }
                if case == "stale-writer" {
                    f.repo.activate(f.repo.lease(USER_B, 7, 2))?;
                }
                let (_, _, receipt) = persist_namespace_delivery(
                    &app,
                    &mut store,
                    &f.repo.writer,
                    prepared,
                    "fixture",
                )?;
                if case.ends_with("before-ack") {
                    replace()?;
                }
                if case == "missing-recovery" {
                    remove_pending_generation_for_namespace(&f.authority, &f.record.identity())?;
                }
                if case == "changed-recovery" {
                    let mut changed = f.record.clone();
                    changed.server_task_id = USER_B.into();
                    upsert_pending_generation_for_namespace(&f.authority, &f.scope, changed)?;
                }
                if case == "recovery-save-failure" {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let recovery = f
                            .authority
                            .lease()
                            .namespace
                            .path(ManagedUserArea::Recovery);
                        std::fs::set_permissions(recovery, std::fs::Permissions::from_mode(0o555))?;
                    }
                    #[cfg(not(unix))]
                    {
                        remove_pending_generation_for_namespace(
                            &f.authority,
                            &f.record.identity(),
                        )?;
                    }
                }
                Ok(run_owned_worker(|| {
                    acknowledge_namespace_delivery(receipt)
                })?)
            })();
            let requests = server.finish();
            if case == "recovery-save-failure" {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        f.authority
                            .lease()
                            .namespace
                            .path(ManagedUserArea::Recovery),
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .unwrap();
                }
            }
            assert!(!matches!(result, Ok(true)), "{case}");
            assert!(!requests.iter().any(|r| r.0.starts_with("POST ")), "{case}");
            if case.ends_with("before-ui") {
                assert!(store.assets.is_empty(), "{case}");
            } else {
                assert_eq!(store.assets.len(), 1, "{case}");
            }
            if case == "stale-writer" {
                // The adapter stages metadata before the ordered writer can
                // reject its retired lease. Staging is not a durable save or
                // a delivery acknowledgement, and is retained for recovery.
                assert_eq!(store.assets[0].id, DELIVERY_FILE);
                assert_eq!(store.assets[0].source_path, f.output().to_string_lossy());
                assert_eq!(store.generations.len(), 1);
                assert_eq!(store.generations[0].id, DELIVERY_FILE);
                assert_eq!(std::fs::read(f.output()).unwrap(), DELIVERY_PNG);
                assert!(f.repo.load_client_state_for_namespace(f.authority.lease()).unwrap().is_none());
                assert!(f.repo.load_client_state_for_namespace(&f.repo.lease(USER_B, 7, 2)).unwrap().is_none());
                let pending = load_pending_generations_for_namespace(&f.authority).unwrap();
                let original = pending.iter().find(|record| record.identity() == f.record.identity()).unwrap();
                assert!(!original.deliveries.iter().any(|delivery| delivery.acknowledged));
            }
            if case.starts_with("leaf") || case.starts_with("ancestor") {
                assert_eq!(std::fs::read(f.output()).unwrap(), b"replacement");
            }
        }
    }
    #[test]
    fn namespace_delivery_session_invalidation_during_blob_read_stops_publication() {
        use std::io::Write;
        let (listener, url) = backend_generation::billing_capture_test_support::listener();
        let f = DeliveryFixture::new(&url);
        let session = f.session.clone();
        let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            let mut requests = Vec::new();
            while requests.len() < 2 && std::time::Instant::now() < deadline {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let request = String::from_utf8(
                    backend_generation::billing_capture_test_support::read_request_bytes(
                        &mut stream,
                    ),
                )
                .unwrap();
                if request.starts_with("GET /blob ") {
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: 68\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                    stream.write_all(&DELIVERY_PNG[..20]).unwrap();
                    blocked_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                    let _ = stream.write_all(&DELIVERY_PNG[20..]);
                } else {
                    let mut detail = delivery_detail();
                    detail["items"][0]["file"]["download_url"] = format!("{url}blob").into();
                    let body = serde_json::to_vec(&serde_json::json!({"request_id":"fixture","data":detail,"error":null,"meta":null})).unwrap();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(&body).unwrap();
                }
                requests.push(request);
            }
            requests
        });
        let (observed, result) = std::thread::scope(|scope| {
            let worker = scope.spawn(|| f.prepare());
            let observed = blocked_rx.recv_timeout(Duration::from_secs(5)).is_ok();
            session.clear().unwrap();
            let _ = release_tx.send(());
            (observed, worker.join().unwrap())
        });
        let requests = server.join().unwrap();
        assert!(observed, "real response must reach the blocked read");
        assert!(result.is_err());
        assert_eq!(requests.len(), 2);
        assert!(f
            .authority
            .enumerate_regular_names(ManagedUserArea::Output)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn namespace_delivery_task_created_in_group_a_delivers_after_switch_to_group_b() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let (listener, url) = backend_generation::billing_capture_test_support::listener();
        let f = DeliveryFixture::new(&url);
        let b = f.repo.lease(USER_B, 7, 2);
        f.repo.activate(b.clone()).unwrap();
        f.repo
            .persist_client_state_checked_for_namespace(&b, store_with_asset("sentinel"))
            .unwrap();
        let before_b = durable_json(&f.repo, &b);
        let other = NamespaceStorageAuthority::open(f.repo.data_root_capability_arc(), &b).unwrap();
        let key = ManagedFileKey::new(ManagedUserArea::Output, "sentinel").unwrap();
        let mut sentinel = other.create_new_regular(&key).unwrap();
        other
            .write_new_regular_from(&mut sentinel, &mut &b"untouched"[..])
            .unwrap();
        f.repo.activate(f.authority.lease().clone()).unwrap();
        f.repo
            .save_selected_group(USER_A, "device", GROUP_A)
            .unwrap();
        f.repo
            .save_selected_group(USER_A, "device", GROUP_B)
            .unwrap();
        let server = DeliveryServer::start(
            listener,
            &url,
            &f,
            delivery_detail(),
            DELIVERY_PNG.to_vec(),
            true,
        );
        let mut store = Store::default();
        let result = (|| -> Result<_> {
            let prepared = f.prepare()?;
            let (image, id, committed) = persist_namespace_delivery(
                &app,
                &mut store,
                &f.repo.writer,
                prepared,
                "fixture-time",
            )?;
            let ack = run_owned_worker(|| acknowledge_namespace_delivery(committed))?;
            Ok((image, id, ack))
        })();
        let requests = server.finish();
        let (image, id, acknowledged) = result.expect("real namespace delivery must complete");
        assert!(acknowledged);
        assert_eq!(id, DELIVERY_FILE);
        assert_eq!((image.size().width, image.size().height), (1, 1));
        assert_eq!(image.to_rgba8().unwrap().as_bytes(), &[0, 0, 0, 255]);
        assert_eq!(std::fs::read(f.output()).unwrap(), DELIVERY_PNG);
        assert_eq!(f.recovery(), serde_json::json!([]));
        let saved = f
            .repo
            .load_client_state_for_namespace(f.authority.lease())
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                saved.assets.len(),
                saved.generations.len(),
                saved.notifications.len()
            ),
            (1, 1, 1)
        );
        assert_eq!((saved.assets[0].width, saved.assets[0].height), (1, 1));
        assert_eq!(saved.assets[0].prompt, "generated prompt");
        assert!(f
            .index
            .find_file_by_path_for_namespace(
                &f.authority,
                ManagedUserArea::Output,
                &format!("{DELIVERY_FILE}.png")
            )
            .unwrap()
            .is_some());
        assert_eq!(
            f.repo
                .load_selected_group(USER_A, "device")
                .unwrap()
                .as_deref(),
            Some(GROUP_B)
        );
        assert_eq!(durable_json(&f.repo, &b), before_b);
        let mut bytes = Vec::new();
        other.read_regular_to(&mut sentinel, &mut bytes).unwrap();
        assert_eq!(bytes, b"untouched");
        assert!(!UserNamespace::new(f.repo.directory.path(), GROUP_B)
            .unwrap()
            .root()
            .exists());
        assert_eq!(requests.len(), 3);
        for (request, durable) in &requests {
            let headers = request.split("\r\n\r\n").next().unwrap().to_lowercase();
            assert!(!headers.contains("x-account-group-id"));
            if request.starts_with("GET /blob ") {
                assert!(!headers.contains("x-token"));
                assert!(!headers.contains("authorization"));
                assert!(!headers.contains("x-device"));
            } else {
                assert!(headers.contains("x-token: delivery-fixture-access"));
            }
            if request.starts_with("POST ") {
                assert!(
                    *durable,
                    "HTTP ack must observe durable metadata and unacknowledged recovery"
                );
                assert!(request.starts_with(&format!(
                    "POST /v1/generation/tasks/{DELIVERY_TASK}/deliveries/{DELIVERY_FILE}/ack "
                )));
                let body: serde_json::Value =
                    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                assert_eq!(body["size_bytes"], 68);
                assert_eq!(
                    body["sha256"],
                    delivery_detail()["items"][0]["file"]["sha256"]
                );
            }
        }
    }
    const V1_SCHEMA: &str = r#"CREATE TABLE IF NOT EXISTS client_meta (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS assets (
            collection TEXT NOT NULL,
            id TEXT NOT NULL,
            position INTEGER NOT NULL,
            conversation_id TEXT NOT NULL,
            title TEXT NOT NULL,
            category TEXT NOT NULL,
            kind TEXT NOT NULL,
            time TEXT NOT NULL,
            prompt TEXT NOT NULL,
            ratio TEXT NOT NULL,
            quality TEXT NOT NULL,
            model TEXT NOT NULL,
            origin TEXT NOT NULL,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            source_path TEXT NOT NULL,
            cutout_done INTEGER NOT NULL,
            remove_black_done INTEGER NOT NULL,
            upscale_done INTEGER NOT NULL,
            PRIMARY KEY (collection, id)
        );
        CREATE INDEX IF NOT EXISTS assets_collection_position
            ON assets(collection, position);
        CREATE TABLE IF NOT EXISTS asset_references (
            collection TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            path TEXT NOT NULL,
            PRIMARY KEY (collection, asset_id, position),
            FOREIGN KEY (collection, asset_id)
                REFERENCES assets(collection, id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS notifications (
            id TEXT PRIMARY KEY NOT NULL,
            position INTEGER NOT NULL,
            title TEXT NOT NULL,
            model TEXT NOT NULL,
            time TEXT NOT NULL,
            reason TEXT NOT NULL,
            success INTEGER NOT NULL,
            read INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS canvas_nodes (
            id TEXT PRIMARY KEY NOT NULL,
            position INTEGER NOT NULL,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            x REAL NOT NULL,
            y REAL NOT NULL,
            width REAL NOT NULL,
            height REAL NOT NULL,
            parent_group_id TEXT NOT NULL,
            z_index INTEGER NOT NULL,
            image_path TEXT NOT NULL,
            font_size REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS canvas_links (
            id TEXT PRIMARY KEY NOT NULL,
            position INTEGER NOT NULL,
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            flow_reversed INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS custom_prompts (
            prompt TEXT PRIMARY KEY NOT NULL,
            position INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            profile_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS client_settings (
            key TEXT PRIMARY KEY NOT NULL,
            value_json TEXT NOT NULL
        );"#;
    pub(in crate::runtime) struct Fixture {
        writer: ClientStateWriter,
        pause: Arc<(Mutex<bool>, Condvar)>,
        private_paused: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
        directory: tempfile::TempDir,
    }
    impl std::ops::Deref for Fixture {
        type Target = ClientStateWriter;
        fn deref(&self) -> &Self::Target {
            &self.writer
        }
    }
    impl Fixture {
        pub(crate) fn new(paused: bool, private_paused: bool) -> Self {
            let directory =
                tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
                    .unwrap();
            let root = Arc::new(
                NamespaceFs::open_data_root(&directory.path().canonicalize().unwrap()).unwrap(),
            );
            let path = directory.path().join("fixture.sqlite3");
            let mut connection = open_client_state_connection(&path).unwrap();
            migrate_client_state_schema(&mut connection, root.as_ref()).unwrap();
            let (writer, receiver) = ClientStateWriter::channel(path, root);
            let pause = Arc::new((Mutex::new(paused), Condvar::new()));
            let private_paused = Arc::new(AtomicBool::new(private_paused));
            let stop = Arc::new(AtomicBool::new(false));
            let (w, p, pp, s) = (
                writer.clone(),
                pause.clone(),
                private_paused.clone(),
                stop.clone(),
            );
            let worker = std::thread::spawn(move || {
                let mut guard = p.0.lock().unwrap();
                while *guard {
                    guard = p.1.wait(guard).unwrap();
                }
                drop(guard);
                let mut last = CommittedSequences::default();
                let mut held = std::collections::VecDeque::new();
                while !s.load(Ordering::SeqCst) {
                    let next = if !pp.load(Ordering::SeqCst) && !held.is_empty() {
                        held.pop_front()
                    } else {
                        receiver.recv_timeout(Duration::from_millis(5)).ok()
                    };
                    if let Some(command) = next {
                        let private = matches!(
                            &command.command,
                            ClientStateWrite::Wake(_)
                                | ClientStateWrite::RetainedRedemptionRead { .. }
                                | ClientStateWrite::LocalStoreChecked { .. }
                                | ClientStateWrite::UserProfileChecked { .. }
                                | ClientStateWrite::Flush { .. }
                                | ClientStateWrite::FlushForRetirement { .. }
                        );
                        if pp.load(Ordering::SeqCst) && private {
                            held.push_back(command);
                        } else {
                            process_client_state_write(&mut connection, &w, &mut last, command);
                        }
                    }
                }
            });
            Self {
                writer,
                directory,
                pause,
                private_paused,
                stop,
                worker: Some(worker),
            }
        }
        fn resume_writer(&self) {
            *self.pause.0.lock().unwrap() = false;
            self.pause.1.notify_one();
        }
        fn resume_private_writer(&self) {
            self.private_paused.store(false, Ordering::SeqCst);
        }
        fn connection(&self) -> Connection {
            open_client_state_connection(&self.path).unwrap()
        }
        pub(in crate::runtime) fn reject_custom_prompt_inserts_for_test(&self,reject:bool) {
            let c=self.connection();
            c.execute_batch(if reject {
                "CREATE TRIGGER fixture_reject_custom_prompt_insert BEFORE INSERT ON custom_prompts BEGIN SELECT RAISE(ABORT,'fixture custom save failure'); END;"
            } else {"DROP TRIGGER fixture_reject_custom_prompt_insert"}).unwrap();
        }
        pub(in crate::runtime) fn reject_notification_inserts_for_test(&self) {
            reject_notification_inserts(self);
        }
        pub(crate) fn data_root_capability_arc(&self) -> Arc<DataRootCapability> {
            self.data_root.clone()
        }
        pub(crate) fn lease(&self, user: &str, auth_epoch: u64, namespace_epoch: u64) -> NamespaceLease {
            NamespaceLease {
                namespace: UserNamespace::new(self.directory.path(), user).unwrap(),
                auth_epoch,
                namespace_epoch,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.resume_writer();
            if let Some(worker) = self.worker.take() { worker.join().unwrap(); }
        }
    }
    fn test_repository_v2() -> Fixture {
        Fixture::new(false, false)
    }
    #[test]
    fn core_ordered_save_receiver_drop_keeps_real_activity_until_ack_and_preserves_queue_order() {
        let r = paused_private_test_writer();
        let lease = r.lease(USER_A, 1, 10); r.activate(lease.clone()).unwrap();
        let activity = super::super::UserActivityGate::default();
        activity.activate(lease.clone()).unwrap();
        let latch = super::super::api::UpgradeLatch::default();
        let first = r.enqueue_client_state_checked_for_namespace(&lease, store_with_asset("first"),
            (activity.begin_recovery_unit(&lease).unwrap(), latch.begin_ordinary_durable_commit().unwrap())).unwrap();
        let second = r.enqueue_client_state_checked_for_namespace(&lease, store_with_asset("second"),
            (activity.begin_recovery_unit(&lease).unwrap(), latch.begin_ordinary_durable_commit().unwrap())).unwrap();
        drop(first);
        struct Resume(Arc<AtomicBool>);
        impl Drop for Resume { fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); } }
        std::thread::scope(|threads| {
            let _resume = Resume(r.private_paused.clone());
            let (done, received) = mpsc::channel();
            let captured = activity.clone(); let old = lease.clone();
            let worker = threads.spawn(move || {
                let quiesced = captured.begin_quiesce(&old).unwrap();
                done.send(()).unwrap(); drop(quiesced);
            });
            assert!(received.recv_timeout(Duration::from_millis(50)).is_err(), "dropped receiver released admitted write early");
            r.resume_private_writer();
            second.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
            received.recv_timeout(Duration::from_secs(5)).unwrap(); worker.join().unwrap();
        });
        assert_eq!(r.load_client_state_for_namespace(&lease).unwrap().unwrap().assets[0].id, "second");
    }
    #[test]
    fn core_ordered_save_disconnected_queue_returns_guard_ownership_outside_completion() {
        let mut r = Fixture::new(false, false);
        let lease = r.lease(USER_A, 1, 10); r.activate(lease.clone()).unwrap();
        let activity = super::super::UserActivityGate::default(); activity.activate(lease.clone()).unwrap();
        let latch = super::super::api::UpgradeLatch::default();
        let mut admission = Some((activity.begin_recovery_unit(&lease).unwrap(), latch.begin_ordinary_durable_commit().unwrap()));
        r.stop.store(true, Ordering::SeqCst);
        r.worker.take().unwrap().join().unwrap();
        let result = latch.apply_if_open(|| r.enqueue_client_state_checked_for_namespace(&lease, store_with_asset("never"),
            admission.take().unwrap())).unwrap();
        assert!(result.is_err());
        // This exact return/drop order is required: errors own guards, so no
        // counted guard is destroyed while the completion mutex is held.
        drop(result); drop(admission);
        let quiesced = activity.begin_quiesce(&lease).unwrap(); drop(quiesced);
        assert!(r.load_client_state_for_namespace(&lease).unwrap().is_none());
        // The actual fixture worker was explicitly joined before root release.
    }
    #[test]
    fn core_ordered_profile_dropped_receiver_holds_admission_and_preserves_shared_queue_order(){
        let r=paused_private_test_writer();let lease=r.lease(USER_A,1,10);r.activate(lease.clone()).unwrap();
        let activity=UserActivityGate::default();activity.activate(lease.clone()).unwrap();let latch=api::UpgradeLatch::default();
        let p=PrivatePersistence::for_test(r.writer.clone(),lease.clone(),activity.clone(),latch.clone());
        let mut first=Some(p.prepare_ordered_save().unwrap());let mut second=Some(p.prepare_ordered_save().unwrap());
        let first=latch.apply_if_open(||first.take().unwrap().enqueue_profile(UserProfileData{nickname:"first".into(),..Default::default()})).unwrap().unwrap();
        let between=p.prepare_ordered_save().unwrap().enqueue(store_with_asset("between")).unwrap();
        let last=latch.apply_if_open(||second.take().unwrap().enqueue_profile(UserProfileData{nickname:"last".into(),..Default::default()})).unwrap().unwrap();
        drop(first);drop(between);
        struct Resume(Arc<AtomicBool>);impl Drop for Resume{fn drop(&mut self){self.0.store(false,Ordering::SeqCst);}}
        std::thread::scope(|threads|{
            let _resume=Resume(r.private_paused.clone());let(done,received)=mpsc::channel();let gate=activity.clone();let old=lease.clone();
            let worker=threads.spawn(move||{let retired=gate.begin_quiesce(&old).unwrap();done.send(()).unwrap();drop(retired);});
            assert!(received.recv_timeout(Duration::from_millis(50)).is_err(),"profile receiver drop released real admission before writer ack");
            r.resume_private_writer();last.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
            received.recv_timeout(Duration::from_secs(5)).unwrap();worker.join().unwrap();
        });
        assert_eq!(r.load_client_user_profile_for_namespace(&lease).unwrap().unwrap().nickname,"last");
        assert_eq!(r.load_client_state_for_namespace(&lease).unwrap().unwrap().assets[0].id,"between");
    }
    #[test]
    fn core_ordered_profile_disconnected_queue_returns_owned_guards_outside_completion(){
        let mut r=test_repository_v2();let lease=r.lease(USER_A,1,10);r.activate(lease.clone()).unwrap();
        let activity=UserActivityGate::default();activity.activate(lease.clone()).unwrap();let latch=api::UpgradeLatch::default();
        let p=PrivatePersistence::for_test(r.writer.clone(),lease.clone(),activity.clone(),latch.clone());
        let mut write=Some(p.prepare_ordered_save().unwrap());r.stop.store(true,Ordering::SeqCst);r.worker.take().unwrap().join().unwrap();
        let result=latch.apply_if_open(||write.take().unwrap().enqueue_profile(UserProfileData{nickname:"never".into(),..Default::default()})).unwrap();
        assert!(result.is_err());drop(result);drop(write);
        let retired=activity.begin_quiesce(&lease).unwrap();drop(retired);
        assert!(r.load_client_user_profile_for_namespace(&lease).unwrap().is_none());
    }
    #[test]
    fn core_ordered_profile_sqlite_rejection_retains_prior_profile_and_retry_uses_real_ack(){
        let r=test_repository_v2();let lease=r.lease(USER_A,1,10);r.activate(lease.clone()).unwrap();
        let activity=UserActivityGate::default();activity.activate(lease.clone()).unwrap();let latch=api::UpgradeLatch::default();
        let p=PrivatePersistence::for_test(r.writer.clone(),lease.clone(),activity.clone(),latch);
        p.prepare_ordered_save().unwrap().enqueue_profile(UserProfileData{nickname:"original".into(),..Default::default()}).unwrap()
            .recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        r.connection().execute_batch("CREATE TRIGGER reject_profile BEFORE INSERT ON user_settings WHEN NEW.key='user_profile' BEGIN SELECT RAISE(ABORT,'controlled profile failure'); END;").unwrap();
        let result=p.prepare_ordered_save().unwrap().enqueue_profile(UserProfileData{nickname:"retry".into(),..Default::default()}).unwrap();
        assert!(result.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
        assert_eq!(r.load_client_user_profile_for_namespace(&lease).unwrap().unwrap().nickname,"original");
        r.connection().execute_batch("DROP TRIGGER reject_profile").unwrap();
        p.prepare_ordered_save().unwrap().enqueue_profile(UserProfileData{nickname:"retry".into(),..Default::default()}).unwrap()
            .recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(r.load_client_user_profile_for_namespace(&lease).unwrap().unwrap().nickname,"retry");
        let retired=activity.begin_quiesce(&lease).unwrap();drop(retired);
    }

    fn paused_test_writer() -> Fixture {
        Fixture::new(true, false)
    }
    fn paused_private_test_writer() -> Fixture {
        Fixture::new(false, true)
    }
    fn store_with_asset(id: &str) -> LocalStoreData {
        serde_json::from_value(serde_json::json!({"assets":[{"id":id,"conversation_id":"conversation","title":id,"category":"scene","kind":"game","time":"time","prompt":"private prompt","ratio":"1:1","quality":"2k","model":"model","source_path":"private/image.png","reference_paths":["private/ref.png"]}],"notifications":[{"id":"same","title":id,"model":"model","time":"time","reason":"private","success":true,"read":false}],"canvas_notes":[{"id":"same","content":id,"x":1.0,"y":2.0}],"canvas_links":[{"id":"same","source_id":id,"target_id":"other"}],"custom_prompts":[id],"image_model":id,"prompt_drafts":{"scene":id}})).unwrap()
    }
    fn checked_asset(id: &str, source_path: &str, prompt: &str) -> AssetData {
        AssetData {
            id: id.into(),
            conversation_id: "owned-conversation".into(),
            title: format!("title-{id}"),
            category: "scene".into(),
            kind: "game".into(),
            time: "2026-09-04 12:00".into(),
            prompt: prompt.into(),
            ratio: "16:9".into(),
            quality: "2K".into(),
            model: "owned-image-model".into(),
            origin: "generation".into(),
            width: 1920,
            height: 1080,
            source_path: source_path.into(),
            reference_paths: vec!["owned/reference.png".into()],
            cutout_done: true,
            remove_black_done: false,
            upscale_done: true,
            is_new: true,
            delivery_recoverable: false,
            delivery_downloading: false,
        }
    }
    fn checked_notification(id: &str, title: &str) -> NotificationData {
        NotificationData {
            id: id.into(),
            title: title.into(),
            model: "owned-image-model".into(),
            time: "2026-09-04 12:01".into(),
            reason: String::new(),
            success: true,
            read: false,
        }
    }
    fn durable_json(repo: &Fixture, lease: &NamespaceLease) -> Option<serde_json::Value> {
        repo.load_client_state_for_namespace(lease)
            .unwrap()
            .map(|data| serde_json::to_value(data).unwrap())
    }
    fn assert_stale_anyhow(error: anyhow::Error) {
        assert_eq!(
            error.downcast_ref::<ClientStateWriteError>(),
            Some(&ClientStateWriteError::StaleLease)
        );
    }
    fn reject_notification_inserts(repo: &Fixture) {
        repo.connection()
            .execute_batch(concat!(
                "CREATE TRIGGER reject_fixture_notification ",
                "BEFORE INSERT ON notifications BEGIN ",
                "SELECT RAISE(ABORT, 'fixture rejected'); END;"
            ))
            .unwrap();
    }
    struct PauseReleaseGuard {
        pause: Arc<(Mutex<bool>, Condvar)>,
    }
    impl PauseReleaseGuard {
        fn new(pause: Arc<(Mutex<bool>, Condvar)>) -> Self {
            Self { pause }
        }
    }
    impl Drop for PauseReleaseGuard {
        fn drop(&mut self) {
            let mut paused = self
                .pause
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *paused = false;
            self.pause.1.notify_all();
        }
    }
    struct RaceControllerResult {
        observation: std::result::Result<bool, ClientStateWriteError>,
        activation: WriteResult,
    }
    fn observe_queue_activate_and_resume(
        writer: ClientStateWriter,
        pause: Arc<(Mutex<bool>, Condvar)>,
        start_sequence: u64,
        next: NamespaceLease,
    ) -> RaceControllerResult {
        let _release = PauseReleaseGuard::new(pause);
        let deadline = Instant::now() + Duration::from_secs(5);
        let observation = (|| {
            loop {
                let pending = writer.pending.lock().map_err(local_error)?;
                if pending.sequence > start_sequence {
                    return Ok(true);
                }
                drop(pending);
                if Instant::now() >= deadline {
                    return Ok(false);
                }
                std::thread::yield_now();
            }
        })();
        let activation = writer.activate(next);
        RaceControllerResult {
            observation,
            activation,
        }
    }
    #[derive(Debug, PartialEq, Eq)]
    struct AssetMemorySnapshot {
        id: String,
        conversation_id: String,
        title: String,
        category: String,
        kind: String,
        time: String,
        prompt: String,
        ratio: String,
        quality: String,
        model: String,
        origin: String,
        width: i32,
        height: i32,
        source_path: String,
        reference_paths: Vec<String>,
        cutout_done: bool,
        remove_black_done: bool,
        upscale_done: bool,
        is_new: bool,
        delivery_recoverable: bool,
        delivery_downloading: bool,
    }
    impl From<&AssetData> for AssetMemorySnapshot {
        fn from(item: &AssetData) -> Self {
            Self {
                id: item.id.clone(),
                conversation_id: item.conversation_id.clone(),
                title: item.title.clone(),
                category: item.category.clone(),
                kind: item.kind.clone(),
                time: item.time.clone(),
                prompt: item.prompt.clone(),
                ratio: item.ratio.clone(),
                quality: item.quality.clone(),
                model: item.model.clone(),
                origin: item.origin.clone(),
                width: item.width,
                height: item.height,
                source_path: item.source_path.clone(),
                reference_paths: item.reference_paths.clone(),
                cutout_done: item.cutout_done,
                remove_black_done: item.remove_black_done,
                upscale_done: item.upscale_done,
                is_new: item.is_new,
                delivery_recoverable: item.delivery_recoverable,
                delivery_downloading: item.delivery_downloading,
            }
        }
    }
    #[derive(Debug, PartialEq, Eq)]
    struct NotificationMemorySnapshot {
        id: String,
        title: String,
        model: String,
        time: String,
        reason: String,
        success: bool,
        read: bool,
    }
    impl From<&NotificationData> for NotificationMemorySnapshot {
        fn from(item: &NotificationData) -> Self {
            Self {
                id: item.id.clone(),
                title: item.title.clone(),
                model: item.model.clone(),
                time: item.time.clone(),
                reason: item.reason.clone(),
                success: item.success,
                read: item.read,
            }
        }
    }
    #[derive(Debug, PartialEq, Eq)]
    struct ReplacementMemorySnapshot {
        generations: Vec<AssetMemorySnapshot>,
        assets: Vec<AssetMemorySnapshot>,
        notifications: Vec<NotificationMemorySnapshot>,
    }
    fn replacement_memory_snapshot(store: &Store) -> ReplacementMemorySnapshot {
        ReplacementMemorySnapshot {
            generations: store
                .generations
                .iter()
                .map(AssetMemorySnapshot::from)
                .collect(),
            assets: store
                .assets
                .iter()
                .map(AssetMemorySnapshot::from)
                .collect(),
            notifications: store
                .notifications
                .iter()
                .map(NotificationMemorySnapshot::from)
                .collect(),
        }
    }
    fn failed_delivery_store() -> Store {
        let mut store = Store::default();
        store.generations.push(checked_asset(
            "existing",
            "owned/existing.png",
            "existing",
        ));
        store
            .generations
            .push(checked_asset("failed-1", "failed", "failed prompt"));
        store
            .assets
            .push(checked_asset("prior-asset", "owned/prior.png", "prior"));
        store
            .notifications
            .push(checked_notification("prior-notification", "prior"));
        store
    }
    #[derive(Clone, Copy, Debug)]
    enum InvalidReplacementCase {
        EmptyFailedId,
        WrongCompletedId,
        EmptyCompletedPath,
        FailedCompletedPath,
        MissingCard,
        AmbiguousCard,
        NonFailedCard,
        DuplicateAsset,
    }
    fn invalid_replacement_fixture(
        case: InvalidReplacementCase,
        completed_path: &str,
    ) -> (Store, &'static str, AssetData) {
        let mut store = failed_delivery_store();
        let mut failed_asset_id = "failed-1";
        let mut completed = checked_asset("failed-1", completed_path, "completed prompt");
        match case {
            InvalidReplacementCase::EmptyFailedId => failed_asset_id = "",
            InvalidReplacementCase::WrongCompletedId => completed.id = "wrong-id".into(),
            InvalidReplacementCase::EmptyCompletedPath => completed.source_path = "  ".into(),
            InvalidReplacementCase::FailedCompletedPath => completed.source_path = "failed".into(),
            InvalidReplacementCase::MissingCard => {
                failed_asset_id = "missing";
                completed.id = "missing".into();
            }
            InvalidReplacementCase::AmbiguousCard => {
                store
                    .generations
                    .push(checked_asset("failed-1", "failed", "duplicate failed"));
            }
            InvalidReplacementCase::NonFailedCard => {
                store.generations[1].source_path = "owned/already-complete.png".into();
            }
            InvalidReplacementCase::DuplicateAsset => {
                store.assets.push(checked_asset(
                    "failed-1",
                    "owned/duplicate.png",
                    "duplicate asset",
                ));
            }
        }
        (store, failed_asset_id, completed)
    }
    #[test]
    fn namespace_checked_store_round_trips_models_and_drafts() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let repo = test_repository_v2();
        let lease = repo.lease(USER_A, 1, 10);
        repo.activate(lease.clone()).unwrap();
        let mut store = Store::default();
        store.generations.push(checked_asset(
            "existing-generation",
            "owned/existing-generation.png",
            "generation prompt",
        ));
        store.assets.push(checked_asset(
            "existing-asset",
            "owned/existing-asset.png",
            "asset prompt",
        ));
        store.notifications.push(checked_notification(
            "existing-notification",
            "existing title",
        ));
        store.prompt_drafts.scene = "owned draft".into();
        store.prompt_drafts.negative_scene = "owned negative draft".into();
        store.dismissed_prompt_history.insert("dismissed prompt".into());
        store.custom_prompts.push("owned custom prompt".into());
        store.selected_custom_prompts.insert(
            "scene".into(),
            BTreeSet::from(["owned custom prompt".into()]),
        );
        store
            .custom_prompt_times
            .insert("owned custom prompt".into(), "2026-09-04 11:00".into());
        store.custom_prompt_profiles.insert(
            "owned custom prompt".into(),
            CustomPromptProfile {
                name: "Owned profile".into(),
                category: "scene".into(),
                format: "cinematic".into(),
                negative_prompt: "blur".into(),
                reference_path: "owned/profile.png".into(),
                reference_paths: vec!["owned/profile-2.png".into()],
            },
        );
        store.active_canvas_workspace_id = "owned-workspace".into();
        store.canvas_notes.push(CanvasNoteData {
            id: "owned-node".into(),
            content: "owned canvas content".into(),
            x: 11.0,
            y: 12.0,
            ..Default::default()
        });
        store.canvas_links.push(CanvasLinkData {
            id: "owned-link".into(),
            source_id: "owned-node".into(),
            target_id: "target-node".into(),
            flow_reversed: true,
        });
        store.canvas_references.push(ReferenceData {
            id: "owned-reference".into(),
            source_path: "owned/canvas-reference.png".into(),
        });
        store.canvas_workspaces.insert(
            "existing-workspace".into(),
            CanvasWorkspaceData {
                notes: vec![CanvasNoteData {
                    id: "existing-node".into(),
                    content: "existing workspace content".into(),
                    ..Default::default()
                }],
                links: Vec::new(),
                prompt: "existing workspace prompt".into(),
                references: vec![ReferenceData {
                    id: "existing-reference".into(),
                    source_path: "owned/existing-reference.png".into(),
                }],
            },
        );
        store.legacy_deep_prompt_job_id = "legacy-owned-job".into();
        store
            .deep_prompt_jobs_by_owner
            .insert(USER_A.into(), "owned-job".into());
        store.deep_prompt_pending_requests_by_owner.insert(
            USER_A.into(),
            CreatePromptOptimization {
                client_request_id: "owned-request-12345678".into(),
                prompt: "owned pending prompt".into(),
                run_mode: "auto".into(),
                focus_mode: "system".into(),
                max_rounds: 3,
                target_score: 91,
            },
        );
        store.deep_prompt_bindings.insert(
            "scene".into(),
            DeepPromptBinding {
                chinese: "中文绑定".into(),
                english: "English binding".into(),
            },
        );
        store.contact_popup_dismissed = true;
        let state = app.global::<AppState>();
        state.set_image_model("owned-image-model".into());
        state.set_reasoning_model("owned-reasoning-model".into());
        state.set_video_model("owned-video-model".into());
        state.set_canvas_workflow_prompt("owned\ncanvas prompt".into());

        crate::runtime::local_store::save_local_store_checked_for_namespace(
            &app,
            &store,
            &repo.writer,
            &lease,
        )
        .unwrap();

        let saved = repo
            .load_client_state_for_namespace(&lease)
            .unwrap()
            .unwrap();
        assert_eq!(saved.generations[0].id, "existing-generation");
        assert_eq!(saved.assets[0].id, "existing-asset");
        assert_eq!(saved.notifications[0].id, "existing-notification");
        assert_eq!(saved.image_model, "owned-image-model");
        assert_eq!(saved.reasoning_model, "owned-reasoning-model");
        assert_eq!(saved.video_model, "owned-video-model");
        assert_eq!(saved.prompt_drafts.scene, "owned draft");
        assert_eq!(saved.prompt_drafts.negative_scene, "owned negative draft");
        assert!(saved.dismissed_prompt_history.contains("dismissed prompt"));
        assert_eq!(saved.custom_prompts, ["owned custom prompt"]);
        assert!(saved.selected_custom_prompts["scene"].contains("owned custom prompt"));
        assert_eq!(
            saved.custom_prompt_times["owned custom prompt"],
            "2026-09-04 11:00"
        );
        let saved_profile = &saved.custom_prompt_profiles["owned custom prompt"];
        assert_eq!(saved_profile.name, "Owned profile");
        assert_eq!(saved_profile.category, "scene");
        assert_eq!(saved_profile.format, "cinematic");
        assert_eq!(saved_profile.negative_prompt, "blur");
        assert_eq!(saved_profile.reference_path, "owned/profile.png");
        assert_eq!(saved_profile.reference_paths, ["owned/profile-2.png"]);
        assert_eq!(saved.active_canvas_workspace_id, "owned-workspace");
        assert_eq!(saved.canvas_notes[0].content, "owned canvas content");
        assert!(saved.canvas_links[0].flow_reversed);
        assert_eq!(
            saved.canvas_workspaces["owned-workspace"].prompt,
            "owned canvas prompt"
        );
        assert_eq!(
            saved.canvas_workspaces["owned-workspace"].references[0].id,
            "owned-reference"
        );
        assert_eq!(
            saved.canvas_workspaces["existing-workspace"].notes[0].id,
            "existing-node"
        );
        assert_eq!(saved.deep_prompt_job_id, "legacy-owned-job");
        assert_eq!(saved.deep_prompt_jobs_by_owner[USER_A], "owned-job");
        assert_eq!(
            saved.deep_prompt_pending_requests_by_owner[USER_A].prompt,
            "owned pending prompt"
        );
        assert_eq!(saved.deep_prompt_bindings["scene"].english, "English binding");
        assert!(saved.contact_popup_dismissed);
    }
    #[test]
    fn namespace_checked_generated_assets_preserve_user_partition_and_device_preferences() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let repo = test_repository_v2();
        let a = repo.lease(USER_A, 1, 10);
        let b = repo.lease(USER_B, 1, 20);
        repo.activate(b.clone()).unwrap();
        repo.persist_client_state_checked_for_namespace(&b, store_with_asset("shared-id"))
            .unwrap();
        repo.persist_device_settings_checked(settings()).unwrap();
        repo.save_selected_group(USER_A, "device-owned", GROUP_A)
            .unwrap();
        repo.activate(a.clone()).unwrap();
        let mut store = Store::default();
        store.dismissed_prompt_history.insert("reveal me".into());

        crate::runtime::local_store::persist_generated_asset_checked_for_namespace(
            &app,
            &mut store,
            &repo.writer,
            &a,
            checked_asset("shared-id", "owned/generated.png", "reveal me"),
            checked_notification("same", "A generated"),
            true,
            Some("reveal me"),
        )
        .unwrap();
        repo.save_selected_group(USER_A, "device-owned", GROUP_B)
            .unwrap();
        crate::runtime::local_store::persist_generated_asset_checked_for_namespace(
            &app,
            &mut store,
            &repo.writer,
            &a,
            checked_asset("asset-only", "owned/asset-only.png", "asset only"),
            checked_notification("asset-only-notification", "A asset only"),
            false,
            None,
        )
        .unwrap();

        assert_eq!(
            store
                .assets
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["asset-only", "shared-id"]
        );
        assert_eq!(
            store
                .generations
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["shared-id"]
        );
        assert!(!store.dismissed_prompt_history.contains("reveal me"));
        let saved_a = repo.load_client_state_for_namespace(&a).unwrap().unwrap();
        assert_eq!(
            saved_a
                .assets
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["asset-only", "shared-id"]
        );
        assert_eq!(
            saved_a
                .generations
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["shared-id"]
        );
        assert_eq!(saved_a.notifications.len(), 2);
        let saved_b = repo.load_client_state_for_namespace(&b).unwrap().unwrap();
        assert_eq!(saved_b.assets[0].id, "shared-id");
        assert_eq!(saved_b.assets[0].title, "shared-id");
        assert_eq!(saved_b.notifications[0].title, "shared-id");
        assert_eq!(repo.load_device_settings().unwrap(), Some(settings()));
        assert_eq!(
            repo.load_selected_group(USER_A, "device-owned")
                .unwrap()
                .as_deref(),
            Some(GROUP_B)
        );
    }
    #[test]
    fn namespace_checked_generated_asset_sql_failure_rolls_back_memory_and_both_users() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let repo = test_repository_v2();
        let a = repo.lease(USER_A, 1, 10);
        let b = repo.lease(USER_B, 1, 20);
        repo.activate(b.clone()).unwrap();
        repo.persist_client_state_checked_for_namespace(&b, store_with_asset("durable-b"))
            .unwrap();
        repo.activate(a.clone()).unwrap();
        repo.persist_client_state_checked_for_namespace(&a, store_with_asset("durable-a"))
            .unwrap();
        let before_a = durable_json(&repo, &a);
        let before_b = durable_json(&repo, &b);
        reject_notification_inserts(&repo);
        let mut store = Store::default();
        store.assets.push(checked_asset("prior-local", "owned/prior.png", "prior"));
        store.dismissed_prompt_history.insert("reveal me".into());

        let error = crate::runtime::local_store::persist_generated_asset_checked_for_namespace(
            &app,
            &mut store,
            &repo.writer,
            &a,
            checked_asset("rejected", "owned/rejected.png", "reveal me"),
            checked_notification("rejected-notification", "rejected"),
            true,
            Some("reveal me"),
        )
        .unwrap_err();

        assert!(matches!(
            error.downcast_ref::<ClientStateWriteError>(),
            Some(ClientStateWriteError::LocalState { .. })
        ));
        assert_eq!(
            store
                .assets
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["prior-local"]
        );
        assert!(store.generations.is_empty());
        assert!(store.notifications.is_empty());
        assert!(store.dismissed_prompt_history.contains("reveal me"));
        assert_eq!(durable_json(&repo, &a), before_a);
        assert_eq!(durable_json(&repo, &b), before_b);
    }
    #[test]
    fn namespace_checked_stale_leases_roll_back_mutation_and_leave_durable_users_unchanged() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let repo = test_repository_v2();
        let a = repo.lease(USER_A, 1, 10);
        let b = repo.lease(USER_B, 1, 20);
        repo.activate(b.clone()).unwrap();
        repo.persist_client_state_checked_for_namespace(&b, store_with_asset("durable-b"))
            .unwrap();
        repo.activate(a.clone()).unwrap();
        repo.persist_client_state_checked_for_namespace(&a, store_with_asset("durable-a"))
            .unwrap();
        let before_a = durable_json(&repo, &a);
        let before_b = durable_json(&repo, &b);
        for (index, stale) in [
            repo.lease(USER_B, 1, 10),
            repo.lease(USER_A, 2, 10),
            repo.lease(USER_A, 1, 11),
        ]
        .into_iter()
        .enumerate()
        {
            let mut store = Store::default();
            let error = crate::runtime::local_store::persist_generated_asset_checked_for_namespace(
                &app,
                &mut store,
                &repo.writer,
                &stale,
                checked_asset(&format!("stale-{index}"), "owned/stale.png", "stale"),
                checked_notification(&format!("stale-notification-{index}"), "stale"),
                true,
                None,
            )
            .unwrap_err();
            assert_stale_anyhow(error);
            assert!(store.assets.is_empty());
            assert!(store.generations.is_empty());
            assert!(store.notifications.is_empty());
        }
        repo.deactivate(&a).unwrap();
        let mut store = Store::default();
        let error = crate::runtime::local_store::persist_generated_asset_checked_for_namespace(
            &app,
            &mut store,
            &repo.writer,
            &a,
            checked_asset("inactive", "owned/inactive.png", "inactive"),
            checked_notification("inactive-notification", "inactive"),
            false,
            None,
        )
        .unwrap_err();
        assert_stale_anyhow(error);
        assert!(store.assets.is_empty());
        assert!(store.notifications.is_empty());
        assert_eq!(durable_json(&repo, &a), before_a);
        assert_eq!(durable_json(&repo, &b), before_b);
    }
    #[test]
    fn namespace_checked_accepted_command_race_rolls_back_after_activation_changes() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let repo = paused_test_writer();
        let a = repo.lease(USER_A, 1, 10);
        let b = repo.lease(USER_B, 2, 20);
        repo.activate(a.clone()).unwrap();
        let start_sequence = repo.pending.lock().unwrap().sequence;
        let writer = repo.writer.clone();
        let next = b.clone();
        let pause = Arc::clone(&repo.pause);
        let control = std::thread::spawn(move || {
            observe_queue_activate_and_resume(writer, pause, start_sequence, next)
        });
        let mut store = Store::default();
        store.dismissed_prompt_history.insert("race prompt".into());

        let result = crate::runtime::local_store::persist_generated_asset_checked_for_namespace(
            &app,
            &mut store,
            &repo.writer,
            &a,
            checked_asset("race", "owned/race.png", "race prompt"),
            checked_notification("race-notification", "race"),
            true,
            Some("race prompt"),
        );
        let controller = control.join().unwrap();
        let error = result.unwrap_err();

        assert_eq!(controller.observation, Ok(true));
        assert_eq!(controller.activation, Ok(()));
        assert_stale_anyhow(error);
        assert!(store.assets.is_empty());
        assert!(store.generations.is_empty());
        assert!(store.notifications.is_empty());
        assert!(store.dismissed_prompt_history.contains("race prompt"));
        assert!(repo.load_client_state_for_namespace(&a).unwrap().is_none());
        assert!(repo.load_client_state_for_namespace(&b).unwrap().is_none());
    }
    #[test]
    fn namespace_checked_race_controller_failure_resumes_and_terminates_owned_fixture() {
        let repo = paused_test_writer();
        let a = repo.lease(USER_A, 1, 10);
        let b = repo.lease(USER_B, 2, 20);
        repo.activate(a).unwrap();
        let pending = Arc::clone(&repo.pending);
        assert!(std::thread::spawn(move || {
            let _guard = pending.lock().unwrap();
            panic!("poison the fixture pending mutex");
        })
        .join()
        .is_err());
        let writer = repo.writer.clone();
        let pause = Arc::clone(&repo.pause);
        let (outcome_sender, outcome_receiver) = mpsc::channel();
        let control = std::thread::spawn(move || {
            let outcome = observe_queue_activate_and_resume(writer, pause, 0, b);
            let _ = outcome_sender.send(outcome);
        });

        let controller = outcome_receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        control.join().unwrap();
        assert!(matches!(
            controller.observation,
            Err(ClientStateWriteError::LocalState { .. })
        ));
        assert!(matches!(
            controller.activation,
            Err(ClientStateWriteError::LocalState { .. })
        ));
        assert!(!*repo.pause.0.lock().unwrap());

        let (dropped_sender, dropped_receiver) = mpsc::channel();
        let dropper = std::thread::spawn(move || {
            drop(repo);
            let _ = dropped_sender.send(());
        });
        dropped_receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        dropper.join().unwrap();
    }
    #[test]
    fn namespace_checked_pause_release_guard_recovers_a_poisoned_pause_mutex() {
        let pause = Arc::new((Mutex::new(true), Condvar::new()));
        let poison = Arc::clone(&pause);
        assert!(std::thread::spawn(move || {
            let _guard = poison.0.lock().unwrap();
            panic!("poison the pause mutex");
        })
        .join()
        .is_err());

        drop(PauseReleaseGuard::new(Arc::clone(&pause)));

        let paused = pause
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(!*paused);
    }
    #[test]
    fn namespace_checked_failed_delivery_replacement_persists_or_rolls_back_atomically() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let repo = test_repository_v2();
        let a = repo.lease(USER_A, 1, 10);
        repo.activate(a.clone()).unwrap();
        let completed_path = repo.directory.path().join("completed.png");
        fs::write(&completed_path, b"verified fixture bytes").unwrap();
        let mut store = failed_delivery_store();

        crate::runtime::local_store::replace_failed_delivery_asset_checked_for_namespace(
            &app,
            &mut store,
            &repo.writer,
            &a,
            "failed-1",
            checked_asset(
                "failed-1",
                completed_path.to_str().unwrap(),
                "completed prompt",
            ),
            checked_notification("completed-notification", "completed"),
        )
        .unwrap();

        assert_eq!(
            store.generations[1].source_path,
            completed_path.to_str().unwrap()
        );
        assert_eq!(store.assets[0].id, "failed-1");
        assert_eq!(store.notifications[0].id, "completed-notification");
        let saved = repo.load_client_state_for_namespace(&a).unwrap().unwrap();
        assert_eq!(
            saved.generations[1].source_path,
            completed_path.to_str().unwrap()
        );
        assert_eq!(saved.assets[0].id, "failed-1");
        assert_eq!(saved.notifications[0].id, "completed-notification");
        assert!(completed_path.is_file());
        let before = durable_json(&repo, &a);
        store
            .generations
            .push(checked_asset("failed-2", "failed", "second failed"));
        let memory_before_failure = replacement_memory_snapshot(&store);
        reject_notification_inserts(&repo);
        let error =
            crate::runtime::local_store::replace_failed_delivery_asset_checked_for_namespace(
                &app,
                &mut store,
                &repo.writer,
                &a,
                "failed-2",
                checked_asset(
                    "failed-2",
                    completed_path.to_str().unwrap(),
                    "second completed",
                ),
                checked_notification("rejected-replacement-notification", "rejected"),
            )
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ClientStateWriteError>(),
            Some(ClientStateWriteError::LocalState { .. })
        ));
        assert_eq!(replacement_memory_snapshot(&store), memory_before_failure);
        assert_eq!(durable_json(&repo, &a), before);
        assert!(completed_path.is_file());
    }
    #[test]
    fn namespace_checked_failed_delivery_invalid_inputs_do_not_enqueue_or_mutate() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let repo = test_repository_v2();
        let a = repo.lease(USER_A, 1, 10);
        repo.activate(a.clone()).unwrap();
        repo.persist_client_state_checked_for_namespace(&a, store_with_asset("durable-a"))
            .unwrap();
        let durable_before = durable_json(&repo, &a);
        let completed_path = repo.directory.path().join("completed.png");
        fs::write(&completed_path, b"verified fixture bytes").unwrap();

        for case in [
            InvalidReplacementCase::EmptyFailedId,
            InvalidReplacementCase::WrongCompletedId,
            InvalidReplacementCase::EmptyCompletedPath,
            InvalidReplacementCase::FailedCompletedPath,
            InvalidReplacementCase::MissingCard,
            InvalidReplacementCase::AmbiguousCard,
            InvalidReplacementCase::NonFailedCard,
            InvalidReplacementCase::DuplicateAsset,
        ] {
            let (mut store, failed_asset_id, completed) =
                invalid_replacement_fixture(case, completed_path.to_str().unwrap());
            let memory_before = replacement_memory_snapshot(&store);
            let sequence_before = repo.pending.lock().unwrap().sequence;
            let result =
                crate::runtime::local_store::replace_failed_delivery_asset_checked_for_namespace(
                    &app,
                    &mut store,
                    &repo.writer,
                    &a,
                    failed_asset_id,
                    completed,
                    checked_notification("must-not-enqueue", "invalid"),
                );

            assert!(result.is_err(), "case {case:?}");
            assert_eq!(
                replacement_memory_snapshot(&store),
                memory_before,
                "case {case:?}"
            );
            assert_eq!(
                repo.pending.lock().unwrap().sequence,
                sequence_before,
                "case {case:?}"
            );
            assert_eq!(durable_json(&repo, &a), durable_before, "case {case:?}");
            assert!(completed_path.is_file(), "case {case:?}");
        }
    }
    #[test]
    fn namespace_checked_old_no_lease_save_remains_closed_with_an_active_fixture_writer() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let repo = test_repository_v2();
        let a = repo.lease(USER_A, 1, 10);
        repo.activate(a.clone()).unwrap();
        let mut store = Store::default();
        store.assets.push(checked_asset(
            "must-not-save",
            "owned/closed.png",
            "closed",
        ));

        assert!(
            crate::runtime::local_store::save_local_store_checked(&app, &store).is_err()
        );
        assert!(repo.load_client_state_for_namespace(&a).unwrap().is_none());
    }
    fn settings() -> DeviceSettings {
        DeviceSettings {
            theme_id: "dark".into(),
            language: "zh-CN".into(),
            close_behavior: "tray".into(),
            generation_gallery_layout: "waterfall".into(),
            ..Default::default()
        }
    }
    fn seed_v1(
        profile: Option<&str>,
        directory_json: Option<&str>,
    ) -> (tempfile::TempDir, Arc<DataRootCapability>, Connection) {
        let directory = tempfile::tempdir().unwrap();
        let root = Arc::new(
            NamespaceFs::open_data_root(&directory.path().canonicalize().unwrap()).unwrap(),
        );
        let connection =
            open_client_state_connection(&directory.path().join("v1.sqlite3")).unwrap();
        connection.execute_batch(V1_SCHEMA).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute(
                "INSERT INTO client_meta(key,value) VALUES ('marker','private')",
                [],
            )
            .unwrap();
        if let Some(bytes) = profile {
            connection
                .execute(
                    "INSERT INTO client_settings(key,value_json) VALUES ('user_profile',?1)",
                    params![bytes],
                )
                .unwrap();
        }
        if let Some(bytes) = directory_json {
            connection
                .execute(
                    "INSERT INTO client_settings(key,value_json) VALUES ('directory_locations',?1)",
                    params![bytes],
                )
                .unwrap();
        }
        // Literal v1 records cover each ambiguous table, including FK references.
        connection.execute_batch("INSERT INTO assets VALUES ('asset','same',0,'conversation','private','scene','game','time','private prompt','1:1','2k','model','generation',1,1,'private.png',0,0,0); INSERT INTO asset_references VALUES ('asset','same',0,'private-ref.png'); INSERT INTO notifications VALUES ('same',0,'private','model','time','private',1,0); INSERT INTO canvas_nodes VALUES ('same',0,'text','private',0,0,10,10,'',0,'',12); INSERT INTO canvas_links VALUES ('same',0,'same','other',0); INSERT INTO custom_prompts VALUES ('private',0,'time','{}');").unwrap();
        (directory, root, connection)
    }
    fn row_count(c: &Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn legacy_bytes(c: &Connection, key: &str) -> String {
        c.query_row(
            "SELECT value_json FROM legacy_unassigned_client_settings WHERE key=?1",
            params![key],
            |r| r.get(0),
        )
        .unwrap()
    }
    #[test]
    fn v1_private_rows_move_to_legacy_tables_not_a_user() {
        let (_dir, root, mut c) = seed_v1(Some("{\"nickname\":\"Prior\"}"), None);
        migrate_client_state_schema(&mut c, root.as_ref()).unwrap();
        for table in V1_TABLES {
            assert_eq!(
                row_count(&c, &format!("legacy_unassigned_{table}")),
                1,
                "{table}"
            );
        }
        for table in [
            "assets",
            "asset_references",
            "notifications",
            "canvas_nodes",
            "canvas_links",
            "custom_prompts",
            "user_settings",
            "user_meta",
        ] {
            assert_eq!(row_count(&c, table), 0, "{table}");
        }
        assert_eq!(
            c.pragma_query_value::<i32, _>(None, "user_version", |r| r.get(0))
                .unwrap(),
            2
        );
    }
    #[test]
    fn v1_migration_failure_rolls_back_every_rename_and_retry_is_idempotent() {
        let (_dir, root, mut c) = seed_v1(Some("{}"), None);
        assert!(migrate_schema_with_checkpoint(&mut c, root.as_ref(), |n| {
            if n == 2 {
                anyhow::bail!("injected");
            }
            Ok(())
        })
        .is_err());
        for table in V1_TABLES {
            assert_eq!(row_count(&c, table), 1);
        }
        assert_eq!(
            c.pragma_query_value::<i32, _>(None, "user_version", |r| r.get(0))
                .unwrap(),
            1
        );
        migrate_client_state_schema(&mut c, root.as_ref()).unwrap();
        migrate_client_state_schema(&mut c, root.as_ref()).unwrap();
        for table in V1_TABLES {
            assert_eq!(row_count(&c, &format!("legacy_unassigned_{table}")), 1);
        }
    }
    #[test]
    fn v1_profile_splits_device_preferences_without_claiming_private_identity() {
        let bytes = r#"{"nickname":"Prior User","email_mask":"p***@example.com","accepted_user_terms_version":"2026-01-01","theme_id":"dark","card_style":"square","language":"zh-CN","close_behavior":"tray","ui_preferences":{"generation_gallery_layout":" WATERFALL ","asset_gallery_layout":"grid","inspiration_gallery_layout":"waterfall"}}"#;
        let (_dir, root, mut c) = seed_v1(Some(bytes), None);
        c.execute(
            "INSERT INTO client_settings VALUES ('theme_id','\"light\"')",
            [],
        )
        .unwrap();
        migrate_client_state_schema(&mut c, root.as_ref()).unwrap();
        let saved = read_device_settings(&c).unwrap().unwrap();
        assert_eq!(saved.theme_id, "light");
        assert_eq!(saved.card_style, "square");
        assert_eq!(saved.generation_gallery_layout, "waterfall");
        assert_eq!(saved.language, "zh-CN");
        assert_eq!(legacy_bytes(&c, "user_profile"), bytes);
        assert_eq!(row_count(&c, "user_meta"), 0);
    }
    #[test]
    fn malformed_legacy_profile_keeps_quarantine_and_uses_device_defaults() {
        let bytes = "{invalid private-person@example.com nickname";
        let (_dir, root, mut c) = seed_v1(Some(bytes), None);
        migrate_client_state_schema(&mut c, root.as_ref()).unwrap();
        assert_eq!(
            read_device_settings(&c).unwrap(),
            Some(DeviceSettings::default())
        );
        assert_eq!(legacy_bytes(&c, "user_profile"), bytes);
        let diagnostic = decode_legacy_device_settings(bytes).unwrap_err();
        assert_eq!(diagnostic, "legacy_user_profile_device_extract_skipped");
        assert!(!diagnostic.contains("private-person"));
        assert!(!diagnostic.contains("nickname"));
        assert!(!diagnostic.contains(bytes));
    }
    #[test]
    fn migrated_export_directory_loads_without_polluting_display_settings() {
        let external = tempfile::tempdir().unwrap();
        let path = external
            .path()
            .canonicalize()
            .unwrap()
            .join("ArtForge Export");
        let bytes=serde_json::json!({"output":path,"input":"private/input","prompt":"private/prompt","relocations":[{"source":"secret","destination":"secret2"}]}).to_string();
        let (_dir, root, mut c) = seed_v1(None, Some(&bytes));
        migrate_client_state_schema(&mut c, root.as_ref()).unwrap();
        assert_eq!(legacy_bytes(&c, "directory_locations"), bytes);
        assert!(read_device_settings(&c).unwrap().is_some());
        let raw = read_device_rows(&c).unwrap();
        assert_eq!(raw.len(), KNOWN_DEVICE_SETTING_KEYS.len());
        assert_eq!(
            serde_json::from_str::<PathBuf>(&raw["export_directory"]).unwrap(),
            path
        );
    }
    #[test]
    fn rejected_legacy_export_keeps_original_bytes_and_no_applied_preference() {
        let (_dir, root, mut c) = seed_v1(None, None);
        let bytes =
            serde_json::json!({"output":_dir.path().join("accounts/new"),"input":"private"})
                .to_string();
        c.execute(
            "INSERT INTO client_settings VALUES ('directory_locations',?1)",
            params![bytes],
        )
        .unwrap();
        migrate_client_state_schema(&mut c, root.as_ref()).unwrap();
        assert_eq!(legacy_bytes(&c, "directory_locations"), bytes);
        assert!(!read_device_rows(&c)
            .unwrap()
            .contains_key("export_directory"));
        assert!(!_dir.path().join("accounts/new").exists());
    }
    #[test]
    fn client_state_reads_and_replaces_only_the_leased_user() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 1);
        let b = r.lease(USER_B, 2, 2);
        r.activate(a.clone()).unwrap();
        r.persist_client_state_checked_for_namespace(&a, store_with_asset("a"))
            .unwrap();
        r.activate(b.clone()).unwrap();
        r.persist_client_state_checked_for_namespace(&b, store_with_asset("b"))
            .unwrap();
        let before =
            serde_json::to_value(r.load_client_state_for_namespace(&b).unwrap().unwrap()).unwrap();
        assert_eq!(
            r.load_client_state_for_namespace(&a)
                .unwrap()
                .unwrap()
                .assets[0]
                .id,
            "a"
        );
        r.activate(a.clone()).unwrap();
        r.persist_client_state_checked_for_namespace(&a, LocalStoreData::default())
            .unwrap();
        assert_eq!(
            serde_json::to_value(r.load_client_state_for_namespace(&b).unwrap().unwrap()).unwrap(),
            before
        );
        assert!(r
            .load_client_state_for_namespace(&a)
            .unwrap()
            .unwrap()
            .assets
            .is_empty());
        let c = r.connection();
        for table in [
            "assets",
            "asset_references",
            "notifications",
            "canvas_nodes",
            "canvas_links",
            "custom_prompts",
        ] {
            assert_eq!(
                c.query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE user_public_id=?1"),
                    params![USER_B],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                1,
                "{table}"
            );
        }
    }
    #[test]
    fn colliding_asset_keys_and_collection_references_remain_user_scoped() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 1);
        let b = r.lease(USER_B, 2, 2);
        let mut data = store_with_asset("same");
        data.generations = data.assets.clone();
        r.activate(a.clone()).unwrap();
        r.persist_client_state_checked_for_namespace(&a, data.clone())
            .unwrap();
        data.assets[0].title = "B asset".into();
        data.assets[0].reference_paths = vec!["B asset ref".into(), "B asset ref 2".into()];
        data.generations[0].title = "B generation".into();
        data.generations[0].reference_paths = vec!["B generation ref".into()];
        r.activate(b.clone()).unwrap();
        r.persist_client_state_checked_for_namespace(&b, data)
            .unwrap();
        let before =
            serde_json::to_value(r.load_client_state_for_namespace(&b).unwrap().unwrap()).unwrap();
        r.activate(a.clone()).unwrap();
        r.persist_client_state_checked_for_namespace(&a, LocalStoreData::default())
            .unwrap();
        assert_eq!(
            serde_json::to_value(r.load_client_state_for_namespace(&b).unwrap().unwrap()).unwrap(),
            before
        );
        assert_eq!(
            r.load_client_state_for_namespace(&b)
                .unwrap()
                .unwrap()
                .assets[0]
                .reference_paths,
            ["B asset ref", "B asset ref 2"]
        );
    }
    #[cfg(unix)]
    #[test]
    fn writer_keeps_the_original_root_identity_after_its_display_path_is_replaced() {
        let r = paused_test_writer();
        let external = tempfile::tempdir().unwrap();
        let moved = external
            .path()
            .canonicalize()
            .unwrap()
            .join("retained-private");
        let retained = r.data_root_capability_arc();
        assert!(Arc::ptr_eq(&retained, &r.writer.data_root));
        fs::rename(r.directory.path(), &moved).unwrap();
        fs::create_dir(r.directory.path()).unwrap();
        assert!(r
            .queue_device(
                None,
                Some(Some(ExportDirectoryPreference {
                    normalized_path: moved.join("must-not-export")
                }))
            )
            .is_err());
        assert!(!moved.join("must-not-export").exists());
        drop(retained);
        drop(r);
    }
    #[test]
    fn device_preferences_survive_a_b_a_without_entering_user_settings() {
        let r = test_repository_v2();
        r.persist_device_settings_checked(settings()).unwrap();
        for (user, epoch, nickname) in [
            (USER_A, 1, "Alice"),
            (USER_B, 2, "Bob"),
            (USER_A, 3, "Alice"),
        ] {
            let lease = r.lease(user, epoch, epoch);
            r.activate(lease.clone()).unwrap();
            r.persist_client_user_profile_checked_for_namespace(
                &lease,
                UserProfileData {
                    nickname: nickname.into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let value = serde_json::to_value(
                r.load_client_user_profile_for_namespace(&lease)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(value["nickname"], nickname);
            for key in KNOWN_DEVICE_SETTING_KEYS {
                assert!(value.get(key).is_none(), "{key}");
            }
            assert!(value.get("ui_preferences").is_none());
            assert_eq!(r.load_device_settings().unwrap(), Some(settings()));
        }
    }
    #[test]
    fn display_settings_async_save_preserves_export_directory() {
        let r = test_repository_v2();
        let external = tempfile::tempdir().unwrap();
        let export = ExportDirectoryPreference {
            normalized_path: external.path().canonicalize().unwrap(),
        };
        r.persist_export_directory_checked(Some(export.clone()))
            .unwrap();
        r.queue_device(Some(settings()), None).unwrap();
        r.flush_device().unwrap();
        assert_eq!(r.load_export_directory().unwrap(), Some(export));
        assert_eq!(
            read_device_rows(&r.connection()).unwrap().len(),
            KNOWN_DEVICE_SETTING_KEYS.len()
        );
        r.persist_export_directory_checked(None).unwrap();
        assert_eq!(r.load_device_settings().unwrap(), Some(settings()));
    }
    #[test]
    fn export_preference_save_and_load_revalidate_private_boundaries() {
        let r = test_repository_v2();
        for path in [
            r.directory.path().to_path_buf(),
            r.directory.path().join("legacy_unassigned/new"),
            r.directory.path().join("accounts/new"),
        ] {
            let before = path.exists();
            assert!(r
                .persist_export_directory_checked(Some(ExportDirectoryPreference {
                    normalized_path: path.clone()
                }))
                .is_err());
            assert!(r
                .queue_device(
                    None,
                    Some(Some(ExportDirectoryPreference {
                        normalized_path: path.clone()
                    }))
                )
                .is_err());
            assert_eq!(path.exists(), before);
            assert_eq!(row_count(&r.connection(), "device_settings"), 0);
            r.connection()
                .execute(
                    "INSERT INTO device_settings VALUES ('export_directory',?1)",
                    params![serde_json::to_string(&path).unwrap()],
                )
                .unwrap();
            assert!(r.load_export_directory().is_err());
            r.connection()
                .execute("DELETE FROM device_settings", [])
                .unwrap();
        }
    }
    #[cfg(unix)]
    #[test]
    fn queued_export_revalidates_after_admission_and_alias_load_is_rejected() {
        let r = paused_test_writer();
        let external = tempfile::tempdir().unwrap();
        let path = external.path().canonicalize().unwrap().join("export");
        fs::create_dir(&path).unwrap();
        r.queue_device(
            None,
            Some(Some(ExportDirectoryPreference {
                normalized_path: path.clone(),
            })),
        )
        .unwrap();
        fs::remove_dir(&path).unwrap();
        std::os::unix::fs::symlink(r.directory.path(), &path).unwrap();
        r.resume_writer();
        assert!(matches!(
            r.flush_device(),
            Err(ClientStateWriteError::LocalState { .. })
        ));
        assert_eq!(row_count(&r.connection(), "device_settings"), 0);
        assert!(r
            .persist_export_directory_checked(Some(ExportDirectoryPreference {
                normalized_path: path.clone()
            }))
            .is_err());
        r.connection()
            .execute(
                "INSERT INTO device_settings VALUES ('export_directory',?1)",
                params![serde_json::to_string(&path).unwrap()],
            )
            .unwrap();
        assert!(r.load_export_directory().is_err());
    }
    #[test]
    fn device_loaders_reject_unknown_keys_but_accept_each_others_known_keys() {
        let r = test_repository_v2();
        r.persist_device_settings_checked(settings()).unwrap();
        assert_eq!(r.load_export_directory().unwrap(), None);
        r.connection()
            .execute(
                "INSERT INTO device_settings VALUES ('private_identity','\"secret\"')",
                [],
            )
            .unwrap();
        assert!(r.load_device_settings().is_err());
        assert!(r.load_export_directory().is_err());
    }
    #[test]
    fn queued_write_from_previous_namespace_epoch_is_discarded() {
        let r = paused_test_writer();
        let a = r.lease(USER_A, 1, 10);
        let b = r.lease(USER_B, 2, 11);
        r.activate(a.clone()).unwrap();
        r.queue_private(a.clone(), Some(store_with_asset("late")), None)
            .unwrap();
        r.activate(b.clone()).unwrap();
        r.resume_writer();
        r.flush(&b).unwrap();
        assert!(r.load_client_state_for_namespace(&a).unwrap().is_none());
        assert!(r.load_client_state_for_namespace(&b).unwrap().is_none());
    }
    #[test]
    fn queued_device_write_survives_namespace_activation_and_flush() {
        let r = paused_test_writer();
        let a = r.lease(USER_A, 1, 10);
        let b = r.lease(USER_B, 2, 11);
        r.activate(a.clone()).unwrap();
        r.queue_device(Some(settings()), None).unwrap();
        r.queue_private(a.clone(), Some(store_with_asset("stale-a")), None)
            .unwrap();
        r.activate(b.clone()).unwrap();
        r.resume_writer();
        r.flush(&b).unwrap();
        assert_eq!(r.load_device_settings().unwrap(), Some(settings()));
        assert!(r.load_client_state_for_namespace(&a).unwrap().is_none());
    }
    #[test]
    fn deactivate_rejects_a_stale_lease_and_blocks_later_private_writes() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 10);
        r.activate(a.clone()).unwrap();
        for stale in [
            r.lease(USER_B, 1, 10),
            r.lease(USER_A, 2, 10),
            r.lease(USER_A, 1, 11),
        ] {
            assert_eq!(r.deactivate(&stale), Err(ClientStateWriteError::StaleLease));
            assert_eq!(r.flush(&stale), Err(ClientStateWriteError::StaleLease));
        }
        r.persist_client_state_checked_for_namespace(&a, store_with_asset("before"))
            .unwrap();
        r.queue_device(Some(settings()), None).unwrap();
        r.deactivate(&a).unwrap();
        let error = r
            .persist_client_state_checked_for_namespace(&a, store_with_asset("bad"))
            .unwrap_err();
        let erased: anyhow::Error = error.into();
        assert_eq!(
            erased.downcast_ref::<ClientStateWriteError>(),
            Some(&ClientStateWriteError::StaleLease)
        );
        assert!(r
            .queue_private(a.clone(), Some(store_with_asset("bad")), None)
            .is_err());
        r.flush_device().unwrap();
        assert_eq!(r.load_device_settings().unwrap(), Some(settings()));
        assert_eq!(
            r.load_client_state_for_namespace(&a)
                .unwrap()
                .unwrap()
                .assets[0]
                .id,
            "before"
        );
    }
    fn await_retirement_reservation(writer: &ClientStateWriter) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if writer.pending.lock().unwrap().retirement.is_some() {
                return;
            }
            assert!(Instant::now() < deadline, "retirement did not reserve admission");
            std::thread::yield_now();
        }
    }

    #[test]
    fn writer_retirement_drains_admitted_work_and_preserves_reserved_binding() {
        let r = paused_private_test_writer();
        let a = r.lease(USER_A, 1, 10);
        let b = r.lease(USER_B, 2, 11);
        r.activate(a.clone()).unwrap();
        struct ResumePrivateOnDrop(Arc<AtomicBool>);
        impl Drop for ResumePrivateOnDrop {
            fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); }
        }
        std::thread::scope(|threads| {
        let _resume = ResumePrivateOnDrop(r.private_paused.clone());
        r.queue_private(a.clone(), Some(store_with_asset("coalesced")), Some(UserProfileData {
            nickname: "admitted-profile".into(), ..Default::default()
        })).unwrap();
        let start = r.pending.lock().unwrap().sequence;
        let writer = r.writer.clone();
        let old = a.clone();
        let checked = threads.spawn(move || {
            writer.persist_client_state_checked_for_namespace(&old, store_with_asset("checked"))
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while r.pending.lock().unwrap().sequence == start {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let writer = r.writer.clone();
        let old = a.clone();
        let retirement = threads.spawn(move || writer.flush_for_retirement(&old));
        await_retirement_reservation(&r);
        assert!(r.queue_private(a.clone(), Some(store_with_asset("rejected")), None).is_err());
        assert!(r.persist_client_state_checked_for_namespace(&a, store_with_asset("rejected")).is_err());
        assert!(r.activate(b.clone()).is_err());
        assert!(r.deactivate(&a).is_err());
        assert!(r.flush_for_retirement(&a).is_err());
        r.persist_device_settings_checked(settings()).unwrap();
        // A sibling of app data is required by the real export validator.
        let external = tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let export = ExportDirectoryPreference { normalized_path: external.path().canonicalize().unwrap() };
        r.persist_export_directory_checked(Some(export.clone())).unwrap();
        assert!(r.load_client_state_for_namespace(&a).unwrap().is_none());
        r.resume_private_writer();
        checked.join().unwrap().unwrap();
        let proof = retirement.join().unwrap().unwrap();
        assert_eq!(r.load_client_state_for_namespace(&a).unwrap().unwrap().assets[0].id, "checked");
        assert_eq!(r.load_client_user_profile_for_namespace(&a).unwrap().unwrap().nickname, "admitted-profile");
        assert_eq!(r.load_device_settings().unwrap(), Some(settings()));
        assert_eq!(r.load_export_directory().unwrap(), Some(export));
        assert!(r.queue_private(a.clone(), Some(store_with_asset("after-proof")), None).is_err());
        proof.retire_flushed();
        assert_eq!(r.flush(&a), Err(ClientStateWriteError::StaleLease));
        r.activate(b.clone()).unwrap();
        r.persist_client_state_checked_for_namespace(&b, store_with_asset("B")).unwrap();
        assert_eq!(r.load_client_state_for_namespace(&a).unwrap().unwrap().assets[0].id, "checked");
        });
    }

    #[test]
    fn writer_retirement_drop_and_consume_are_bound_to_original_instance() {
        let r = test_repository_v2();
        let other = test_repository_v2();
        let a = r.lease(USER_A, 1, 10);
        let b = other.lease(USER_A, 1, 10);
        r.activate(a.clone()).unwrap();
        other.activate(b.clone()).unwrap();
        assert!(other.flush_for_retirement(&a).is_err());
        for wrong in [r.lease(USER_B, 1, 10), r.lease(USER_A, 2, 10), r.lease(USER_A, 1, 11)] {
            assert!(r.flush_for_retirement(&wrong).is_err());
        }
        let proof = r.flush_for_retirement(&a).unwrap();
        std::thread::spawn(move || drop(proof)).join().unwrap();
        r.persist_client_state_checked_for_namespace(&a, store_with_asset("after-drop")).unwrap();
        let proof = r.flush_for_retirement(&a).unwrap();
        std::thread::spawn(move || proof.retire_flushed()).join().unwrap();
        assert!(r.flush_for_retirement(&a).is_err());
        assert!(r.queue_private(a.clone(), Some(store_with_asset("stale")), None).is_err());
        other.persist_client_state_checked_for_namespace(&b, store_with_asset("unrelated")).unwrap();
        assert_eq!(r.load_client_state_for_namespace(&a).unwrap().unwrap().assets[0].id, "after-drop");
        assert_eq!(other.load_client_state_for_namespace(&b).unwrap().unwrap().assets[0].id, "unrelated");
    }

    #[test]
    fn writer_retirement_failure_debt_survives_ordinary_flush_and_empty_retries() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 10);
        r.activate(a.clone()).unwrap();
        reject_notification_inserts(&r);
        r.queue_private(a.clone(), Some(store_with_asset("undurable")), None).unwrap();
        assert!(r.flush(&a).is_err());
        // Ordinary flush retains its established one-shot error-reporting behavior.
        r.flush(&a).unwrap();
        for _ in 0..2 {
            assert!(r.flush_for_retirement(&a).is_err());
            assert_eq!(r.pending.lock().unwrap().active.as_ref(), Some(&a));
            assert!(r.pending.lock().unwrap().retirement.is_none());
        }
        assert!(r.persist_client_state_checked_for_namespace(&a, store_with_asset("failed-checked")).is_err());
        r.connection().execute_batch("DROP TRIGGER reject_fixture_notification").unwrap();
        assert!(r.flush_for_retirement(&a).is_err());
        r.persist_client_state_checked_for_namespace(&a, store_with_asset("covering")).unwrap();
        r.flush_for_retirement(&a).unwrap().retire_flushed();
        assert_eq!(r.load_client_state_for_namespace(&a).unwrap().unwrap().assets[0].id, "covering");
    }

    #[test]
    fn writer_retirement_profile_and_device_failures_require_covering_writes() {
        for slot in ["profile", "device", "export"] {
            let r = test_repository_v2();
            let a = r.lease(USER_A, 1, 10);
            r.activate(a.clone()).unwrap();
            let sql = match slot {
                "profile" => "CREATE TRIGGER fail_retirement BEFORE INSERT ON user_settings BEGIN SELECT RAISE(ABORT, 'fixture'); END;",
                "device" => "CREATE TRIGGER fail_retirement BEFORE INSERT ON device_settings WHEN NEW.key != 'export_directory' BEGIN SELECT RAISE(ABORT, 'fixture'); END;",
                _ => "CREATE TRIGGER fail_retirement BEFORE INSERT ON device_settings WHEN NEW.key = 'export_directory' BEGIN SELECT RAISE(ABORT, 'fixture'); END;",
            };
            r.connection().execute_batch(sql).unwrap();
            let external = tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
            let export = ExportDirectoryPreference { normalized_path: external.path().canonicalize().unwrap() };
            let write = || match slot {
                "profile" => r.persist_client_user_profile_checked_for_namespace(&a, UserProfileData { nickname: "covered".into(), ..Default::default() }),
                "device" => r.persist_device_settings_checked(settings()),
                _ => r.persist_export_directory_checked(Some(export.clone())),
            };
            assert!(write().is_err());
            assert!(r.flush_for_retirement(&a).is_err());
            r.connection().execute_batch("DROP TRIGGER fail_retirement").unwrap();
            assert!(r.flush_for_retirement(&a).is_err());
            write().unwrap();
            r.flush_for_retirement(&a).unwrap().retire_flushed();
        }
    }

    #[test]
    fn writer_retirement_other_lease_success_does_not_discharge_private_debt() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 10);
        let b = r.lease(USER_B, 2, 11);
        r.activate(a.clone()).unwrap();
        reject_notification_inserts(&r);
        assert!(r.persist_client_state_checked_for_namespace(&a, store_with_asset("A-failed")).is_err());
        r.connection().execute_batch("DROP TRIGGER reject_fixture_notification").unwrap();
        r.activate(b.clone()).unwrap();
        r.persist_client_state_checked_for_namespace(&b, store_with_asset("B-success")).unwrap();
        r.flush_for_retirement(&b).unwrap().retire_flushed();
        r.activate(a.clone()).unwrap();
        assert!(r.flush_for_retirement(&a).is_err());
        r.persist_client_state_checked_for_namespace(&a, store_with_asset("A-covered")).unwrap();
        r.flush_for_retirement(&a).unwrap().retire_flushed();
    }

    #[test]
    fn writer_retirement_sequence_exhaustion_never_reuses_authority() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 10);
        r.activate(a.clone()).unwrap();
        r.pending.lock().unwrap().sequence = u64::MAX;
        assert!(r.flush_for_retirement(&a).is_err());
        assert_eq!(r.pending.lock().unwrap().active.as_ref(), Some(&a));
        assert!(r.pending.lock().unwrap().retirement.is_none());
        assert!(r.queue_private(a.clone(), Some(store_with_asset("overflow")), None).is_err());
        assert!(r.activate(r.lease(USER_B, 2, 11)).is_err());
        assert_eq!(r.pending.lock().unwrap().sequence, u64::MAX);
    }

    #[test]
    fn writer_retirement_proof_is_send_but_not_clone() {
        fn assert_send<T: Send>() {}
        assert_send::<FlushedWriterRetirement>();
        trait AmbiguousIfClone<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfClone<()> for T {}
        struct Cloned;
        impl<T: ?Sized + Clone> AmbiguousIfClone<Cloned> for T {}
        let _ = <FlushedWriterRetirement as AmbiguousIfClone<_>>::check;
    }

    #[test]
    fn writer_retirement_disconnected_queue_or_ack_never_returns_proof() {
        for disconnect_ack in [false, true] {
            let root = tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
            let data_root = Arc::new(NamespaceFs::open_data_root(root.path()).unwrap());
            let (writer, receiver) = ClientStateWriter::channel(root.path().join("fixture.sqlite3"), data_root);
            let lease = NamespaceLease { namespace: UserNamespace::new(root.path(), USER_A).unwrap(), auth_epoch: 1, namespace_epoch: 1 };
            writer.activate(lease.clone()).unwrap();
            let worker = if disconnect_ack {
                Some(std::thread::spawn(move || {
                    // Actual receiver death after dequeue, before acknowledgement.
                    let command = receiver.recv().unwrap();
                    assert!(matches!(command.command, ClientStateWrite::FlushForRetirement { .. }));
                    drop(command);
                }))
            } else { drop(receiver); None };
            let result = writer.flush_for_retirement(&lease);
            if let Some(worker) = worker { worker.join().unwrap(); }
            assert!(result.is_err());
            let pending = writer.pending.lock().unwrap();
            assert_eq!(pending.active.as_ref(), Some(&lease));
            assert!(pending.retirement.is_none());
            drop(pending);
            assert!(writer.flush_for_retirement(&lease).is_err());
        }
    }
    #[test]
    fn flush_device_drains_only_device_work_while_private_work_is_paused() {
        let r = paused_private_test_writer();
        let a = r.lease(USER_A, 1, 10);
        r.activate(a.clone()).unwrap();
        r.queue_private(a.clone(), Some(store_with_asset("private-later")), None)
            .unwrap();
        r.queue_device(Some(settings()), None).unwrap();
        r.flush_device().unwrap();
        assert_eq!(r.load_device_settings().unwrap(), Some(settings()));
        assert!(r.load_client_state_for_namespace(&a).unwrap().is_none());
        r.resume_private_writer();
        r.flush(&a).unwrap();
        assert_eq!(
            r.load_client_state_for_namespace(&a)
                .unwrap()
                .unwrap()
                .assets[0]
                .id,
            "private-later"
        );
    }
    #[test]
    fn checked_write_cannot_be_overwritten_by_an_older_pending_snapshot() {
        let r = paused_test_writer();
        let a = r.lease(USER_A, 1, 1);
        r.activate(a.clone()).unwrap();
        r.queue_private(a.clone(), Some(store_with_asset("older")), None)
            .unwrap();
        let (ack, result) = mpsc::channel();
        {
            let mut p = r.pending.lock().unwrap();
            let sequence = p.next_sequence().unwrap();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::LocalStoreChecked {
                        lease: a.clone(),
                        data: store_with_asset("checked"),
                        acknowledgement: ack,
                        admission: None,
                    },
                })
                .unwrap();
        }
        r.resume_writer();
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        r.flush(&a).unwrap();
        assert_eq!(
            r.load_client_state_for_namespace(&a)
                .unwrap()
                .unwrap()
                .assets[0]
                .id,
            "checked"
        );
    }
    #[test]
    fn coalesced_newer_snapshot_cannot_be_overwritten_by_older_checked_command() {
        let r = paused_test_writer();
        let a = r.lease(USER_A, 1, 1);
        r.activate(a.clone()).unwrap();
        r.queue_private(a.clone(), Some(store_with_asset("oldest")), None)
            .unwrap();
        let (ack, result) = mpsc::channel();
        {
            let mut p = r.pending.lock().unwrap();
            let sequence = p.next_sequence().unwrap();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::LocalStoreChecked {
                        lease: a.clone(),
                        data: store_with_asset("checked"),
                        acknowledgement: ack,
                        admission: None,
                    },
                })
                .unwrap();
        }
        r.queue_private(a.clone(), Some(store_with_asset("newest")), None)
            .unwrap();
        r.resume_writer();
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        r.flush(&a).unwrap();
        assert_eq!(
            r.load_client_state_for_namespace(&a)
                .unwrap()
                .unwrap()
                .assets[0]
                .id,
            "newest"
        );
    }
    #[test]
    fn stale_checked_command_is_rejected_after_activation_before_worker_resume() {
        let r = paused_test_writer();
        let a = r.lease(USER_A, 1, 1);
        let b = r.lease(USER_B, 2, 2);
        r.activate(a.clone()).unwrap();
        let (ack, result) = mpsc::channel();
        {
            let mut p = r.pending.lock().unwrap();
            let sequence = p.next_sequence().unwrap();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::LocalStoreChecked {
                        lease: a.clone(),
                        data: store_with_asset("late"),
                        acknowledgement: ack,
                        admission: None,
                    },
                })
                .unwrap();
        }
        r.activate(b).unwrap();
        r.resume_writer();
        assert_eq!(
            result.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(ClientStateWriteError::StaleLease)
        );
        assert!(r.load_client_state_for_namespace(&a).unwrap().is_none());
    }
    #[test]
    fn activation_waits_for_private_transaction_commit_and_later_a_is_stale() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 1);
        let b = r.lease(USER_B, 2, 2);
        r.activate(a.clone()).unwrap();
        let (entered, at_commit) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let writer = r.writer.clone();
        let old = a.clone();
        let transaction = std::thread::spawn(move || {
            let mut c = open_client_state_connection(&writer.path).unwrap();
            commit_private(&mut c, &writer, &old, 1, &mut 0, |c, user| {
                let tx = c.transaction()?;
                write_meta(&tx, user, "commit-proof", "A")?;
                entered.send(()).unwrap();
                resume.recv().unwrap();
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        });
        at_commit.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            r.pending.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let writer = r.writer.clone();
        let next = b.clone();
        let (done, activated) = mpsc::channel();
        let activation = std::thread::spawn(move || {
            writer.activate(next).unwrap();
            done.send(()).unwrap();
        });
        assert!(activated.recv_timeout(Duration::from_millis(50)).is_err());
        release.send(()).unwrap();
        transaction.join().unwrap();
        activated.recv_timeout(Duration::from_secs(5)).unwrap();
        activation.join().unwrap();
        assert_eq!(
            read_meta(&r.connection(), USER_A, "commit-proof")
                .unwrap()
                .as_deref(),
            Some("A")
        );
        assert_eq!(
            r.persist_client_state_checked_for_namespace(&a, store_with_asset("late")),
            Err(ClientStateWriteError::StaleLease)
        );
        r.flush(&b).unwrap();
    }
    #[test]
    fn deactivation_drops_pending_private_but_preserves_both_device_slots() {
        let r = paused_test_writer();
        let a = r.lease(USER_A, 1, 1);
        r.activate(a.clone()).unwrap();
        let external = tempfile::tempdir().unwrap();
        let export = ExportDirectoryPreference {
            normalized_path: external.path().canonicalize().unwrap(),
        };
        r.queue_private(
            a.clone(),
            Some(store_with_asset("drop")),
            Some(UserProfileData {
                nickname: "drop".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        r.queue_device(Some(settings()), Some(Some(export.clone())))
            .unwrap();
        r.deactivate(&a).unwrap();
        r.resume_writer();
        r.flush_device().unwrap();
        assert!(r.load_client_state_for_namespace(&a).unwrap().is_none());
        assert!(r
            .load_client_user_profile_for_namespace(&a)
            .unwrap()
            .is_none());
        assert_eq!(r.load_device_settings().unwrap(), Some(settings()));
        assert_eq!(r.load_export_directory().unwrap(), Some(export));
    }
    #[test]
    fn checked_device_and_profile_commands_do_not_replay_over_newer_async_slots() {
        let r = paused_test_writer();
        let a = r.lease(USER_A, 1, 1);
        r.activate(a.clone()).unwrap();
        r.queue_device(Some(DeviceSettings::default()), None)
            .unwrap();
        r.queue_private(a.clone(), None, Some(UserProfileData::default()))
            .unwrap();
        let (ack, result) = mpsc::channel();
        let (ack2, result2) = mpsc::channel();
        {
            let mut p = r.pending.lock().unwrap();
            let sequence = p.next_sequence().unwrap();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::DeviceSettingsChecked {
                        data: DeviceSettings::default(),
                        acknowledgement: ack,
                    },
                })
                .unwrap();
            let sequence = p.next_sequence().unwrap();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::UserProfileChecked {
                        lease: a.clone(),
                        data: UserProfileData {
                            nickname: "old".into(),
                            ..Default::default()
                        },
                        acknowledgement: ack2,
                        admission: None,
                    },
                })
                .unwrap();
        }
        r.queue_device(Some(settings()), None).unwrap();
        r.queue_private(
            a.clone(),
            None,
            Some(UserProfileData {
                nickname: "latest".into(),
                ..Default::default()
            }),
        )
        .unwrap();
        r.resume_writer();
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        result2
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        r.flush(&a).unwrap();
        assert_eq!(r.load_device_settings().unwrap(), Some(settings()));
        assert_eq!(
            r.load_client_user_profile_for_namespace(&a)
                .unwrap()
                .unwrap()
                .nickname,
            "latest"
        );
    }
    #[test]
    fn sqlite_round_trip_preserves_normalized_client_state() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 1);
        r.activate(a.clone()).unwrap();
        let mut data = store_with_asset("asset-1");
        data.video_model = "seedance-pro".into();
        data.custom_prompt_times
            .insert("asset-1".into(), "2026-08-12 10:01".into());
        data.custom_prompt_profiles.insert(
            "asset-1".into(),
            CustomPromptProfile {
                name: "Plant prompt".into(),
                ..Default::default()
            },
        );
        data.active_canvas_workspace_id = "monster-generator".into();
        data.canvas_workspaces.insert(
            "plant-growth".into(),
            CanvasWorkspaceData {
                notes: vec![CanvasNoteData {
                    id: "plant-note".into(),
                    content: "番茄".into(),
                    ..Default::default()
                }],
                prompt: "陶盆里的番茄".into(),
                references: vec![ReferenceData {
                    id: "plant-reference".into(),
                    source_path: "private/plant.png".into(),
                }],
                ..Default::default()
            },
        );
        r.persist_client_state_checked_for_namespace(&a, data.clone())
            .unwrap();
        let restored = r.load_client_state_for_namespace(&a).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&restored).unwrap(),
            serde_json::to_value(&data).unwrap()
        );
        r.persist_client_state_checked_for_namespace(&a, LocalStoreData::default())
            .unwrap();
        assert_eq!(row_count(&r.connection(), "asset_references"), 0);
    }
    #[test]
    fn group_preference_is_scoped_by_user_and_device() {
        let r = test_repository_v2();
        r.save_selected_group(USER_A, "device-1", GROUP_A).unwrap();
        r.save_selected_group(USER_A, "device-2", GROUP_B).unwrap();
        r.save_selected_group(USER_B, "device-1", GROUP_B).unwrap();
        assert_eq!(
            r.load_selected_group(USER_A, "device-1")
                .unwrap()
                .as_deref(),
            Some(GROUP_A)
        );
        assert_eq!(
            r.load_selected_group(USER_A, "device-2")
                .unwrap()
                .as_deref(),
            Some(GROUP_B)
        );
        assert_eq!(
            r.load_selected_group(USER_B, "device-1")
                .unwrap()
                .as_deref(),
            Some(GROUP_B)
        );
        for (u, d, g) in [
            ("not-a-uuid", "device", GROUP_A),
            (USER_A, "", GROUP_A),
            (USER_A, "device", "CCCCCCCC-CCCC-4CCC-8CCC-CCCCCCCCCCCC"),
        ] {
            assert!(r.save_selected_group(u, d, g).is_err());
        }
        assert_eq!(row_count(&r.connection(), "billing_context_preferences"), 3);
    }
    #[test]
    fn client_state_writer_retries_after_a_temporary_startup_database_lock() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 1);
        r.activate(a.clone()).unwrap();
        let blocker = r.connection();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(matches!(
            r.persist_client_state_checked_for_namespace(&a, store_with_asset("blocked")),
            Err(ClientStateWriteError::LocalState { .. })
        ));
        blocker.execute_batch("ROLLBACK").unwrap();
        r.persist_client_state_checked_for_namespace(&a, store_with_asset("retry"))
            .unwrap();
        assert_eq!(
            r.load_client_state_for_namespace(&a)
                .unwrap()
                .unwrap()
                .assets[0]
                .id,
            "retry"
        );
    }
    #[test]
    fn core_credit_redemption_intent_survives_exact_owner_sqlite_round_trip() {
        let repository = test_repository_v2();
        let lease = repository.lease(USER_A, 1, 1);
        repository.activate(lease.clone()).unwrap();
        let data: LocalStoreData = serde_json::from_value(serde_json::json!({
            "pending_credit_redemptions_by_owner": { (USER_A): {
                "code": "original-code", "client_request_id": "original-key", "billing_account_group_id": "22222222-2222-4222-8222-222222222222"
            }}
        })).unwrap();
        repository.persist_client_state_checked_for_namespace(&lease, data).unwrap();
        let saved = repository.load_client_state_for_namespace(&lease).unwrap().unwrap();
        let intent = saved.pending_credit_redemptions_by_owner.get(USER_A).unwrap();
        assert_eq!(intent.code, "original-code");
        assert_eq!(intent.client_request_id, "original-key");
        assert_eq!(intent.billing_account_group_id, "22222222-2222-4222-8222-222222222222");
        assert_eq!(saved.pending_credit_redemptions_by_owner.len(), 1);
    }
    #[test]
    fn core_retained_redemption_read_uses_held_connection_and_exact_active_lease() {
        let moved_root = tempfile::tempdir().unwrap();
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 1);
        r.activate(a.clone()).unwrap();
        let data: LocalStoreData = serde_json::from_value(serde_json::json!({
            "pending_credit_redemptions_by_owner": {(USER_A): {
                "code":"retained-code","client_request_id":"retained-key",
                "billing_account_group_id":"22222222-2222-4222-8222-222222222222"
            }}
        })).unwrap();
        r.persist_client_state_checked_for_namespace(&a, data).unwrap();
        let moved = moved_root.path().join("held");
        fs::rename(r.directory.path(), &moved).unwrap();
        fs::create_dir(r.directory.path()).unwrap();
        assert!(!r.path.exists());
        let retained = r.read_retained_redemption_checked(&a, "retained-key").unwrap().unwrap();
        assert_eq!(retained.code, "retained-code");
        assert!(!r.path.exists(), "a retained read must not create a substitute database");
        fs::write(&r.path, b"sentinel: not a database").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&r.path, fs::Permissions::from_mode(0o444)).unwrap();
        }
        assert_eq!(r.read_retained_redemption_checked(&a, "retained-key").unwrap().unwrap().code, "retained-code");
        assert_eq!(fs::read(&r.path).unwrap(), b"sentinel: not a database");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&r.path).unwrap().permissions().mode() & 0o777, 0o444);
        }
        assert!(r.read_retained_redemption_checked(&r.lease(USER_B, 1, 1), "retained-key").is_err());
        assert!(r.read_retained_redemption_checked(&r.lease(USER_A, 2, 1), "retained-key").is_err());
        assert!(r.read_retained_redemption_checked(&a, "different-key").unwrap().is_none());
        r.flush_for_retirement(&a).unwrap().retire_flushed();
        assert!(r.read_retained_redemption_checked(&a, "retained-key").is_err());
        assert_eq!(fs::read(&r.path).unwrap(), b"sentinel: not a database");
    }
    #[test]
    fn video_prompt_draft_survives_sqlite_round_trip_without_changing_image_drafts() {
        let r = test_repository_v2();
        let a = r.lease(USER_A, 1, 1);
        r.activate(a.clone()).unwrap();
        let data=serde_json::from_value(serde_json::json!({"prompt_drafts":{"scene":"image prompt","video_by_owner":{"user-a":{"source_id":"image-a","prompt":"video prompt\n镜头缓慢推进"}}}})).unwrap();
        r.persist_client_state_checked_for_namespace(&a, data)
            .unwrap();
        let value =
            serde_json::to_value(r.load_client_state_for_namespace(&a).unwrap().unwrap()).unwrap();
        assert_eq!(value["prompt_drafts"]["scene"], "image prompt");
        assert_eq!(
            value["prompt_drafts"]["video_by_owner"]["user-a"]["prompt"],
            "video prompt\n镜头缓慢推进"
        );
    }
    #[cfg(unix)]
    #[test]
    fn client_state_database_rejects_symbolic_links() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        fs::write(&target, b"do-not-touch").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_client_state_connection(&link).is_err());
        assert_eq!(fs::read(target).unwrap(), b"do-not-touch");
    }
}
