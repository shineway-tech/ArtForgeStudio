use super::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;

const CLIENT_STATE_FILE_NAME: &str = "client-state.sqlite3";
const CLIENT_STATE_SCHEMA_VERSION: i32 = 1;

enum ClientStateWrite {
    Wake,
    DirectoryLocationsChecked {
        data: DirectoryLocations,
        acknowledgement: mpsc::Sender<std::result::Result<(), String>>,
    },
    LocalStoreChecked {
        data: LocalStoreData,
        acknowledgement: mpsc::Sender<std::result::Result<(), String>>,
    },
    UserProfileChecked {
        data: UserProfileData,
        acknowledgement: mpsc::Sender<std::result::Result<(), String>>,
    },
}

#[derive(Default)]
struct PendingClientState {
    local_store: Option<LocalStoreData>,
    user_profile: Option<UserProfileData>,
}

#[derive(Clone)]
struct ClientStateWriter {
    sender: SyncSender<ClientStateWrite>,
    pending: Arc<Mutex<PendingClientState>>,
}

static CLIENT_STATE_WRITER: OnceLock<ClientStateWriter> = OnceLock::new();

pub(super) fn client_state_path() -> PathBuf {
    app_data_dir().join(CLIENT_STATE_FILE_NAME)
}

pub(super) fn initialize_client_state_repository() -> Result<()> {
    if CLIENT_STATE_WRITER.get().is_some() {
        return Ok(());
    }
    let path = client_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut connection = open_client_state_connection(&path)?;
    migrate_client_state_schema(&mut connection)?;
    drop(connection);

    // One wake-up is enough: ordinary saves replace the pending snapshot, so a
    // slow disk can never accumulate an unbounded queue of full metadata copies.
    let (sender, receiver) = mpsc::sync_channel(1);
    let pending = Arc::new(Mutex::new(PendingClientState::default()));
    let worker_path = path.clone();
    let worker_pending = pending.clone();
    std::thread::Builder::new()
        .name("client-state-writer".to_string())
        .spawn(move || client_state_writer_loop(worker_path, receiver, worker_pending))
        .context("无法启动本地数据库写入线程")?;
    CLIENT_STATE_WRITER
        .set(ClientStateWriter { sender, pending })
        .map_err(|_| anyhow!("本地数据库写入线程重复初始化"))?;
    Ok(())
}

pub(super) fn load_client_state() -> Result<Option<LocalStoreData>> {
    let mut connection = open_initialized_client_state_connection()?;
    let initialized =
        read_meta(&connection, "local_store_initialized")?.is_some_and(|value| value == "1");
    if !initialized {
        return Ok(None);
    }
    let transaction = connection.transaction()?;
    let mut data = read_local_store_transaction(&transaction)?;
    directory_locations().remap_local_store(&mut data);
    transaction.commit()?;
    Ok(Some(data))
}

pub(super) fn load_client_user_profile() -> Result<Option<UserProfileData>> {
    let connection = open_initialized_client_state_connection()?;
    let initialized =
        read_meta(&connection, "user_profile_initialized")?.is_some_and(|value| value == "1");
    if !initialized {
        return Ok(None);
    }
    read_setting_json(&connection, "user_profile").map(Some)
}

pub(super) fn load_directory_locations() -> Result<DirectoryLocations> {
    let connection = open_initialized_client_state_connection()?;
    read_setting_json_or_default(&connection, "directory_locations")
}

pub(super) fn persist_directory_locations_checked(data: DirectoryLocations) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    client_state_writer()?.sender.send(ClientStateWrite::DirectoryLocationsChecked {
        data, acknowledgement: sender,
    }).map_err(|_| anyhow!("本地数据库写入线程已退出"))?;
    receiver.recv().map_err(|_| anyhow!("本地数据库写入线程未返回结果"))?.map_err(anyhow::Error::msg)
}

pub(super) fn persist_client_state_async(data: LocalStoreData) -> Result<()> {
    let writer = client_state_writer()?;
    writer
        .pending
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .local_store = Some(data);
    wake_client_state_writer(writer)
}

