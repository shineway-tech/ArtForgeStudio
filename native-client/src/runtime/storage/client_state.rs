use super::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::OnceLock;

const CLIENT_STATE_FILE_NAME: &str = "client-state.sqlite3";
const CLIENT_STATE_SCHEMA_VERSION: i32 = 2;
pub(super) const KNOWN_DEVICE_SETTING_KEYS: [&str; 8] = [
    "export_directory",
    "theme_id",
    "card_style",
    "language",
    "close_behavior",
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
    LocalState { message: String },
}
impl std::fmt::Display for ClientStateWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleLease => f.write_str("用户命名空间已失效"),
            Self::LocalState { message } => f.write_str(message),
        }
    }
}
impl std::error::Error for ClientStateWriteError {}
type WriteResult = std::result::Result<(), ClientStateWriteError>;
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
    LocalStoreChecked {
        lease: NamespaceLease,
        data: LocalStoreData,
        acknowledgement: Ack,
    },
    UserProfileChecked {
        lease: NamespaceLease,
        data: UserProfileData,
        acknowledgement: Ack,
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
    fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
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
        pending.clear_private();
        pending.active = Some(lease);
        Ok(())
    }
    pub(super) fn deactivate(&self, lease: &NamespaceLease) -> WriteResult {
        let mut pending = self.pending.lock().map_err(local_error)?;
        pending.require(lease)?;
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
                pending.require(lease)?;
            }
            let sequence = pending.next_sequence();
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
        pending.require(&lease)?;
        let sequence = pending.next_sequence();
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
        let sequence = pending.next_sequence();
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
    pub(super) fn flush_device(&self) -> WriteResult {
        self.checked(None, |acknowledgement| ClientStateWrite::FlushDevice {
            acknowledgement,
        })
    }
    fn persist_client_state_checked_for_namespace(
        &self,
        lease: &NamespaceLease,
        data: LocalStoreData,
    ) -> WriteResult {
        self.checked(Some(lease), |acknowledgement| {
            ClientStateWrite::LocalStoreChecked {
                lease: lease.clone(),
                data,
                acknowledgement,
            }
        })
    }
    fn persist_client_user_profile_checked_for_namespace(
        &self,
        lease: &NamespaceLease,
        data: UserProfileData,
    ) -> WriteResult {
        self.checked(Some(lease), |acknowledgement| {
            ClientStateWrite::UserProfileChecked {
                lease: lease.clone(),
                data,
                acknowledgement,
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
    fn load_client_state_for_namespace(
        &self,
        lease: &NamespaceLease,
    ) -> Result<Option<LocalStoreData>> {
        let mut connection = open_client_state_connection(&self.path)?;
        let user = lease.namespace.user_public_id();
        let tx = connection.transaction()?;
        if read_meta(&tx, user, "local_store_initialized")?.as_deref() != Some("1") {
            return Ok(None);
        }
        let data = read_local_store_transaction(&tx, user)?;
        tx.commit()?;
        Ok(Some(data))
    }
    fn load_client_user_profile_for_namespace(
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
    fn load_selected_group(&self, user: &str, device: &str) -> Result<Option<String>> {
        validate_uuid(user)?;
        anyhow::ensure!(!device.is_empty(), "设备标识不能为空");
        Ok(open_client_state_connection(&self.path)?.query_row("SELECT account_group_id FROM billing_context_preferences WHERE user_public_id = ?1 AND device_installation_id = ?2", params![user, device], |row| row.get(0)).optional()?)
    }
    fn save_selected_group(&self, user: &str, device: &str, group: &str) -> Result<()> {
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
}
impl CommittedSequences {
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
        result = commit_private(connection, writer, &lease, seq, &mut last.store, |c, u| {
            write_local_store(c, u, &data)
        });
        last.store_error = result.as_ref().err().map(|error| (lease, error.clone()));
    }
    if let Some((seq, lease, data)) = profile {
        let next = commit_private(
            connection,
            writer,
            &lease,
            seq,
            &mut last.profile,
            |c, u| write_user_profile(c, u, &data),
        );
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
        ClientStateWrite::LocalStoreChecked {
            lease,
            data,
            acknowledgement,
        } => {
            let _ = acknowledgement.send(commit_private(
                connection,
                writer,
                &lease,
                seq,
                &mut last.store,
                |c, u| write_local_store(c, u, &data),
            ));
        }
        ClientStateWrite::UserProfileChecked {
            lease,
            data,
            acknowledgement,
        } => {
            let _ = acknowledgement.send(commit_private(
                connection,
                writer,
                &lease,
                seq,
                &mut last.profile,
                |c, u| write_user_profile(c, u, &data),
            ));
        }
        ClientStateWrite::DeviceSettingsChecked {
            data,
            acknowledgement,
        } => {
            let result = if seq > last.device {
                write_device_settings(connection, &data).map_err(local_error)
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
                write_export_directory(connection, writer.data_root.as_ref(), data)
                    .map_err(local_error)
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
            if let Ok(value) = serde_json::from_str::<String>(&bytes) {
                values[*key] = value.into();
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
            value[*key] = serde_json::from_str::<String>(bytes)?.into();
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
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Condvar,
    };
    const USER_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const USER_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const GROUP_A: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    const GROUP_B: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
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
    struct Fixture {
        writer: ClientStateWriter,
        directory: tempfile::TempDir,
        pause: Arc<(Mutex<bool>, Condvar)>,
        private_paused: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl std::ops::Deref for Fixture {
        type Target = ClientStateWriter;
        fn deref(&self) -> &Self::Target {
            &self.writer
        }
    }
    impl Fixture {
        fn new(paused: bool, private_paused: bool) -> Self {
            let directory = tempfile::tempdir().unwrap();
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
                                | ClientStateWrite::LocalStoreChecked { .. }
                                | ClientStateWrite::UserProfileChecked { .. }
                                | ClientStateWrite::Flush { .. }
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
        fn data_root_capability_arc(&self) -> Arc<DataRootCapability> {
            self.data_root.clone()
        }
        fn lease(&self, user: &str, auth_epoch: u64, namespace_epoch: u64) -> NamespaceLease {
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
            self.worker.take().unwrap().join().unwrap();
        }
    }
    fn test_repository_v2() -> Fixture {
        Fixture::new(false, false)
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
        assert_eq!(raw.len(), 8);
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
        assert_eq!(read_device_rows(&r.connection()).unwrap().len(), 8);
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
            let sequence = p.next_sequence();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::LocalStoreChecked {
                        lease: a.clone(),
                        data: store_with_asset("checked"),
                        acknowledgement: ack,
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
            let sequence = p.next_sequence();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::LocalStoreChecked {
                        lease: a.clone(),
                        data: store_with_asset("checked"),
                        acknowledgement: ack,
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
            let sequence = p.next_sequence();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::LocalStoreChecked {
                        lease: a.clone(),
                        data: store_with_asset("late"),
                        acknowledgement: ack,
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
            let sequence = p.next_sequence();
            r.sender
                .send(QueuedWrite {
                    sequence,
                    command: ClientStateWrite::DeviceSettingsChecked {
                        data: DeviceSettings::default(),
                        acknowledgement: ack,
                    },
                })
                .unwrap();
            let sequence = p.next_sequence();
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