pub(super) fn persist_client_state_checked(data: LocalStoreData) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    client_state_writer()?
        .sender
        .send(ClientStateWrite::LocalStoreChecked {
            data,
            acknowledgement: sender,
        })
        .map_err(|_| anyhow!("本地数据库写入线程已退出"))?;
    receiver
        .recv()
        .map_err(|_| anyhow!("本地数据库写入线程未返回结果"))?
        .map_err(anyhow::Error::msg)
}

pub(super) fn persist_client_user_profile_async(data: UserProfileData) -> Result<()> {
    let writer = client_state_writer()?;
    writer
        .pending
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .user_profile = Some(data);
    wake_client_state_writer(writer)
}

pub(super) fn persist_client_user_profile_checked(data: UserProfileData) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    client_state_writer()?
        .sender
        .send(ClientStateWrite::UserProfileChecked {
            data,
            acknowledgement: sender,
        })
        .map_err(|_| anyhow!("本地数据库写入线程已退出"))?;
    receiver
        .recv()
        .map_err(|_| anyhow!("本地数据库写入线程未返回结果"))?
        .map_err(anyhow::Error::msg)
}

fn client_state_writer() -> Result<&'static ClientStateWriter> {
    CLIENT_STATE_WRITER
        .get()
        .ok_or_else(|| anyhow!("本地数据库尚未初始化"))
}

fn wake_client_state_writer(writer: &ClientStateWriter) -> Result<()> {
    match writer.sender.try_send(ClientStateWrite::Wake) {
        Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Disconnected(_)) => anyhow::bail!("本地数据库写入线程已退出"),
    }
}

fn client_state_writer_loop(
    path: PathBuf,
    receiver: Receiver<ClientStateWrite>,
    pending: Arc<Mutex<PendingClientState>>,
) {
    let mut connection = match open_client_state_connection(&path).and_then(|mut connection| {
        migrate_client_state_schema(&mut connection)?;
        Ok(connection)
    }) {
        Ok(connection) => connection,
        Err(error) => {
            fail_pending_client_state_writes(receiver, error.to_string());
            return;
        }
    };

    while let Ok(command) = receiver.recv() {
        match command {
            ClientStateWrite::DirectoryLocationsChecked { data, acknowledgement } => {
                flush_pending_client_state(&mut connection, &pending);
                let result = write_directory_locations(&mut connection, &data).map_err(|error| error.to_string());
                if result.is_ok() { set_directory_locations(data); }
                let _ = acknowledgement.send(result);
                flush_pending_client_state(&mut connection, &pending);
            }
            ClientStateWrite::Wake => {
                flush_pending_client_state(&mut connection, &pending);
            }
            ClientStateWrite::LocalStoreChecked {
                data,
                acknowledgement,
            } => {
                flush_pending_client_state(&mut connection, &pending);
                let result =
                    write_local_store(&mut connection, &data).map_err(|error| error.to_string());
                let _ = acknowledgement.send(result);
                flush_pending_client_state(&mut connection, &pending);
            }
            ClientStateWrite::UserProfileChecked {
                data,
                acknowledgement,
            } => {
                flush_pending_client_state(&mut connection, &pending);
                let result =
                    write_user_profile(&mut connection, &data).map_err(|error| error.to_string());
                let _ = acknowledgement.send(result);
                flush_pending_client_state(&mut connection, &pending);
            }
        }
    }
}

fn flush_pending_client_state(
    connection: &mut Connection,
    pending: &Arc<Mutex<PendingClientState>>,
) {
    let (pending_store, pending_profile) = {
        let mut pending = pending.lock().unwrap_or_else(|error| error.into_inner());
        (pending.local_store.take(), pending.user_profile.take())
    };
    if let Some(data) = pending_store {
        if let Err(error) = write_local_store(connection, &data) {
            eprintln!("failed to persist local client state: {error}");
        }
    }
    if let Some(data) = pending_profile {
        if let Err(error) = write_user_profile(connection, &data) {
            eprintln!("failed to persist local user profile: {error}");
        }
    }
}

fn fail_pending_client_state_writes(receiver: Receiver<ClientStateWrite>, message: String) {
    for command in receiver {
        let acknowledgement = match command {
            ClientStateWrite::Wake => None,
            ClientStateWrite::DirectoryLocationsChecked { acknowledgement, .. } => Some(acknowledgement),
            ClientStateWrite::LocalStoreChecked {
                acknowledgement, ..
            }
            | ClientStateWrite::UserProfileChecked {
                acknowledgement, ..
            } => Some(acknowledgement),
        };
        if let Some(acknowledgement) = acknowledgement {
            let _ = acknowledgement.send(Err(message.clone()));
        }
    }
}

fn open_initialized_client_state_connection() -> Result<Connection> {
    let path = client_state_path();
    let mut connection = open_client_state_connection(&path)?;
    migrate_client_state_schema(&mut connection)?;
    Ok(connection)
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

fn migrate_client_state_schema(connection: &mut Connection) -> Result<()> {
    let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > CLIENT_STATE_SCHEMA_VERSION {
        anyhow::bail!("本地数据库来自更新版本，当前客户端无法安全写入");
    }
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS client_meta (
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
        );",
    )?;
    transaction.pragma_update(None, "user_version", CLIENT_STATE_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn write_local_store(connection: &mut Connection, data: &LocalStoreData) -> Result<()> {
    let mut normalized = data.clone();
    directory_locations().remap_local_store(&mut normalized);
    let data = &normalized;
    let transaction = connection.transaction()?;
    write_asset_collection(&transaction, "generation", &data.generations)?;
    write_asset_collection(&transaction, "asset", &data.assets)?;
    write_notifications(&transaction, &data.notifications)?;
    write_canvas_nodes(&transaction, &data.canvas_notes)?;
    write_canvas_links(&transaction, &data.canvas_links)?;
    write_custom_prompts(&transaction, data)?;

    write_setting_json(&transaction, "image_model", &data.image_model)?;
    write_setting_json(&transaction, "reasoning_model", &data.reasoning_model)?;
    write_setting_json(&transaction, "video_model", &data.video_model)?;
    write_setting_json(&transaction, "prompt_drafts", &data.prompt_drafts)?;
    write_setting_json(
        &transaction,
        "dismissed_prompt_history",
        &data.dismissed_prompt_history,
    )?;
    write_setting_json(
        &transaction,
        "selected_custom_prompts",
        &data.selected_custom_prompts,
    )?;
    write_setting_json(&transaction, "deep_prompt_job_id", &data.deep_prompt_job_id)?;
    write_setting_json(
        &transaction,
        "deep_prompt_jobs_by_owner",
        &data.deep_prompt_jobs_by_owner,
    )?;
    write_setting_json(
        &transaction,
        "deep_prompt_pending_requests_by_owner",
        &data.deep_prompt_pending_requests_by_owner,
    )?;
    write_setting_json(
        &transaction,
        "deep_prompt_bindings",
        &data.deep_prompt_bindings,
    )?;
    write_setting_json(
        &transaction,
        "contact_popup_dismissed",
        &data.contact_popup_dismissed,
    )?;
    write_meta(&transaction, "local_store_initialized", "1")?;
    transaction.commit()?;
    Ok(())
}

fn write_directory_locations(connection: &mut Connection, data: &DirectoryLocations) -> Result<()> {
    let transaction = connection.transaction()?;
    write_setting_json(&transaction, "directory_locations", data)?;
    transaction.commit()?;
    Ok(())
}

fn write_user_profile(connection: &mut Connection, data: &UserProfileData) -> Result<()> {
    let transaction = connection.transaction()?;
    write_setting_json(&transaction, "user_profile", data)?;
    write_meta(&transaction, "user_profile_initialized", "1")?;
    transaction.commit()?;
    Ok(())
}

fn write_asset_collection(
    transaction: &Transaction<'_>,
    collection: &str,
    assets: &[StoredAssetData],
) -> Result<()> {
    transaction.execute(
        "DELETE FROM assets WHERE collection = ?1
         AND id NOT IN (SELECT value FROM json_each(?2))",
        params![
            collection,
            serde_json::to_string(&assets.iter().map(|item| &item.id).collect::<Vec<_>>())?
        ],
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO assets (
            collection, id, position, conversation_id, title, category, kind, time,
            prompt, ratio, quality, model, origin, width, height, source_path,
            cutout_done, remove_black_done, upscale_done
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19
        ) ON CONFLICT(collection, id) DO UPDATE SET
            position=excluded.position, conversation_id=excluded.conversation_id,
            title=excluded.title, category=excluded.category, kind=excluded.kind,
            time=excluded.time, prompt=excluded.prompt, ratio=excluded.ratio,
            quality=excluded.quality, model=excluded.model, origin=excluded.origin,
            width=excluded.width, height=excluded.height, source_path=excluded.source_path,
            cutout_done=excluded.cutout_done,
            remove_black_done=excluded.remove_black_done,
            upscale_done=excluded.upscale_done
        WHERE position != excluded.position OR conversation_id != excluded.conversation_id
            OR title != excluded.title OR category != excluded.category OR kind != excluded.kind
            OR time != excluded.time OR prompt != excluded.prompt OR ratio != excluded.ratio
            OR quality != excluded.quality OR model != excluded.model OR origin != excluded.origin
            OR width != excluded.width OR height != excluded.height
            OR source_path != excluded.source_path OR cutout_done != excluded.cutout_done
            OR remove_black_done != excluded.remove_black_done
            OR upscale_done != excluded.upscale_done",
    )?;
    for (position, asset) in assets.iter().enumerate() {
        statement.execute(params![
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
            "DELETE FROM asset_references WHERE collection = ?1 AND asset_id = ?2
             AND position >= ?3",
            params![collection, asset.id, asset.reference_paths.len() as i64],
        )?;
        for (reference_position, path) in asset.reference_paths.iter().enumerate() {
            transaction.execute(
                "INSERT INTO asset_references(collection, asset_id, position, path)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(collection, asset_id, position) DO UPDATE SET path=excluded.path
                 WHERE path != excluded.path",
                params![collection, asset.id, reference_position as i64, path],
            )?;
        }
    }
    Ok(())
}

fn write_notifications(
    transaction: &Transaction<'_>,
    notifications: &[NotificationData],
) -> Result<()> {
    delete_missing_ids(
        transaction,
        "notifications",
        notifications.iter().map(|item| item.id.as_str()),
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO notifications(id, position, title, model, time, reason, success, read)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET position=excluded.position, title=excluded.title,
            model=excluded.model, time=excluded.time, reason=excluded.reason,
            success=excluded.success, read=excluded.read
         WHERE position != excluded.position OR title != excluded.title OR model != excluded.model
            OR time != excluded.time OR reason != excluded.reason
            OR success != excluded.success OR read != excluded.read",
    )?;
    for (position, item) in notifications.iter().enumerate() {
        statement.execute(params![
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

fn write_canvas_nodes(transaction: &Transaction<'_>, nodes: &[CanvasNoteData]) -> Result<()> {
    delete_missing_ids(
        transaction,
        "canvas_nodes",
        nodes.iter().map(|item| item.id.as_str()),
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO canvas_nodes(
            id, position, kind, content, x, y, width, height, parent_group_id,
            z_index, image_path, font_size
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET position=excluded.position, kind=excluded.kind,
            content=excluded.content, x=excluded.x, y=excluded.y, width=excluded.width,
            height=excluded.height, parent_group_id=excluded.parent_group_id,
            z_index=excluded.z_index, image_path=excluded.image_path,
            font_size=excluded.font_size
         WHERE position != excluded.position OR kind != excluded.kind OR content != excluded.content
            OR x != excluded.x OR y != excluded.y OR width != excluded.width
            OR height != excluded.height OR parent_group_id != excluded.parent_group_id
            OR z_index != excluded.z_index OR image_path != excluded.image_path
            OR font_size != excluded.font_size",
    )?;
    for (position, item) in nodes.iter().enumerate() {
        statement.execute(params![
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

fn write_canvas_links(transaction: &Transaction<'_>, links: &[CanvasLinkData]) -> Result<()> {
    delete_missing_ids(
        transaction,
        "canvas_links",
        links.iter().map(|item| item.id.as_str()),
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO canvas_links(id, position, source_id, target_id, flow_reversed)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET position=excluded.position,
            source_id=excluded.source_id, target_id=excluded.target_id,
            flow_reversed=excluded.flow_reversed
         WHERE position != excluded.position OR source_id != excluded.source_id
            OR target_id != excluded.target_id OR flow_reversed != excluded.flow_reversed",
    )?;
    for (position, item) in links.iter().enumerate() {
        statement.execute(params![
            item.id,
            position as i64,
            item.source_id,
            item.target_id,
            item.flow_reversed,
        ])?;
    }
    Ok(())
}

fn write_custom_prompts(transaction: &Transaction<'_>, data: &LocalStoreData) -> Result<()> {
    delete_missing_text_values(
        transaction,
        "custom_prompts",
        "prompt",
        &data.custom_prompts,
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT INTO custom_prompts(prompt, position, created_at, profile_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(prompt) DO UPDATE SET position=excluded.position,
            created_at=excluded.created_at, profile_json=excluded.profile_json
         WHERE position != excluded.position OR created_at != excluded.created_at
            OR profile_json != excluded.profile_json",
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
    table: &str,
    ids: impl Iterator<Item = &'a str>,
) -> Result<()> {
    let ids = ids.collect::<Vec<_>>();
    delete_missing_text_values(transaction, table, "id", &ids)
}

fn delete_missing_text_values<T: AsRef<str>>(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
    values: &[T],
) -> Result<()> {
    let sql =
        format!("DELETE FROM {table} WHERE {column} NOT IN (SELECT value FROM json_each(?1))");
    let values = values
        .iter()
        .map(|value| value.as_ref())
        .collect::<Vec<_>>();
    transaction.execute(&sql, params![serde_json::to_string(&values)?])?;
    Ok(())
}

fn read_local_store_transaction(transaction: &Transaction<'_>) -> Result<LocalStoreData> {
    let custom_prompt_rows = read_custom_prompts(transaction)?;
    let mut custom_prompts = Vec::with_capacity(custom_prompt_rows.len());
    let mut custom_prompt_times = BTreeMap::new();
    let mut custom_prompt_profiles = BTreeMap::new();
    for (prompt, created_at, profile) in custom_prompt_rows {
        custom_prompt_times.insert(prompt.clone(), created_at);
        custom_prompt_profiles.insert(prompt.clone(), profile);
        custom_prompts.push(prompt);
    }
    Ok(LocalStoreData {
        generations: read_assets(transaction, "generation")?,
        assets: read_assets(transaction, "asset")?,
        notifications: read_notifications(transaction)?,
        image_model: read_setting_json_or_default(transaction, "image_model")?,
        reasoning_model: read_setting_json_or_default(transaction, "reasoning_model")?,
        video_model: read_setting_json_or_default(transaction, "video_model")?,
        prompt_drafts: read_setting_json_or_default(transaction, "prompt_drafts")?,
        dismissed_prompt_history: read_setting_json_or_default(
            transaction,
            "dismissed_prompt_history",
        )?,
        custom_prompts,
        selected_custom_prompts: read_setting_json_or_default(
            transaction,
            "selected_custom_prompts",
        )?,
        custom_prompt_times,
        custom_prompt_profiles,
        canvas_notes: read_canvas_nodes(transaction)?,
        canvas_links: read_canvas_links(transaction)?,
        deep_prompt_job_id: read_setting_json_or_default(transaction, "deep_prompt_job_id")?,
        deep_prompt_jobs_by_owner: read_setting_json_or_default(
            transaction,
            "deep_prompt_jobs_by_owner",
        )?,
        deep_prompt_pending_requests_by_owner: read_setting_json_or_default(
            transaction,
            "deep_prompt_pending_requests_by_owner",
        )?,
        deep_prompt_bindings: read_setting_json_or_default(transaction, "deep_prompt_bindings")?,
        contact_popup_dismissed: read_setting_json_or_default(
            transaction,
            "contact_popup_dismissed",
        )?,
    })
}

fn read_assets(transaction: &Transaction<'_>, collection: &str) -> Result<Vec<StoredAssetData>> {
    let mut statement = transaction.prepare(
        "SELECT id, conversation_id, title, category, kind, time, prompt, ratio,
                quality, model, origin, width, height, source_path,
                cutout_done, remove_black_done, upscale_done
         FROM assets WHERE collection = ?1 ORDER BY position, id",
    )?;
    let mut rows = statement.query(params![collection])?;
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
            reference_paths: read_asset_references(transaction, collection, &id)?,
            cutout_done: row.get(14)?,
            remove_black_done: row.get(15)?,
            upscale_done: row.get(16)?,
        });
    }
    Ok(assets)
}

fn read_asset_references(
    transaction: &Transaction<'_>,
    collection: &str,
    asset_id: &str,
) -> Result<Vec<String>> {
    let mut statement = transaction.prepare_cached(
        "SELECT path FROM asset_references
         WHERE collection = ?1 AND asset_id = ?2 ORDER BY position",
    )?;
    let values = statement
        .query_map(params![collection, asset_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(values)
}

fn read_notifications(transaction: &Transaction<'_>) -> Result<Vec<NotificationData>> {
    let mut statement = transaction.prepare(
        "SELECT id, title, model, time, reason, success, read
         FROM notifications ORDER BY position, id",
    )?;
    let values = statement
        .query_map([], |row| {
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

fn read_canvas_nodes(transaction: &Transaction<'_>) -> Result<Vec<CanvasNoteData>> {
    let mut statement = transaction.prepare(
        "SELECT id, kind, content, x, y, width, height, parent_group_id,
                z_index, image_path, font_size
         FROM canvas_nodes ORDER BY position, id",
    )?;
    let values = statement
        .query_map([], |row| {
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

fn read_canvas_links(transaction: &Transaction<'_>) -> Result<Vec<CanvasLinkData>> {
    let mut statement = transaction.prepare(
        "SELECT id, source_id, target_id, flow_reversed
         FROM canvas_links ORDER BY position, id",
    )?;
    let values = statement
        .query_map([], |row| {
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
) -> Result<Vec<(String, String, CustomPromptProfile)>> {
    let mut statement = transaction.prepare(
        "SELECT prompt, created_at, profile_json
         FROM custom_prompts ORDER BY position, prompt",
    )?;
    let mut rows = statement.query([])?;
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
    key: &str,
    value: &impl Serialize,
) -> Result<()> {
    let value = serde_json::to_string(value)?;
    transaction.execute(
        "INSERT INTO client_settings(key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json
         WHERE value_json != excluded.value_json",
        params![key, value],
    )?;
    Ok(())
}

fn read_setting_json<T: DeserializeOwned>(connection: &Connection, key: &str) -> Result<T> {
    let value: String = connection
        .query_row(
            "SELECT value_json FROM client_settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow!("本地数据库缺少设置项 {key}"))?;
    Ok(serde_json::from_str(&value)?)
}

fn read_setting_json_or_default<T: DeserializeOwned + Default>(
    connection: &Connection,
    key: &str,
) -> Result<T> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value_json FROM client_settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    value
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .unwrap_or_else(|| Ok(T::default()))
}

fn write_meta(transaction: &Transaction<'_>, key: &str, value: &str) -> Result<()> {
    transaction.execute(
        "INSERT INTO client_meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn read_meta(connection: &Connection, key: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT value FROM client_meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_prompt_draft_survives_sqlite_round_trip_without_changing_image_drafts() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate_client_state_schema(&mut connection).unwrap();
        let data: LocalStoreData = serde_json::from_value(serde_json::json!({
            "prompt_drafts": {
                "scene": "image prompt",
                "video_by_owner": {
                    "user-a": {"source_id": "image-a", "prompt": "video prompt\n镜头缓慢推进"}
                }
            }
        })).unwrap();
        write_local_store(&mut connection, &data).unwrap();
        let transaction = connection.transaction().unwrap();
        let restored = serde_json::to_value(read_local_store_transaction(&transaction).unwrap()).unwrap();
        assert_eq!(restored["prompt_drafts"]["scene"], "image prompt");
        assert_eq!(
            restored["prompt_drafts"]["video_by_owner"]["user-a"]["prompt"],
            "video prompt\n镜头缓慢推进"
        );
    }

    #[test]
    fn sqlite_round_trip_preserves_normalized_client_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("client-state.sqlite3");
        let mut connection = open_client_state_connection(&path).expect("open");
        migrate_client_state_schema(&mut connection).expect("schema");
        let data = LocalStoreData {
            video_model: "seedance-pro".to_string(),
            assets: vec![StoredAssetData {
                id: "asset-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                title: "Title".to_string(),
                category: "scene".to_string(),
                kind: "game".to_string(),
                time: "2026-08-12 10:00".to_string(),
                prompt: "prompt".to_string(),
                ratio: "16:9".to_string(),
                quality: "2k".to_string(),
                model: "model".to_string(),
                origin: "generation".to_string(),
                width: 1600,
                height: 900,
                source_path: "/tmp/image.png".to_string(),
                reference_paths: vec!["/tmp/reference.png".to_string()],
                cutout_done: true,
                remove_black_done: false,
                upscale_done: true,
            }],
            custom_prompts: vec!["custom".to_string()],
            custom_prompt_times: BTreeMap::from([(
                "custom".to_string(),
                "2026-08-12 10:01".to_string(),
            )]),
            ..LocalStoreData::default()
        };
        write_local_store(&mut connection, &data).expect("write");
        let transaction = connection.transaction().expect("transaction");
        let restored = read_local_store_transaction(&transaction).expect("read");
        transaction.commit().expect("commit");
        assert_eq!(restored.assets.len(), 1);
        assert_eq!(
            restored.assets[0].reference_paths,
            data.assets[0].reference_paths
        );
        assert_eq!(restored.assets[0].width, 1600);
        assert_eq!(restored.video_model, data.video_model);
        assert_eq!(restored.custom_prompts, data.custom_prompts);
        assert_eq!(
            restored.custom_prompt_times.get("custom"),
            data.custom_prompt_times.get("custom")
        );

        write_local_store(&mut connection, &LocalStoreData::default()).expect("remove state");
        let asset_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
            .expect("asset count");
        let reference_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM asset_references", [], |row| {
                row.get(0)
            })
            .expect("reference count");
        assert_eq!(asset_count, 0);
        assert_eq!(reference_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn client_state_database_rejects_symbolic_links() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("tempdir");
        let target = directory.path().join("target.sqlite3");
        let link = directory.path().join("client-state.sqlite3");
        fs::write(&target, b"do-not-touch").expect("target");
        symlink(&target, &link).expect("symlink");

        assert!(open_client_state_connection(&link).is_err());
        assert_eq!(fs::read(&target).expect("read target"), b"do-not-touch");
    }

    #[test]
    fn directory_locations_are_atomic_and_survive_database_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.sqlite3");
        let old = directory.path().join("old");
        let new = directory.path().join("new");
        let mut connection = open_client_state_connection(&path).unwrap();
        migrate_client_state_schema(&mut connection).unwrap();
        let defaults: DirectoryLocations = read_setting_json_or_default(&connection, "directory_locations").unwrap();
        assert!(defaults.relocations.is_empty());
        let config = defaults.migrated("output", old.clone(), new.clone()).unwrap();
        write_directory_locations(&mut connection, &config).unwrap();
        drop(connection);
        let mut reopened = open_client_state_connection(&path).unwrap();
        let restored: DirectoryLocations = read_setting_json(&reopened, "directory_locations").unwrap();
        assert_eq!(restored.directory("output"), Some(new));
        let mut historical = old.join("asset.png").display().to_string();
        restored.remap(&mut historical);
        assert_eq!(PathBuf::from(historical), directory.path().join("new/asset.png"));
        reopened.execute_batch("CREATE TRIGGER reject_location_update BEFORE UPDATE ON client_settings WHEN NEW.key='directory_locations' BEGIN SELECT RAISE(ABORT, 'test disk failure'); END;").unwrap();
        assert!(write_directory_locations(&mut reopened, &DirectoryLocations::default()).is_err());
        let retained: DirectoryLocations = read_setting_json(&reopened, "directory_locations").unwrap();
        assert_eq!(retained.output, restored.output);
        assert_eq!(retained.relocations.len(), 1);
    }
}
