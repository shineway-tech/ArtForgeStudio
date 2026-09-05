use super::user_namespace::{
    ManagedFileKey, ManagedFileMetadata, ManagedUserArea, NamespaceManagedFile,
    NamespaceManagedFileCheck, NamespaceStorageAuthority, StableFileIdentity,
};
use rusqlite::{params, Connection, Row, Transaction};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub(super) const FILE_INDEX_SCHEMA_VERSION: i32 = 2;
const BUSY_TIMEOUT: Duration = Duration::from_millis(3_000);
#[derive(Debug, Error)]
pub(super) enum FileIndexError {
    #[error("SQLite file index error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("file index I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("the file index is damaged: {0}")]
    Damaged(String),
    #[error("file index schema {found} exceeds supported {supported}")]
    UnsupportedSchema { found: i32, supported: i32 },
    #[error("invalid file index value: {0}")]
    InvalidValue(String),
    #[error("file index connection lock was poisoned")]
    LockPoisoned,
    #[error("an explicit namespace authority is required")]
    NamespaceRequired,
    #[error("the indexed managed file is missing")]
    MissingManagedFile,
    #[error("file index changed during discovery")]
    ConcurrentChange,
    #[error("recovery references are unavailable")]
    RecoveryReferencesUnavailable,
    #[error("managed file capability failed: {0}")]
    Capability(#[source] anyhow::Error),
}
pub(super) type FileIndexResult<T> = std::result::Result<T, FileIndexError>;
#[derive(Clone)]
pub(super) struct FileIndex {
    database_path: Arc<PathBuf>,
    connection: Arc<Mutex<Connection>>,
}
const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS managed_files (
    id               INTEGER PRIMARY KEY,
    path             TEXT NOT NULL UNIQUE,
    kind             TEXT NOT NULL CHECK(length(kind) > 0),
    byte_size        INTEGER NOT NULL DEFAULT 0 CHECK(byte_size >= 0),
    managed          INTEGER NOT NULL DEFAULT 0 CHECK(managed IN (0, 1)),
    retention_policy TEXT NOT NULL CHECK(length(retention_policy) > 0),
    created_at       INTEGER NOT NULL,
    last_accessed_at INTEGER NOT NULL,
    pending_delete   INTEGER NOT NULL DEFAULT 0 CHECK(pending_delete IN (0, 1))
);

CREATE TABLE IF NOT EXISTS file_references (
    file_id    INTEGER NOT NULL,
    owner_type TEXT NOT NULL CHECK(length(owner_type) > 0),
    owner_id   TEXT NOT NULL CHECK(length(owner_id) > 0),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (file_id, owner_type, owner_id),
    FOREIGN KEY (file_id) REFERENCES managed_files(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS preview_cache (
    id                 INTEGER PRIMARY KEY,
    source_file_id     INTEGER NOT NULL,
    preview_file_id    INTEGER NOT NULL UNIQUE,
    purpose            TEXT NOT NULL CHECK(length(purpose) > 0),
    longest_edge       INTEGER NOT NULL CHECK(longest_edge > 0),
    source_size        INTEGER NOT NULL CHECK(source_size >= 0),
    source_mtime_ns    INTEGER NOT NULL,
    cache_version      INTEGER NOT NULL CHECK(cache_version > 0),
    status             TEXT NOT NULL CHECK(length(status) > 0),
    last_accessed_at   INTEGER NOT NULL,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    UNIQUE (source_file_id, purpose, longest_edge, cache_version),
    FOREIGN KEY (source_file_id) REFERENCES managed_files(id) ON DELETE CASCADE,
    FOREIGN KEY (preview_file_id) REFERENCES managed_files(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS managed_files_kind
    ON managed_files(kind);
CREATE INDEX IF NOT EXISTS managed_files_pending_delete
    ON managed_files(pending_delete, kind);
CREATE INDEX IF NOT EXISTS file_references_owner
    ON file_references(owner_type, owner_id);
CREATE INDEX IF NOT EXISTS preview_cache_lru
    ON preview_cache(last_accessed_at, id);
CREATE INDEX IF NOT EXISTS preview_cache_source
    ON preview_cache(source_file_id);
CREATE INDEX IF NOT EXISTS preview_cache_status
    ON preview_cache(status, updated_at);
"#;

const SCHEMA_V2: &str = r#"
CREATE TABLE managed_files (
    id               INTEGER PRIMARY KEY,
    user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
    managed_area TEXT NOT NULL CHECK(length(managed_area)>0),
    physical_identity BLOB NOT NULL CHECK(typeof(physical_identity)='blob' AND ((length(physical_identity)=17 AND substr(physical_identity,1,1)=X'01') OR (length(physical_identity)=25 AND substr(physical_identity,1,1)=X'02'))),
    path             TEXT NOT NULL,
    kind             TEXT NOT NULL CHECK(length(kind) > 0),
    byte_size        INTEGER NOT NULL DEFAULT 0 CHECK(byte_size >= 0),
    managed          INTEGER NOT NULL DEFAULT 0 CHECK(managed IN (0, 1)),
    retention_policy TEXT NOT NULL CHECK(length(retention_policy) > 0),
    created_at       INTEGER NOT NULL,
    last_accessed_at INTEGER NOT NULL,
    pending_delete   INTEGER NOT NULL DEFAULT 0 CHECK(pending_delete IN (0, 1)),
    UNIQUE(user_public_id, managed_area, path),
    UNIQUE(user_public_id, physical_identity)
);

CREATE TABLE file_references (
    user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
    file_id    INTEGER NOT NULL,
    owner_type TEXT NOT NULL CHECK(length(owner_type) > 0),
    owner_id   TEXT NOT NULL CHECK(length(owner_id) > 0),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_public_id, file_id, owner_type, owner_id),
    FOREIGN KEY (file_id) REFERENCES managed_files(id) ON DELETE CASCADE
);

CREATE TABLE preview_cache (
    user_public_id TEXT NOT NULL CHECK(length(user_public_id)=36),
    id                 INTEGER PRIMARY KEY,
    source_file_id     INTEGER NOT NULL,
    preview_file_id    INTEGER NOT NULL UNIQUE,
    purpose            TEXT NOT NULL CHECK(length(purpose) > 0),
    longest_edge       INTEGER NOT NULL CHECK(longest_edge > 0),
    source_size        INTEGER NOT NULL CHECK(source_size >= 0),
    source_mtime_ns    INTEGER NOT NULL,
    cache_version      INTEGER NOT NULL CHECK(cache_version > 0),
    status             TEXT NOT NULL CHECK(length(status) > 0),
    last_accessed_at   INTEGER NOT NULL,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    UNIQUE (user_public_id, source_file_id, purpose, longest_edge, cache_version),
    FOREIGN KEY (source_file_id) REFERENCES managed_files(id) ON DELETE CASCADE,
    FOREIGN KEY (preview_file_id) REFERENCES managed_files(id) ON DELETE CASCADE
);

CREATE INDEX managed_files_v2_user_kind
    ON managed_files(user_public_id, kind);
CREATE INDEX managed_files_v2_user_pending_delete
    ON managed_files(user_public_id, pending_delete, kind);
CREATE INDEX file_references_v2_user_owner
    ON file_references(user_public_id, owner_type, owner_id);
CREATE INDEX preview_cache_v2_user_lru
    ON preview_cache(user_public_id, last_accessed_at, id);
CREATE INDEX preview_cache_v2_user_source
    ON preview_cache(user_public_id, source_file_id);
CREATE INDEX preview_cache_v2_user_status
    ON preview_cache(user_public_id, status, updated_at);
CREATE TRIGGER file_references_v2_namespace_insert
BEFORE INSERT ON file_references
WHEN NEW.user_public_id != (SELECT user_public_id FROM managed_files WHERE id = NEW.file_id)
BEGIN SELECT RAISE(ABORT, 'file reference namespace mismatch'); END;
CREATE TRIGGER preview_cache_v2_namespace_insert
BEFORE INSERT ON preview_cache
WHEN NEW.user_public_id != (SELECT user_public_id FROM managed_files WHERE id = NEW.source_file_id)
OR NEW.user_public_id != (SELECT user_public_id FROM managed_files WHERE id = NEW.preview_file_id)
BEGIN SELECT RAISE(ABORT, 'preview namespace mismatch'); END;
CREATE TRIGGER file_references_v2_namespace_update
BEFORE UPDATE OF user_public_id, file_id ON file_references
WHEN NEW.user_public_id != (SELECT user_public_id FROM managed_files WHERE id = NEW.file_id)
BEGIN SELECT RAISE(ABORT, 'file reference namespace mismatch'); END;
CREATE TRIGGER preview_cache_v2_namespace_update
BEFORE UPDATE OF user_public_id, source_file_id, preview_file_id ON preview_cache
WHEN NEW.user_public_id != (SELECT user_public_id FROM managed_files WHERE id = NEW.source_file_id)
OR NEW.user_public_id != (SELECT user_public_id FROM managed_files WHERE id = NEW.preview_file_id)
BEGIN SELECT RAISE(ABORT, 'preview namespace mismatch'); END;
"#;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ManagedFileId(pub(super) i64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ManagedFileRegistration {
    pub(super) path: PathBuf,
    pub(super) kind: String,
    pub(super) byte_size: u64,
    pub(super) managed: bool,
    pub(super) retention_policy: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileReferenceRegistration {
    pub(super) file: ManagedFileRegistration,
    pub(super) owner_type: String,
    pub(super) owner_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ManagedFileRecord {
    pub(super) user_public_id: String,
    pub(super) managed_area: ManagedUserArea,
    pub(super) physical_identity: StableFileIdentity,
    pub(super) id: ManagedFileId,
    pub(super) path: PathBuf,
    pub(super) kind: String,
    pub(super) byte_size: u64,
    pub(super) managed: bool,
    pub(super) retention_policy: String,
    pub(super) created_at: i64,
    pub(super) last_accessed_at: i64,
    pub(super) pending_delete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileKindStats {
    pub(super) kind: String,
    pub(super) file_count: u64,
    pub(super) byte_size: u64,
    pub(super) managed_count: u64,
    pub(super) pending_delete_count: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct PreviewKey {
    pub(super) source_file_id: ManagedFileId,
    pub(super) purpose: String,
    pub(super) longest_edge: u32,
    pub(super) cache_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PreviewRegistration {
    pub(super) key: PreviewKey,
    pub(super) preview_file_id: ManagedFileId,
    pub(super) source_size: u64,
    pub(super) source_mtime_ns: i64,
    pub(super) status: String,
    pub(super) last_accessed_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PreviewCacheRecord {
    pub(super) user_public_id: String,
    pub(super) source_area: ManagedUserArea,
    pub(super) preview_area: ManagedUserArea,
    pub(super) source_identity: StableFileIdentity,
    pub(super) preview_identity: StableFileIdentity,
    pub(super) id: i64,
    pub(super) source_file_id: ManagedFileId,
    pub(super) preview_file_id: ManagedFileId,
    pub(super) source_path: PathBuf,
    pub(super) preview_path: PathBuf,
    pub(super) preview_byte_size: u64,
    pub(super) purpose: String,
    pub(super) longest_edge: u32,
    pub(super) source_size: u64,
    pub(super) source_mtime_ns: i64,
    pub(super) cache_version: u32,
    pub(super) status: String,
    pub(super) last_accessed_at: i64,
    pub(super) created_at: i64,
    pub(super) updated_at: i64,
}

pub(super) struct NamespacedManagedFileRegistration {
    file: NamespaceManagedFile,
    kind: String,
    retention_policy: String,
}
impl NamespacedManagedFileRegistration {
    pub(super) fn new(
        authority: &NamespaceStorageAuthority,
        file: NamespaceManagedFile,
        kind: &str,
        retention_policy: &str,
    ) -> FileIndexResult<Self> {
        let kind = required_text("kind", kind)?.to_owned();
        let retention_policy = required_text("retention_policy", retention_policy)?.to_owned();
        authority
            .with_current_regular_files(
                &[NamespaceManagedFileCheck {
                    file: &file,
                    expected: None,
                }],
                |_| Ok(()),
            )
            .map_err(capability_error)?;
        Ok(Self {
            file,
            kind,
            retention_policy,
        })
    }
}
pub(super) struct NamespacedFileReferenceRegistration {
    pub(super) file: NamespacedManagedFileRegistration,
    pub(super) owner_type: String,
    pub(super) owner_id: String,
}
/// Metadata only; neither complete recovery references nor deletion authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OrphanCandidate {
    pub(super) key: ManagedFileKey,
    pub(super) physical_identity: StableFileIdentity,
    pub(super) byte_size: u64,
    pub(super) modified_at: SystemTime,
    pub(super) indexed_file_id: Option<ManagedFileId>,
}
fn capability_error(error: anyhow::Error) -> FileIndexError {
    match error.downcast::<FileIndexError>() {
        Ok(error) => error,
        Err(error) => FileIndexError::Capability(error),
    }
}

pub(super) fn initialize_file_index(path: impl AsRef<Path>) -> FileIndexResult<FileIndex> {
    FileIndex::initialize(path)
}
static GLOBAL_FILE_INDEX: OnceLock<FileIndex> = OnceLock::new();
pub(super) fn initialize_global_file_index(
    path: impl AsRef<Path>,
) -> FileIndexResult<&'static FileIndex> {
    if let Some(index) = GLOBAL_FILE_INDEX.get() {
        return Ok(index);
    }
    let index = FileIndex::initialize(path)?;
    let _ = GLOBAL_FILE_INDEX.set(index);
    GLOBAL_FILE_INDEX.get().ok_or(FileIndexError::LockPoisoned)
}
pub(super) fn global_file_index() -> Option<&'static FileIndex> {
    GLOBAL_FILE_INDEX.get()
}
impl FileIndex {
    pub(super) fn initialize(path: impl AsRef<Path>) -> FileIndexResult<Self> {
        let database_path = absolute_lexical_path(path.as_ref())?;
        if let Some(parent) = database_path.parent() {
            fs::create_dir_all(parent)?;
        }
        validate_database_file_boundary(&database_path)?;
        let mut connection = Connection::open(&database_path)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        validated_schema_version(&connection)?;
        let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(FileIndexError::Damaged("WAL mode unavailable".into()));
        }
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        migration_checkpoint(3)?;
        migrate_schema(&mut connection)?;
        Ok(Self {
            database_path: Arc::new(database_path),
            connection: Arc::new(Mutex::new(connection)),
        })
    }
    pub(super) fn database_path(&self) -> &Path {
        self.database_path.as_path()
    }
    fn lock_connection(&self) -> FileIndexResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| FileIndexError::LockPoisoned)
    }
}

impl FileIndex {
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn register_file(
        &self,
        registration: &ManagedFileRegistration,
    ) -> FileIndexResult<ManagedFileRecord> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn find_file_by_path(
        &self,
        path: &Path,
    ) -> FileIndexResult<Option<ManagedFileRecord>> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn find_file_by_id(
        &self,
        file_id: ManagedFileId,
    ) -> FileIndexResult<Option<ManagedFileRecord>> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn mark_pending_delete(
        &self,
        file_id: ManagedFileId,
        pending: bool,
    ) -> FileIndexResult<bool> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn delete_file(&self, file_id: ManagedFileId) -> FileIndexResult<bool> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn attach_reference(
        &self,
        file_id: ManagedFileId,
        owner_type: &str,
        owner_id: &str,
    ) -> FileIndexResult<bool> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn detach_reference(
        &self,
        file_id: ManagedFileId,
        owner_type: &str,
        owner_id: &str,
    ) -> FileIndexResult<bool> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn reference_count(&self, file_id: ManagedFileId) -> FileIndexResult<u64> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn clear_all_references(&self) -> FileIndexResult<()> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn replace_all_references(
        &self,
        registrations: &[FileReferenceRegistration],
    ) -> FileIndexResult<()> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn stats_by_kind(&self) -> FileIndexResult<Vec<FileKindStats>> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn upsert_preview(
        &self,
        registration: &PreviewRegistration,
    ) -> FileIndexResult<PreviewCacheRecord> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn find_preview(
        &self,
        key: &PreviewKey,
    ) -> FileIndexResult<Option<PreviewCacheRecord>> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn touch_preview(
        &self,
        key: &PreviewKey,
        accessed_at: i64,
    ) -> FileIndexResult<bool> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn delete_preview(
        &self,
        key: &PreviewKey,
    ) -> FileIndexResult<Option<PreviewCacheRecord>> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn delete_previews_for_source(
        &self,
        source_file_id: ManagedFileId,
    ) -> FileIndexResult<Vec<PreviewCacheRecord>> {
        Err(FileIndexError::NamespaceRequired)
    }
    // TEMP(team-accounts): remove in Task 10.
    #[allow(unused_variables)]
    pub(super) fn least_recently_used_previews(
        &self,
        limit: usize,
    ) -> FileIndexResult<Vec<PreviewCacheRecord>> {
        Err(FileIndexError::NamespaceRequired)
    }
}

const LEGACY_RENAMES: [&str; 3] = [
    "ALTER TABLE managed_files RENAME TO legacy_unassigned_managed_files",
    "ALTER TABLE file_references RENAME TO legacy_unassigned_file_references",
    "ALTER TABLE preview_cache RENAME TO legacy_unassigned_preview_cache",
];

// Schema manifests are evaluated by the actual SQLite library. We compare both
// normalized definitions (including every CHECK/trigger) and structural PRAGMAs.
fn schema_signature(connection: &Connection) -> FileIndexResult<Vec<String>> {
    let mut signature = Vec::new();
    let objects = connection.prepare("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE substr(name,1,7) != 'sqlite_' ORDER BY type,name")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (kind, name, table, sql) in objects {
        let normalized = sql
            .unwrap_or_default()
            .replace("IF NOT EXISTS ", "")
            .replace('"', "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        signature.push(format!("{kind}|{name}|{table}|{normalized}"));
        if kind == "table" {
            for pragma in ["table_info", "foreign_key_list", "index_list"] {
                let mut stmt = connection.prepare(&format!("PRAGMA {pragma}(\"{name}\")"))?;
                let columns = stmt.column_count();
                let mut rows = stmt.query([])?;
                let mut values = Vec::new();
                while let Some(row) = rows.next()? {
                    let mut fields = Vec::new();
                    for column in 0..columns {
                        fields.push(format!("{:?}", row.get_ref(column)?));
                    }
                    values.push(fields.join("|"));
                }
                values.sort();
                signature.push(format!("{name}:{pragma}:{values:?}"));
            }
            let indexes = connection
                .prepare(&format!("PRAGMA index_list(\"{name}\")"))?
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for index in indexes {
                let columns = connection
                    .prepare(&format!("PRAGMA index_info(\"{index}\")"))?
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, Option<String>>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                signature.push(format!("{index}:{columns:?}"));
            }
        }
    }
    signature.sort();
    Ok(signature)
}
fn validate_schema(connection: &Connection, version: i32, quarantine: bool) -> FileIndexResult<()> {
    let manifest = Connection::open_in_memory()?;
    manifest.pragma_update(None, "foreign_keys", "ON")?;
    if version == 1 || quarantine {
        manifest.execute_batch(SCHEMA_V1)?;
        if quarantine {
            for rename in LEGACY_RENAMES {
                manifest.execute_batch(rename)?;
            }
        }
    }
    if version == 2 {
        manifest.execute_batch(SCHEMA_V2)?;
    }
    if schema_signature(connection)? != schema_signature(&manifest)? {
        return Err(FileIndexError::Damaged(
            "schema differs from the complete version manifest".into(),
        ));
    }
    if connection
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return Err(FileIndexError::Damaged(
            "foreign-key consistency check failed".into(),
        ));
    }
    Ok(())
}
#[cfg(test)]
thread_local! { static MIGRATION_FAILURE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
thread_local! {
    // Scheduling seam at the real SQL-discovery/filesystem boundary. It cannot
    // replace validation, commit results or filesystem/SQLite implementations.
    static AFTER_DISCOVERY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
fn migration_checkpoint(_point: u8) -> FileIndexResult<()> {
    #[cfg(test)]
    if MIGRATION_FAILURE.with(|point| point.get() == _point) {
        return Err(FileIndexError::Damaged("injected migration failure".into()));
    }
    Ok(())
}

fn validated_schema_version(connection: &Connection) -> FileIndexResult<i32> {
    let version: i32 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > FILE_INDEX_SCHEMA_VERSION {
        return Err(FileIndexError::UnsupportedSchema {
            found: version,
            supported: FILE_INDEX_SCHEMA_VERSION,
        });
    }
    let integrity: String = connection.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(FileIndexError::Damaged(integrity));
    }
    match version {
        0 => validate_schema(connection, 0, false)?,
        1 => validate_schema(connection, 1, false)?,
        2 => {
            let quarantine: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name GLOB 'legacy_unassigned_*')",
                [],
                |r| r.get(0),
            )?;
            validate_schema(connection, 2, quarantine)?;
        }
        _ => {
            return Err(FileIndexError::Damaged(
                "unsupported negative schema version".into(),
            ))
        }
    }
    Ok(version)
}
fn migrate_schema(connection: &mut Connection) -> FileIndexResult<()> {
    // Dispatch and validate again inside the transaction, so schema discovery
    // and every migration statement share one SQLite snapshot and commit.
    let transaction = connection.transaction()?;
    let version = validated_schema_version(&transaction)?;
    if version == 2 {
        transaction.commit()?;
        return Ok(());
    }
    if version == 1 {
        for (position, rename) in LEGACY_RENAMES.iter().enumerate() {
            transaction.execute_batch(rename)?;
            if position == 1 {
                migration_checkpoint(1)?;
            }
        }
    }
    transaction.execute_batch(SCHEMA_V2)?;
    validate_schema(&transaction, 2, version == 1)?;
    migration_checkpoint(2)?;
    transaction.pragma_update(None, "user_version", 2)?;
    transaction.commit()?;
    Ok(())
}
fn absolute_lexical_path(path: &Path) -> FileIndexResult<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(FileIndexError::InvalidValue(
            "path must not be empty".to_string(),
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    Ok(normalized)
}

fn append_to_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value: OsString = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn now_millis() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn validate_database_file_boundary(database_path: &Path) -> FileIndexResult<()> {
    if let Some(parent) = database_path.parent() {
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(FileIndexError::InvalidValue(
                "file index parent must be a regular directory".to_string(),
            ));
        }
    }
    for candidate in [
        database_path.to_path_buf(),
        append_to_path(database_path, "-wal"),
        append_to_path(database_path, "-shm"),
    ] {
        match fs::symlink_metadata(&candidate) {
            Ok(metadata)
                if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(FileIndexError::InvalidValue(format!(
                    "file index path is not a regular file: {}",
                    candidate.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn validate_preview_key(key: &PreviewKey) -> FileIndexResult<()> {
    required_text("purpose", &key.purpose)?;
    if key.longest_edge == 0 {
        return Err(FileIndexError::InvalidValue(
            "longest_edge must be greater than zero".to_string(),
        ));
    }
    if key.cache_version == 0 {
        return Err(FileIndexError::InvalidValue(
            "cache_version must be greater than zero".to_string(),
        ));
    }
    Ok(())
}

fn required_text<'a>(name: &str, value: &'a str) -> FileIndexResult<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        Err(FileIndexError::InvalidValue(format!(
            "{name} must not be empty"
        )))
    } else {
        Ok(value)
    }
}

fn to_sql_i64(name: &str, value: u64) -> FileIndexResult<i64> {
    i64::try_from(value)
        .map_err(|_| FileIndexError::InvalidValue(format!("{name} exceeds SQLite INTEGER range")))
}

fn nonnegative_u64(name: &str, value: i64) -> FileIndexResult<u64> {
    u64::try_from(value).map_err(|_| {
        FileIndexError::Damaged(format!("{name} contains an unexpected negative value"))
    })
}

fn system_time_ns(time: SystemTime) -> FileIndexResult<i64> {
    let value = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok(),
        Err(error) => i128::try_from(error.duration().as_nanos())
            .ok()
            .and_then(i128::checked_neg),
    }
    .ok_or_else(|| FileIndexError::InvalidValue("mtime exceeds signed nanoseconds".into()))?;
    i64::try_from(value)
        .map_err(|_| FileIndexError::InvalidValue("mtime exceeds SQLite INTEGER".into()))
}

const FILE_SELECT: &str = "SELECT id,path,kind,byte_size,managed,retention_policy,created_at,last_accessed_at,pending_delete,user_public_id,managed_area,physical_identity FROM managed_files";
const PREVIEW_SELECT: &str = "SELECT pc.id,pc.source_file_id,pc.preview_file_id,source.path,preview.path,preview.byte_size,pc.purpose,pc.longest_edge,pc.source_size,pc.source_mtime_ns,pc.cache_version,pc.status,pc.last_accessed_at,pc.created_at,pc.updated_at,pc.user_public_id,source.managed_area,preview.managed_area,source.physical_identity,preview.physical_identity
FROM preview_cache pc
JOIN managed_files source ON source.id=pc.source_file_id AND source.user_public_id=pc.user_public_id
JOIN managed_files preview ON preview.id=pc.preview_file_id AND preview.user_public_id=pc.user_public_id";
fn field<T: rusqlite::types::FromSql>(row: &Row<'_>, column: usize) -> FileIndexResult<T> {
    row.get(column).map_err(|error| {
        FileIndexError::Damaged(format!("invalid stored column {column}: {error}"))
    })
}
fn decode_key(area: &str, path: &str) -> FileIndexResult<ManagedFileKey> {
    ManagedUserArea::from_storage_name(area)
        .and_then(|area| ManagedFileKey::new(area, path))
        .map_err(|error| FileIndexError::Damaged(error.to_string()))
}
fn decode_identity(bytes: &[u8]) -> FileIndexResult<StableFileIdentity> {
    StableFileIdentity::from_storage_bytes(bytes)
        .map_err(|error| FileIndexError::Damaged(error.to_string()))
}
fn decode_user(user: String) -> FileIndexResult<String> {
    if uuid::Uuid::parse_str(&user).is_ok_and(|id| id.to_string() == user) {
        Ok(user)
    } else {
        Err(FileIndexError::Damaged("noncanonical stored user".into()))
    }
}
fn map_file(row: &Row<'_>) -> FileIndexResult<ManagedFileRecord> {
    let key = decode_key(&field::<String>(row, 10)?, &field::<String>(row, 1)?)?;
    Ok(ManagedFileRecord {
        id: ManagedFileId(field(row, 0)?),
        path: PathBuf::from(key.relative_name().as_str()),
        kind: field(row, 2)?,
        byte_size: nonnegative_u64("byte_size", field(row, 3)?)?,
        managed: field(row, 4)?,
        retention_policy: field(row, 5)?,
        created_at: field(row, 6)?,
        last_accessed_at: field(row, 7)?,
        pending_delete: field(row, 8)?,
        user_public_id: decode_user(field(row, 9)?)?,
        managed_area: key.area(),
        physical_identity: decode_identity(&field::<Vec<u8>>(row, 11)?)?,
    })
}
fn map_preview(row: &Row<'_>) -> FileIndexResult<PreviewCacheRecord> {
    let source = decode_key(&field::<String>(row, 16)?, &field::<String>(row, 3)?)?;
    let preview = decode_key(&field::<String>(row, 17)?, &field::<String>(row, 4)?)?;
    let positive = |column| -> FileIndexResult<u32> {
        u32::try_from(field::<i64>(row, column)?)
            .ok()
            .filter(|v| *v > 0)
            .ok_or_else(|| FileIndexError::Damaged("invalid positive preview value".into()))
    };
    Ok(PreviewCacheRecord {
        id: field(row, 0)?,
        source_file_id: ManagedFileId(field(row, 1)?),
        preview_file_id: ManagedFileId(field(row, 2)?),
        source_path: PathBuf::from(source.relative_name().as_str()),
        preview_path: PathBuf::from(preview.relative_name().as_str()),
        preview_byte_size: nonnegative_u64("preview bytes", field(row, 5)?)?,
        purpose: field(row, 6)?,
        longest_edge: positive(7)?,
        source_size: nonnegative_u64("source bytes", field(row, 8)?)?,
        source_mtime_ns: field(row, 9)?,
        cache_version: positive(10)?,
        status: field(row, 11)?,
        last_accessed_at: field(row, 12)?,
        created_at: field(row, 13)?,
        updated_at: field(row, 14)?,
        user_public_id: decode_user(field(row, 15)?)?,
        source_area: source.area(),
        preview_area: preview.area(),
        source_identity: decode_identity(&field::<Vec<u8>>(row, 18)?)?,
        preview_identity: decode_identity(&field::<Vec<u8>>(row, 19)?)?,
    })
}
fn query_file(
    c: &Connection,
    user: &str,
    id: ManagedFileId,
) -> FileIndexResult<Option<ManagedFileRecord>> {
    let mut stmt = c.prepare(&format!("{FILE_SELECT} WHERE user_public_id=?1 AND id=?2"))?;
    let mut rows = stmt.query(params![user, id.0])?;
    rows.next()?.map(map_file).transpose()
}
fn query_key(
    c: &Connection,
    user: &str,
    key: &ManagedFileKey,
) -> FileIndexResult<Option<ManagedFileRecord>> {
    let mut stmt = c.prepare(&format!(
        "{FILE_SELECT} WHERE user_public_id=?1 AND managed_area=?2 AND path=?3"
    ))?;
    let mut rows = stmt.query(params![
        user,
        key.area().storage_name(),
        key.relative_name().as_str()
    ])?;
    rows.next()?.map(map_file).transpose()
}
fn query_identity(
    c: &Connection,
    user: &str,
    identity: StableFileIdentity,
) -> FileIndexResult<Option<ManagedFileRecord>> {
    let mut stmt = c.prepare(&format!(
        "{FILE_SELECT} WHERE user_public_id=?1 AND physical_identity=?2"
    ))?;
    let mut rows = stmt.query(params![user, identity.to_storage_bytes()])?;
    rows.next()?.map(map_file).transpose()
}
fn query_previews(
    c: &Connection,
    user: &str,
    source: Option<ManagedFileId>,
    limit: Option<i64>,
) -> FileIndexResult<Vec<PreviewCacheRecord>> {
    let sql=format!("{PREVIEW_SELECT} WHERE pc.user_public_id=?1 AND (?2 IS NULL OR pc.source_file_id=?2) ORDER BY {} LIMIT ?3",if limit.is_some() {"pc.last_accessed_at,pc.id"} else {"pc.id"});
    let mut stmt = c.prepare(&sql)?;
    let mut rows = stmt.query(params![user, source.map(|id| id.0), limit.unwrap_or(-1)])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        result.push(map_preview(row)?);
    }
    Ok(result)
}
fn query_preview(
    c: &Connection,
    user: &str,
    key: &PreviewKey,
) -> FileIndexResult<Option<PreviewCacheRecord>> {
    let mut stmt=c.prepare(&format!("{PREVIEW_SELECT} WHERE pc.user_public_id=?1 AND pc.source_file_id=?2 AND pc.purpose=?3 AND pc.longest_edge=?4 AND pc.cache_version=?5"))?;
    let mut rows = stmt.query(params![
        user,
        key.source_file_id.0,
        key.purpose.trim(),
        i64::from(key.longest_edge),
        i64::from(key.cache_version)
    ])?;
    rows.next()?.map(map_preview).transpose()
}
fn record_key(record: &ManagedFileRecord) -> FileIndexResult<ManagedFileKey> {
    decode_key(
        record.managed_area.storage_name(),
        record
            .path
            .to_str()
            .ok_or_else(|| FileIndexError::Damaged("non-UTF8 key".into()))?,
    )
}
fn identity_conflict() -> FileIndexError {
    FileIndexError::Capability(anyhow::anyhow!(
        "logical and physical file identity conflict"
    ))
}
fn register_guarded(
    c: &Connection,
    user: &str,
    registration: &NamespacedManagedFileRegistration,
    metadata: &ManagedFileMetadata,
    timestamp: i64,
) -> FileIndexResult<ManagedFileRecord> {
    let key = registration.file.key();
    let bytes = to_sql_i64("byte_size", metadata.byte_size)?;
    let _ = system_time_ns(metadata.modified_at)?;
    let existing = query_key(c, user, key)?;
    let identity = query_identity(c, user, metadata.identity)?;
    let identity_key = identity.as_ref().map(record_key).transpose()?;
    if existing
        .as_ref()
        .is_some_and(|r| r.physical_identity != metadata.identity)
        || identity_key
            .as_ref()
            .is_some_and(|record_key| record_key != key)
    {
        return Err(identity_conflict());
    }
    if existing.as_ref().map(|r| r.id) != identity.as_ref().map(|r| r.id) {
        return Err(identity_conflict());
    }
    if let Some(record) = existing {
        c.execute("UPDATE managed_files SET kind=?3,byte_size=?4,managed=1,retention_policy=?5,last_accessed_at=MAX(last_accessed_at,?6),pending_delete=0 WHERE user_public_id=?1 AND id=?2",
            params![user,record.id.0,registration.kind,bytes,registration.retention_policy,timestamp])?;
    } else {
        c.execute("INSERT INTO managed_files(user_public_id,managed_area,path,physical_identity,kind,byte_size,managed,retention_policy,created_at,last_accessed_at,pending_delete) VALUES(?1,?2,?3,?4,?5,?6,1,?7,?8,?8,0)",
            params![user,key.area().storage_name(),key.relative_name().as_str(),metadata.identity.to_storage_bytes(),registration.kind,bytes,registration.retention_policy,timestamp])?;
    }
    query_key(c, user, key)?
        .ok_or_else(|| FileIndexError::Damaged("registered row unavailable".into()))
}

impl FileIndex {
    // Private implementation helper. Every caller discovers owned SQL values and
    // drops SQLite before entry. Only this helper opens files and acquires the
    // single namespace guard, then SQLite, and compares complete row snapshots.
    fn with_records<T>(
        &self,
        authority: &NamespaceStorageAuthority,
        records: &[ManagedFileRecord],
        operation: impl FnOnce(&Transaction<'_>, &[ManagedFileMetadata]) -> FileIndexResult<T>,
    ) -> FileIndexResult<T> {
        #[cfg(test)]
        AFTER_DISCOVERY.with(|slot| {
            let hook = slot.borrow_mut().take();
            if let Some(hook) = hook {
                hook();
            }
        });
        let user = authority.user_public_id();
        let files = records
            .iter()
            .map(|record| {
                authority
                    .open_optional_regular(&record_key(record)?)
                    .map_err(capability_error)?
                    .ok_or(FileIndexError::MissingManagedFile)
            })
            .collect::<FileIndexResult<Vec<_>>>()?;
        let checks = files
            .iter()
            .zip(records)
            .map(|(file, record)| NamespaceManagedFileCheck {
                file,
                expected: Some(record.physical_identity),
            })
            .collect::<Vec<_>>();
        authority
            .with_current_regular_files(&checks, |metadata| {
                let mut c = self.lock_connection()?;
                let transaction = c.transaction()?;
                for record in records {
                    if query_file(&transaction, user, record.id)?.as_ref() != Some(record) {
                        return Err(FileIndexError::ConcurrentChange.into());
                    }
                }
                let result = operation(&transaction, metadata)?;
                transaction.commit()?;
                Ok(result)
            })
            .map_err(capability_error)
    }
    fn owned_file(
        &self,
        user: &str,
        id: ManagedFileId,
    ) -> FileIndexResult<Option<ManagedFileRecord>> {
        let c = self.lock_connection()?;
        query_file(&c, user, id)
    }
    fn file_operation<T>(
        &self,
        authority: &NamespaceStorageAuthority,
        id: ManagedFileId,
        absent: T,
        operation: impl FnOnce(&Transaction<'_>, &ManagedFileRecord) -> FileIndexResult<T>,
    ) -> FileIndexResult<T> {
        let Some(record) = self.owned_file(authority.user_public_id(), id)? else {
            return Ok(absent);
        };
        self.with_records(authority, std::slice::from_ref(&record), |tx, _| {
            operation(tx, &record)
        })
    }
    fn with_previews<T>(
        &self,
        authority: &NamespaceStorageAuthority,
        records: &[PreviewCacheRecord],
        requery: impl Fn(&Connection) -> FileIndexResult<Vec<PreviewCacheRecord>>,
        operation: impl FnOnce(&Transaction<'_>) -> FileIndexResult<T>,
    ) -> FileIndexResult<T> {
        let user = authority.user_public_id();
        let files = {
            let c = self.lock_connection()?;
            let mut files = Vec::new();
            for preview in records {
                for id in [preview.source_file_id, preview.preview_file_id] {
                    files.push(query_file(&c, user, id)?.ok_or(FileIndexError::ConcurrentChange)?);
                }
            }
            files
        };
        self.with_records(authority, &files, |tx, _| {
            if requery(tx)? != records {
                return Err(FileIndexError::ConcurrentChange);
            }
            operation(tx)
        })
    }
}

impl FileIndex {
    pub(super) fn register_file_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        registration: &NamespacedManagedFileRegistration,
    ) -> FileIndexResult<ManagedFileRecord> {
        let user = authority.user_public_id();
        let timestamp = now_millis();
        authority
            .with_current_regular_files(
                &[NamespaceManagedFileCheck {
                    file: &registration.file,
                    expected: None,
                }],
                |metadata| {
                    let mut c = self.lock_connection()?;
                    let tx = c.transaction()?;
                    let record =
                        register_guarded(&tx, user, registration, &metadata[0], timestamp)?;
                    tx.commit()?;
                    Ok(record)
                },
            )
            .map_err(capability_error)
    }
    pub(super) fn find_file_by_path_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        area: ManagedUserArea,
        relative_name: &str,
    ) -> FileIndexResult<Option<ManagedFileRecord>> {
        let key = ManagedFileKey::new(area, relative_name)
            .map_err(|e| FileIndexError::InvalidValue(e.to_string()))?;
        let record = {
            let c = self.lock_connection()?;
            query_key(&c, authority.user_public_id(), &key)?
        };
        let Some(record) = record else {
            return Ok(None);
        };
        self.with_records(authority, std::slice::from_ref(&record), |_, _| {
            Ok(Some(record.clone()))
        })
    }
    pub(super) fn find_file_by_id_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        file_id: ManagedFileId,
    ) -> FileIndexResult<Option<ManagedFileRecord>> {
        self.file_operation(authority, file_id, None, |_, record| {
            Ok(Some(record.clone()))
        })
    }
    pub(super) fn mark_pending_delete_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        file_id: ManagedFileId,
        pending: bool,
    ) -> FileIndexResult<bool> {
        let user = authority.user_public_id();
        self.file_operation(authority, file_id, false, |tx, _| {
            Ok(tx.execute(
                "UPDATE managed_files SET pending_delete=?3 WHERE user_public_id=?1 AND id=?2",
                params![user, file_id.0, pending],
            )? > 0)
        })
    }
    // Metadata-only removal. A currently guarded regular file is mandatory;
    // this method never unlinks and cannot repair metadata after an unlink.
    pub(super) fn delete_file_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        file_id: ManagedFileId,
    ) -> FileIndexResult<bool> {
        let user = authority.user_public_id();
        self.file_operation(authority, file_id, false, |tx, _| {
            Ok(tx.execute(
                "DELETE FROM managed_files WHERE user_public_id=?1 AND id=?2",
                params![user, file_id.0],
            )? > 0)
        })
    }
    pub(super) fn attach_reference_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        file_id: ManagedFileId,
        owner_type: &str,
        owner_id: &str,
    ) -> FileIndexResult<bool> {
        let owner_type = required_text("owner_type", owner_type)?;
        let owner_id = required_text("owner_id", owner_id)?;
        let user = authority.user_public_id();
        let timestamp = now_millis();
        let record = self
            .owned_file(user, file_id)?
            .ok_or_else(|| FileIndexError::InvalidValue("reference target is not owned".into()))?;
        self.with_records(authority,&[record],|tx,_|Ok(tx.execute("INSERT INTO file_references(user_public_id,file_id,owner_type,owner_id,created_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(user_public_id,file_id,owner_type,owner_id) DO NOTHING",params![user,file_id.0,owner_type,owner_id,timestamp])?>0))
    }
    pub(super) fn detach_reference_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        file_id: ManagedFileId,
        owner_type: &str,
        owner_id: &str,
    ) -> FileIndexResult<bool> {
        let owner_type = required_text("owner_type", owner_type)?;
        let owner_id = required_text("owner_id", owner_id)?;
        let user = authority.user_public_id();
        self.file_operation(authority,file_id,false,|tx,_|Ok(tx.execute("DELETE FROM file_references WHERE user_public_id=?1 AND file_id=?2 AND owner_type=?3 AND owner_id=?4",params![user,file_id.0,owner_type,owner_id])?>0))
    }
    pub(super) fn reference_count_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        file_id: ManagedFileId,
    ) -> FileIndexResult<u64> {
        let user = authority.user_public_id();
        self.file_operation(authority, file_id, 0, |tx, _| {
            reference_count_owned(tx, user, file_id)
        })
    }
    pub(super) fn clear_all_references_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
    ) -> FileIndexResult<()> {
        let mut c = self.lock_connection()?;
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM file_references WHERE user_public_id=?1",
            params![authority.user_public_id()],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn replace_all_references_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        registrations: &[NamespacedFileReferenceRegistration],
    ) -> FileIndexResult<()> {
        if registrations.is_empty() {
            return self.clear_all_references_for_namespace(authority);
        }
        let user = authority.user_public_id();
        let timestamp = now_millis();
        let owners = registrations
            .iter()
            .map(|r| {
                Ok((
                    required_text("owner_type", &r.owner_type)?,
                    required_text("owner_id", &r.owner_id)?,
                ))
            })
            .collect::<FileIndexResult<Vec<_>>>()?;
        let checks = registrations
            .iter()
            .map(|r| NamespaceManagedFileCheck {
                file: &r.file.file,
                expected: None,
            })
            .collect::<Vec<_>>();
        authority.with_current_regular_files(&checks,|metadata| {
            let mut c=self.lock_connection()?; let tx=c.transaction()?;
            tx.execute("DELETE FROM file_references WHERE user_public_id=?1",params![user])?;
            for ((registration,metadata),(owner_type,owner_id)) in registrations.iter().zip(metadata).zip(owners) {
                let record=register_guarded(&tx,user,&registration.file,metadata,timestamp)?;
                tx.execute("INSERT INTO file_references(user_public_id,file_id,owner_type,owner_id,created_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(user_public_id,file_id,owner_type,owner_id) DO NOTHING",params![user,record.id.0,owner_type,owner_id,timestamp])?;
            }
            tx.commit()?; Ok(())
        }).map_err(capability_error)
    }
    pub(super) fn stats_by_kind_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
    ) -> FileIndexResult<Vec<FileKindStats>> {
        let c = self.lock_connection()?;
        let mut stmt=c.prepare("SELECT kind,COUNT(*),COALESCE(SUM(byte_size),0),COALESCE(SUM(managed),0),COALESCE(SUM(pending_delete),0) FROM managed_files WHERE user_public_id=?1 GROUP BY kind ORDER BY kind")?;
        let mut rows = stmt.query(params![authority.user_public_id()])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(FileKindStats {
                kind: field(row, 0)?,
                file_count: nonnegative_u64("count", field(row, 1)?)?,
                byte_size: nonnegative_u64("bytes", field(row, 2)?)?,
                managed_count: nonnegative_u64("managed", field(row, 3)?)?,
                pending_delete_count: nonnegative_u64("pending", field(row, 4)?)?,
            });
        }
        Ok(result)
    }
}
fn reference_count_owned(c: &Connection, user: &str, id: ManagedFileId) -> FileIndexResult<u64> {
    nonnegative_u64(
        "reference count",
        c.query_row(
            "SELECT COUNT(*) FROM file_references WHERE user_public_id=?1 AND file_id=?2",
            params![user, id.0],
            |r| r.get(0),
        )?,
    )
}

impl FileIndex {
    pub(super) fn upsert_preview_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        registration: &PreviewRegistration,
    ) -> FileIndexResult<PreviewCacheRecord> {
        validate_preview_key(&registration.key)?;
        if registration.key.source_file_id == registration.preview_file_id {
            return Err(FileIndexError::InvalidValue(
                "source and preview must differ".into(),
            ));
        }
        let status = required_text("status", &registration.status)?;
        let bytes = to_sql_i64("source_size", registration.source_size)?;
        let user = authority.user_public_id();
        let key = &registration.key;
        let timestamp = now_millis();
        let (files, previous) = {
            let c = self.lock_connection()?;
            let mut files = Vec::new();
            for id in [key.source_file_id, registration.preview_file_id] {
                files.push(query_file(&c, user, id)?.ok_or_else(|| {
                    FileIndexError::InvalidValue("preview target is not owned".into())
                })?);
            }
            let previous = query_preview(&c, user, key)?;
            if let Some(previous) = &previous {
                files.push(
                    query_file(&c, user, previous.preview_file_id)?
                        .ok_or(FileIndexError::ConcurrentChange)?,
                );
            }
            (files, previous)
        };
        self.with_records(authority,&files,|tx,metadata| {
            if query_preview(tx,user,key)?!=previous { return Err(FileIndexError::ConcurrentChange); }
            if metadata[0].byte_size!=registration.source_size || system_time_ns(metadata[0].modified_at)?!=registration.source_mtime_ns { return Err(FileIndexError::InvalidValue("preview source metadata changed".into())); }
            tx.execute("INSERT INTO preview_cache(user_public_id,source_file_id,preview_file_id,purpose,longest_edge,source_size,source_mtime_ns,cache_version,status,last_accessed_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11) ON CONFLICT(user_public_id,source_file_id,purpose,longest_edge,cache_version) DO UPDATE SET preview_file_id=excluded.preview_file_id,source_size=excluded.source_size,source_mtime_ns=excluded.source_mtime_ns,status=excluded.status,last_accessed_at=excluded.last_accessed_at,updated_at=excluded.updated_at",
                params![user,key.source_file_id.0,registration.preview_file_id.0,key.purpose.trim(),i64::from(key.longest_edge),bytes,registration.source_mtime_ns,i64::from(key.cache_version),status,registration.last_accessed_at,timestamp])?;
            tx.execute("UPDATE managed_files SET last_accessed_at=MAX(last_accessed_at,?3) WHERE user_public_id=?1 AND id=?2",params![user,registration.preview_file_id.0,registration.last_accessed_at])?;
            query_preview(tx,user,key)?.ok_or_else(||FileIndexError::Damaged("upserted preview missing".into()))
        })
    }
    pub(super) fn find_preview_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        key: &PreviewKey,
    ) -> FileIndexResult<Option<PreviewCacheRecord>> {
        validate_preview_key(key)?;
        let user = authority.user_public_id();
        let record = {
            let c = self.lock_connection()?;
            query_preview(&c, user, key)?
        };
        let Some(record) = record else {
            return Ok(None);
        };
        self.with_previews(
            authority,
            std::slice::from_ref(&record),
            |c| Ok(query_preview(c, user, key)?.into_iter().collect()),
            |_| Ok(Some(record.clone())),
        )
    }
    pub(super) fn touch_preview_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        key: &PreviewKey,
        accessed_at: i64,
    ) -> FileIndexResult<bool> {
        validate_preview_key(key)?;
        let user = authority.user_public_id();
        let record = {
            let c = self.lock_connection()?;
            query_preview(&c, user, key)?
        };
        let Some(record) = record else {
            return Ok(false);
        };
        self.with_previews(authority,std::slice::from_ref(&record),|c|Ok(query_preview(c,user,key)?.into_iter().collect()),|tx| {
            tx.execute("UPDATE preview_cache SET last_accessed_at=MAX(last_accessed_at,?3) WHERE user_public_id=?1 AND id=?2",params![user,record.id,accessed_at])?;
            tx.execute("UPDATE managed_files SET last_accessed_at=MAX(last_accessed_at,?3) WHERE user_public_id=?1 AND id=?2",params![user,record.preview_file_id.0,accessed_at])?;
            Ok(true)
        })
    }
    pub(super) fn delete_preview_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        key: &PreviewKey,
    ) -> FileIndexResult<Option<PreviewCacheRecord>> {
        validate_preview_key(key)?;
        let user = authority.user_public_id();
        let record = {
            let c = self.lock_connection()?;
            query_preview(&c, user, key)?
        };
        let Some(record) = record else {
            return Ok(None);
        };
        self.with_previews(
            authority,
            std::slice::from_ref(&record),
            |c| Ok(query_preview(c, user, key)?.into_iter().collect()),
            |tx| {
                tx.execute(
                    "DELETE FROM preview_cache WHERE user_public_id=?1 AND id=?2",
                    params![user, record.id],
                )?;
                Ok(Some(record.clone()))
            },
        )
    }
    pub(super) fn delete_previews_for_source_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        source_file_id: ManagedFileId,
    ) -> FileIndexResult<Vec<PreviewCacheRecord>> {
        let user = authority.user_public_id();
        let records = {
            let c = self.lock_connection()?;
            query_previews(&c, user, Some(source_file_id), None)?
        };
        if records.is_empty() {
            return Ok(records);
        }
        self.with_previews(
            authority,
            &records,
            |c| query_previews(c, user, Some(source_file_id), None),
            |tx| {
                tx.execute(
                    "DELETE FROM preview_cache WHERE user_public_id=?1 AND source_file_id=?2",
                    params![user, source_file_id.0],
                )?;
                Ok(records.clone())
            },
        )
    }
    pub(super) fn least_recently_used_previews_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        limit: usize,
    ) -> FileIndexResult<Vec<PreviewCacheRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit)
            .map_err(|_| FileIndexError::InvalidValue("LRU limit exceeds INTEGER".into()))?;
        let user = authority.user_public_id();
        let records = {
            let c = self.lock_connection()?;
            query_previews(&c, user, None, Some(limit))?
        };
        if records.is_empty() {
            return Ok(records);
        }
        self.with_previews(
            authority,
            &records,
            |c| query_previews(c, user, None, Some(limit)),
            |_| Ok(records.clone()),
        )
    }
    pub(super) fn orphan_candidate_for_namespace(
        &self,
        authority: &NamespaceStorageAuthority,
        file: &NamespaceManagedFile,
        cutoff: SystemTime,
    ) -> FileIndexResult<Option<OrphanCandidate>> {
        let key = file.key();
        let user = authority.user_public_id();
        if !matches!(
            key.area(),
            ManagedUserArea::CanvasUploads | ManagedUserArea::ReferencesLibrary
        ) {
            return Err(FileIndexError::InvalidValue(
                "area is not an orphan candidate domain".into(),
            ));
        }
        authority
            .with_current_regular_files(
                &[NamespaceManagedFileCheck {
                    file,
                    expected: None,
                }],
                |metadata| {
                    let metadata = &metadata[0];
                    let mut c = self.lock_connection()?;
                    let tx = c.transaction()?;
                    let row = query_key(&tx, user, key)?;
                    let identity = query_identity(&tx, user, metadata.identity)?;
                    let identity_key = identity.as_ref().map(record_key).transpose()?;
                    if row
                        .as_ref()
                        .is_some_and(|r| r.physical_identity != metadata.identity)
                        || identity_key
                            .as_ref()
                            .is_some_and(|record_key| record_key != key)
                        || row.as_ref().map(|r| r.id) != identity.as_ref().map(|r| r.id)
                    {
                        return Err(identity_conflict().into());
                    }
                    let _ = to_sql_i64("byte_size", metadata.byte_size)?;
                    let _ = system_time_ns(metadata.modified_at)?;
                    let referenced = row
                        .as_ref()
                        .map(|r| reference_count_owned(&tx, user, r.id))
                        .transpose()?
                        .is_some_and(|count| count > 0);
                    let candidate = if metadata.modified_at > cutoff || referenced {
                        None
                    } else {
                        Some(OrphanCandidate {
                            key: key.clone(),
                            physical_identity: metadata.identity,
                            byte_size: metadata.byte_size,
                            modified_at: metadata.modified_at,
                            indexed_file_id: row.map(|r| r.id),
                        })
                    };
                    tx.commit()?;
                    Ok(candidate)
                },
            )
            .map_err(capability_error)
    }
}

#[cfg(test)]
mod tests {
    use super::super::user_namespace::{NamespaceFs, NamespaceLease, UserNamespace};
    use super::*;
    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";
    struct Fixture {
        index: FileIndex,
        a: NamespaceStorageAuthority,
        b: NamespaceStorageAuthority,
        directory: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().canonicalize().unwrap();
            let root = Arc::new(NamespaceFs::open_data_root(&path).unwrap());
            let authority = |user| {
                NamespaceStorageAuthority::open(
                    Arc::clone(&root),
                    &NamespaceLease {
                        namespace: UserNamespace::new(&path, user).unwrap(),
                        auth_epoch: 3,
                        namespace_epoch: 7,
                    },
                )
                .unwrap()
            };
            Self {
                index: FileIndex::initialize(path.join("index.sqlite3")).unwrap(),
                a: authority(A),
                b: authority(B),
                directory,
            }
        }
        fn registration(
            &self,
            authority: &NamespaceStorageAuthority,
            area: ManagedUserArea,
            name: &str,
            bytes: &[u8],
        ) -> NamespacedManagedFileRegistration {
            let key = ManagedFileKey::new(area, name).unwrap();
            let file = authority.create_new_regular(&key).unwrap();
            fs::write(authority.lease().namespace.path(area).join(name), bytes).unwrap();
            NamespacedManagedFileRegistration::new(authority, file, "image", "user").unwrap()
        }
        fn register(
            &self,
            authority: &NamespaceStorageAuthority,
            area: ManagedUserArea,
            name: &str,
        ) -> ManagedFileRecord {
            self.index
                .register_file_for_namespace(
                    authority,
                    &self.registration(authority, area, name, b"image"),
                )
                .unwrap()
        }
        fn preview(
            &self,
            authority: &NamespaceStorageAuthority,
            suffix: &str,
        ) -> (ManagedFileRecord, ManagedFileRecord, PreviewRegistration) {
            let source = self.register(
                authority,
                ManagedUserArea::Output,
                &format!("source-{suffix}.png"),
            );
            let preview = self.register(
                authority,
                ManagedUserArea::Previews,
                &format!("preview-{suffix}.png"),
            );
            let metadata = authority
                .inspect_regular(
                    &authority
                        .open_existing_regular(
                            &ManagedFileKey::new(
                                source.managed_area,
                                source.path.to_str().unwrap(),
                            )
                            .unwrap(),
                        )
                        .unwrap(),
                )
                .unwrap();
            let registration = PreviewRegistration {
                key: PreviewKey {
                    source_file_id: source.id,
                    purpose: "gallery".into(),
                    longest_edge: 64,
                    cache_version: 1,
                },
                preview_file_id: preview.id,
                source_size: 5,
                source_mtime_ns: system_time_ns(metadata.modified_at).unwrap(),
                status: "ready".into(),
                last_accessed_at: 7,
            };
            (source, preview, registration)
        }
    }
    fn seed_v1(path: &Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(SCHEMA_V1).unwrap();
        c.execute_batch("INSERT INTO managed_files VALUES(1,'/legacy/a','output',7,1,'user',1,1,0); INSERT INTO managed_files VALUES(2,'/legacy/b','preview',3,1,'cache',1,1,0); INSERT INTO file_references VALUES(1,'asset','saved',1); INSERT INTO preview_cache VALUES(1,1,2,'gallery',64,7,1,1,'ready',1,1,1); PRAGMA user_version=1;").unwrap();
    }
    fn count(c: &Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    #[test]
    fn v1_file_index_rows_are_quarantined_without_assignment() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index.sqlite3");
        seed_v1(&path);
        let index = FileIndex::initialize(&path).unwrap();
        {
            let c = index.lock_connection().unwrap();
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                2
            );
            assert_eq!(count(&c, "legacy_unassigned_managed_files"), 2);
            assert_eq!(count(&c, "legacy_unassigned_file_references"), 1);
            assert_eq!(count(&c, "legacy_unassigned_preview_cache"), 1);
            for table in ["managed_files", "file_references", "preview_cache"] {
                assert_eq!(count(&c, table), 0);
            }
            assert_eq!(
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND sql IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                12
            );
            assert_eq!(
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                4
            );
            assert!(!c
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap());
            let target:String=c.query_row("SELECT \"table\" FROM pragma_foreign_key_list('legacy_unassigned_preview_cache') LIMIT 1",[],|r|r.get(0)).unwrap();
            assert_eq!(target, "legacy_unassigned_managed_files");
        }
        drop(index);
        FileIndex::initialize(&path).unwrap();
    }
    #[test]
    fn migration_rolls_back_second_rename_and_preversion_then_retries() {
        for point in [1, 2] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("index.sqlite3");
            seed_v1(&path);
            let before = schema_signature(&Connection::open(&path).unwrap()).unwrap();
            MIGRATION_FAILURE.with(|p| p.set(point));
            assert!(FileIndex::initialize(&path).is_err());
            MIGRATION_FAILURE.with(|p| p.set(0));
            let c = Connection::open(&path).unwrap();
            assert_eq!(schema_signature(&c).unwrap(), before);
            assert_eq!(count(&c, "managed_files"), 2);
            assert_eq!(count(&c, "file_references"), 1);
            assert_eq!(count(&c, "preview_cache"), 1);
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            drop(c);
            FileIndex::initialize(&path).unwrap();
        }
    }
    #[test]
    fn unsupported_partial_unknown_and_malformed_databases_are_preserved() {
        for sql in [
            "PRAGMA user_version=3",
            "CREATE TABLE managed_files(id INTEGER)",
            "CREATE TABLE unrelated(value TEXT)",
            "CREATE TABLE sqliteX_hidden(value TEXT)",
            "CREATE TABLE legacy_unassigned_managed_files(id INTEGER)",
            "CREATE TABLE x(id INTEGER); PRAGMA user_version=2",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("index.sqlite3");
            let c = Connection::open(&path).unwrap();
            c.execute_batch(sql).unwrap();
            let signature = schema_signature(&c).unwrap();
            let version: i64 = c
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            drop(c);
            assert!(FileIndex::initialize(&path).is_err());
            let c = Connection::open(&path).unwrap();
            assert_eq!(schema_signature(&c).unwrap(), signature);
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                version
            );
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index.sqlite3");
        fs::write(&path, b"not sqlite: preserve this").unwrap();
        assert!(FileIndex::initialize(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not sqlite: preserve this");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    #[test]
    fn namespace_files_use_distinct_physical_paths_and_reject_cross_registration() {
        let f = Fixture::new();
        let reg = f.registration(&f.a, ManagedUserArea::Output, "same.png", b"alpha");
        assert!(f.index.register_file_for_namespace(&f.b, &reg).is_err());
        let a = f.index.register_file_for_namespace(&f.a, &reg).unwrap();
        let b = f.register(&f.b, ManagedUserArea::Output, "same.png");
        let area = f.register(&f.a, ManagedUserArea::Previews, "same.png");
        assert_ne!(a.physical_identity, b.physical_identity);
        assert_ne!(a.physical_identity, area.physical_identity);
        assert_ne!(a.id, b.id);
        assert_eq!(a.path, PathBuf::from("same.png"));
        assert_eq!(a.user_public_id, A);
        assert!(f
            .index
            .find_file_by_id_for_namespace(&f.b, a.id)
            .unwrap()
            .is_none());
        assert_eq!(
            f.index
                .find_file_by_path_for_namespace(&f.a, ManagedUserArea::Output, "same.png")
                .unwrap(),
            Some(a.clone())
        );
        assert_eq!(
            f.index.register_file_for_namespace(&f.a, &reg).unwrap().id,
            a.id
        );
    }
    #[test]
    fn scoped_file_crud_and_references_preserve_other_user_and_bytes() {
        let f = Fixture::new();
        let a = f.register(&f.a, ManagedUserArea::Output, "a");
        let b = f.register(&f.b, ManagedUserArea::Output, "b");
        assert!(f
            .index
            .attach_reference_for_namespace(&f.a, a.id, "asset", "x")
            .unwrap());
        assert!(!f
            .index
            .attach_reference_for_namespace(&f.a, a.id, "asset", "x")
            .unwrap());
        assert!(matches!(
            f.index
                .attach_reference_for_namespace(&f.b, a.id, "asset", "x"),
            Err(FileIndexError::InvalidValue(_))
        ));
        assert_eq!(
            f.index.reference_count_for_namespace(&f.a, a.id).unwrap(),
            1
        );
        assert_eq!(
            f.index.reference_count_for_namespace(&f.b, a.id).unwrap(),
            0
        );
        assert!(f
            .index
            .detach_reference_for_namespace(&f.a, a.id, "asset", "x")
            .unwrap());
        assert!(!f
            .index
            .detach_reference_for_namespace(&f.a, a.id, "asset", "x")
            .unwrap());
        assert!(f
            .index
            .mark_pending_delete_for_namespace(&f.a, a.id, true)
            .unwrap());
        assert!(!f
            .index
            .mark_pending_delete_for_namespace(&f.b, a.id, true)
            .unwrap());
        assert_eq!(
            f.index.stats_by_kind_for_namespace(&f.a).unwrap(),
            vec![FileKindStats {
                kind: "image".into(),
                file_count: 1,
                byte_size: 5,
                managed_count: 1,
                pending_delete_count: 1
            }]
        );
        assert!(f.index.delete_file_for_namespace(&f.a, a.id).unwrap());
        assert!(!f.index.delete_file_for_namespace(&f.a, a.id).unwrap());
        assert_eq!(
            fs::read(f.a.lease().namespace.output_dir().join("a")).unwrap(),
            b"image"
        );
        assert!(f
            .index
            .find_file_by_id_for_namespace(&f.b, b.id)
            .unwrap()
            .is_some());
    }
    #[test]
    fn replacing_user_a_references_keeps_user_b_references() {
        let f = Fixture::new();
        let b = f.register(&f.b, ManagedUserArea::Output, "b");
        f.index
            .attach_reference_for_namespace(&f.b, b.id, "asset", "same")
            .unwrap();
        let registrations = vec![NamespacedFileReferenceRegistration {
            file: f.registration(&f.a, ManagedUserArea::Output, "a", b"a"),
            owner_type: "asset".into(),
            owner_id: "same".into(),
        }];
        f.index
            .replace_all_references_for_namespace(&f.a, &registrations)
            .unwrap();
        let a = f
            .index
            .find_file_by_path_for_namespace(&f.a, ManagedUserArea::Output, "a")
            .unwrap()
            .unwrap();
        assert_eq!(
            f.index.reference_count_for_namespace(&f.a, a.id).unwrap(),
            1
        );
        f.index
            .replace_all_references_for_namespace(&f.a, &[])
            .unwrap();
        assert_eq!(
            f.index.reference_count_for_namespace(&f.a, a.id).unwrap(),
            0
        );
        assert_eq!(
            f.index.reference_count_for_namespace(&f.b, b.id).unwrap(),
            1
        );
        f.index.clear_all_references_for_namespace(&f.a).unwrap();
        assert_eq!(
            f.index.reference_count_for_namespace(&f.b, b.id).unwrap(),
            1
        );
    }
    #[test]
    fn preview_crud_lru_and_source_deletion_are_scoped_and_metadata_only() {
        let f = Fixture::new();
        let (a, preview, reg) = f.preview(&f.a, "a");
        let (_, _, breg) = f.preview(&f.b, "b");
        let record = f.index.upsert_preview_for_namespace(&f.a, &reg).unwrap();
        f.index.upsert_preview_for_namespace(&f.b, &breg).unwrap();
        assert_eq!(record.source_path, PathBuf::from("source-a.png"));
        assert_eq!(record.source_identity, a.physical_identity);
        assert_eq!(record.preview_identity, preview.physical_identity);
        assert_eq!(
            f.index.find_preview_for_namespace(&f.a, &reg.key).unwrap(),
            Some(record)
        );
        assert!(f
            .index
            .find_preview_for_namespace(&f.b, &reg.key)
            .unwrap()
            .is_none());
        assert!(f
            .index
            .touch_preview_for_namespace(&f.a, &reg.key, 100)
            .unwrap());
        assert!(!f
            .index
            .touch_preview_for_namespace(&f.b, &reg.key, 100)
            .unwrap());
        let rows = f
            .index
            .least_recently_used_previews_for_namespace(&f.a, 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_accessed_at, 100);
        assert!(f
            .index
            .least_recently_used_previews_for_namespace(&f.a, 0)
            .unwrap()
            .is_empty());
        assert!(f
            .index
            .delete_preview_for_namespace(&f.a, &reg.key)
            .unwrap()
            .is_some());
        assert!(f
            .index
            .delete_preview_for_namespace(&f.a, &reg.key)
            .unwrap()
            .is_none());
        f.index.upsert_preview_for_namespace(&f.a, &reg).unwrap();
        assert_eq!(
            f.index
                .delete_previews_for_source_for_namespace(&f.a, a.id)
                .unwrap()
                .len(),
            1
        );
        assert!(f
            .index
            .delete_previews_for_source_for_namespace(&f.a, a.id)
            .unwrap()
            .is_empty());
        assert!(f
            .index
            .find_preview_for_namespace(&f.b, &breg.key)
            .unwrap()
            .is_some());
        assert!(f
            .a
            .lease()
            .namespace
            .preview_dir()
            .join("preview-a.png")
            .is_file());
    }

    #[test]
    fn missing_files_are_typed_errors_and_preserve_rows_and_links() {
        let f = Fixture::new();
        let (source, preview, reg) = f.preview(&f.a, "missing");
        f.index
            .attach_reference_for_namespace(&f.a, source.id, "asset", "keep")
            .unwrap();
        f.index.upsert_preview_for_namespace(&f.a, &reg).unwrap();
        let missing = ManagedFileId(999999);
        assert!(f
            .index
            .find_file_by_id_for_namespace(&f.a, missing)
            .unwrap()
            .is_none());
        assert_eq!(
            f.index
                .reference_count_for_namespace(&f.a, missing)
                .unwrap(),
            0
        );
        fs::remove_file(
            f.a.lease()
                .namespace
                .output_dir()
                .join("source-missing.png"),
        )
        .unwrap();
        macro_rules! missing {
            ($result:expr) => {
                assert!(matches!($result, Err(FileIndexError::MissingManagedFile)));
            };
        }
        missing!(f.index.find_file_by_id_for_namespace(&f.a, source.id));
        missing!(f.index.find_file_by_path_for_namespace(
            &f.a,
            ManagedUserArea::Output,
            "source-missing.png"
        ));
        missing!(f
            .index
            .mark_pending_delete_for_namespace(&f.a, source.id, true));
        missing!(f.index.delete_file_for_namespace(&f.a, source.id));
        missing!(f
            .index
            .attach_reference_for_namespace(&f.a, source.id, "asset", "other"));
        missing!(f
            .index
            .detach_reference_for_namespace(&f.a, source.id, "asset", "keep"));
        missing!(f.index.reference_count_for_namespace(&f.a, source.id));
        missing!(f.index.find_preview_for_namespace(&f.a, &reg.key));
        missing!(f.index.touch_preview_for_namespace(&f.a, &reg.key, 9));
        missing!(f.index.delete_preview_for_namespace(&f.a, &reg.key));
        missing!(f
            .index
            .delete_previews_for_source_for_namespace(&f.a, source.id));
        missing!(f.index.least_recently_used_previews_for_namespace(&f.a, 10));
        missing!(f.index.upsert_preview_for_namespace(&f.a, &reg));
        let c = f.index.lock_connection().unwrap();
        assert_eq!(count(&c, "managed_files"), 2);
        assert_eq!(count(&c, "file_references"), 1);
        assert_eq!(count(&c, "preview_cache"), 1);
        assert!(query_file(&c, A, preview.id).unwrap().is_some());
    }
    #[test]
    fn stale_registration_parent_swap_and_alias_conflicts_fail_closed() {
        let f = Fixture::new();
        let registration = f.registration(&f.a, ManagedUserArea::Output, "a", b"old");
        let path = f.a.lease().namespace.output_dir().join("a");
        fs::rename(&path, path.with_file_name("retained")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        assert!(matches!(
            f.index.register_file_for_namespace(&f.a, &registration),
            Err(FileIndexError::Capability(_))
        ));
        assert_eq!(
            count(&f.index.lock_connection().unwrap(), "managed_files"),
            0
        );
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        let fresh =
            f.a.open_existing_regular(&ManagedFileKey::new(ManagedUserArea::Output, "a").unwrap())
                .unwrap();
        let registration =
            NamespacedManagedFileRegistration::new(&f.a, fresh, "image", "user").unwrap();
        let record = f
            .index
            .register_file_for_namespace(&f.a, &registration)
            .unwrap();
        fs::rename(&path, path.with_file_name("retained-2")).unwrap();
        fs::write(&path, b"another").unwrap();
        let fresh =
            f.a.open_existing_regular(&ManagedFileKey::new(ManagedUserArea::Output, "a").unwrap())
                .unwrap();
        let registration =
            NamespacedManagedFileRegistration::new(&f.a, fresh, "image", "user").unwrap();
        assert!(matches!(
            f.index.register_file_for_namespace(&f.a, &registration),
            Err(FileIndexError::Capability(_))
        ));
        assert!(matches!(
            f.index.find_file_by_id_for_namespace(&f.a, record.id),
            Err(FileIndexError::Capability(_))
        ));
        assert_eq!(
            count(&f.index.lock_connection().unwrap(), "managed_files"),
            1
        );
        assert!(ManagedFileKey::new(ManagedUserArea::Canvas, "uploads/x").is_err());
        assert!(ManagedFileKey::new(ManagedUserArea::Output, "a//b").is_err());
    }
    #[test]
    fn file_index_parent_swap_uses_opened_capability_or_fails_closed() {
        let f = Fixture::new();
        let registration =
            f.registration(&f.a, ManagedUserArea::CanvasUploads, "item", b"original");
        let parent = f.a.lease().namespace.path(ManagedUserArea::CanvasUploads);
        fs::rename(&parent, parent.with_file_name("detached")).unwrap();
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("item"), b"foreign").unwrap();
        assert!(matches!(
            f.index.register_file_for_namespace(&f.a, &registration),
            Err(FileIndexError::Capability(_))
        ));
        assert_eq!(fs::read(parent.join("item")).unwrap(), b"foreign");
        assert_eq!(
            fs::read(parent.with_file_name("detached").join("item")).unwrap(),
            b"original"
        );
        assert_eq!(
            count(&f.index.lock_connection().unwrap(), "managed_files"),
            0
        );
    }
    #[test]
    fn bulk_reference_conflict_rolls_back_reset_and_earlier_registration() {
        let f = Fixture::new();
        let old = f.register(&f.a, ManagedUserArea::Output, "old");
        f.index
            .attach_reference_for_namespace(&f.a, old.id, "asset", "keep")
            .unwrap();
        let conflict = f.registration(&f.a, ManagedUserArea::Output, "conflict", b"a");
        let record = f
            .index
            .register_file_for_namespace(&f.a, &conflict)
            .unwrap();
        let path = f.a.lease().namespace.output_dir().join("conflict");
        fs::rename(&path, path.with_file_name("saved")).unwrap();
        fs::write(&path, b"other").unwrap();
        let other = NamespacedManagedFileRegistration::new(
            &f.a,
            f.a.open_existing_regular(
                &ManagedFileKey::new(ManagedUserArea::Output, "conflict").unwrap(),
            )
            .unwrap(),
            "image",
            "user",
        )
        .unwrap();
        let registrations = vec![
            NamespacedFileReferenceRegistration {
                file: f.registration(&f.a, ManagedUserArea::Output, "new", b"a"),
                owner_type: "asset".into(),
                owner_id: "new".into(),
            },
            NamespacedFileReferenceRegistration {
                file: other,
                owner_type: "asset".into(),
                owner_id: "other".into(),
            },
        ];
        assert!(matches!(
            f.index
                .replace_all_references_for_namespace(&f.a, &registrations),
            Err(FileIndexError::Capability(_))
        ));
        assert_eq!(
            f.index.reference_count_for_namespace(&f.a, old.id).unwrap(),
            1
        );
        assert!(f
            .index
            .find_file_by_path_for_namespace(&f.a, ManagedUserArea::Output, "new")
            .unwrap()
            .is_none());
        assert_eq!(
            query_file(&f.index.lock_connection().unwrap(), A, record.id)
                .unwrap()
                .unwrap()
                .physical_identity,
            record.physical_identity
        );
    }
    #[test]
    fn source_metadata_and_all_preview_trigger_branches_are_checked() {
        let f = Fixture::new();
        let (a, p, mut reg) = f.preview(&f.a, "triggers");
        let b = f.register(&f.b, ManagedUserArea::Output, "b");
        let bp = f.register(&f.b, ManagedUserArea::Previews, "bp");
        reg.source_size = 6;
        assert!(matches!(
            f.index.upsert_preview_for_namespace(&f.a, &reg),
            Err(FileIndexError::InvalidValue(_))
        ));
        reg.source_size = 5;
        reg.source_mtime_ns += 1;
        assert!(matches!(
            f.index.upsert_preview_for_namespace(&f.a, &reg),
            Err(FileIndexError::InvalidValue(_))
        ));
        reg.source_mtime_ns -= 1;
        let preview = f.index.upsert_preview_for_namespace(&f.a, &reg).unwrap();
        let mut foreign = reg.clone();
        foreign.preview_file_id = bp.id;
        assert!(matches!(
            f.index.upsert_preview_for_namespace(&f.a, &foreign),
            Err(FileIndexError::InvalidValue(_))
        ));
        f.index
            .attach_reference_for_namespace(&f.a, a.id, "asset", "x")
            .unwrap();
        let c = f.index.lock_connection().unwrap();
        assert!(c
            .execute(
                "INSERT INTO file_references VALUES(?1,?2,'asset','z',1)",
                params![A, b.id.0]
            )
            .is_err());
        for sql in [
            "UPDATE file_references SET user_public_id=?1 WHERE user_public_id=?2",
            "UPDATE file_references SET file_id=?3 WHERE user_public_id=?2",
        ] {
            let result = if sql.contains("?3") {
                c.execute(sql, params![B, A, b.id.0])
            } else {
                c.execute(sql, params![B, A])
            };
            assert!(result.is_err());
        }
        for (source, target) in [(b.id, p.id), (a.id, bp.id)] {
            assert!(c.execute("INSERT INTO preview_cache(user_public_id,source_file_id,preview_file_id,purpose,longest_edge,source_size,source_mtime_ns,cache_version,status,last_accessed_at,created_at,updated_at) VALUES(?1,?2,?3,'different',32,5,1,1,'ready',1,1,1)",params![A,source.0,target.0]).is_err());
        }
        for (sql, args) in [
            (
                "UPDATE preview_cache SET user_public_id=?1 WHERE id=?2",
                vec![rusqlite::types::Value::Text(B.into()), preview.id.into()],
            ),
            (
                "UPDATE preview_cache SET source_file_id=?1 WHERE id=?2",
                vec![b.id.0.into(), preview.id.into()],
            ),
            (
                "UPDATE preview_cache SET preview_file_id=?1 WHERE id=?2",
                vec![bp.id.0.into(), preview.id.into()],
            ),
        ] {
            assert!(c.execute(sql, rusqlite::params_from_iter(args)).is_err());
        }
        assert_eq!(count(&c, "preview_cache"), 1);
        assert_eq!(count(&c, "file_references"), 1);
    }
    #[test]
    fn identities_round_trip_without_truncation_and_scalar_overflows_reject() {
        let windows1 = StableFileIdentity::Windows {
            volume: u64::MAX,
            file_id: [255; 16],
        };
        let mut tail = [255; 16];
        tail[15] = 254;
        let windows2 = StableFileIdentity::Windows {
            volume: u64::MAX,
            file_id: tail,
        };
        let unix = StableFileIdentity::Unix {
            device: u64::MAX,
            inode: 0x8000000000000001,
        };
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA_V2).unwrap();
        for (n, identity) in [(1, windows1), (2, windows2), (3, unix)] {
            c.execute("INSERT INTO managed_files(user_public_id,managed_area,path,physical_identity,kind,retention_policy,created_at,last_accessed_at) VALUES(?1,'output',?2,?3,'image','user',1,1)",params![A,n.to_string(),identity.to_storage_bytes()]).unwrap();
            assert_eq!(
                query_identity(&c, A, identity)
                    .unwrap()
                    .unwrap()
                    .physical_identity,
                identity
            );
        }
        assert_eq!(count(&c, "managed_files"), 3);
        assert!(to_sql_i64("size", u64::MAX).is_err());
        assert_eq!(to_sql_i64("size", i64::MAX as u64).unwrap(), i64::MAX);
        assert_eq!(
            system_time_ns(UNIX_EPOCH - Duration::from_nanos(1)).unwrap(),
            -1
        );
        assert_eq!(
            system_time_ns(UNIX_EPOCH + Duration::from_nanos(1)).unwrap(),
            1
        );
        assert!(system_time_ns(UNIX_EPOCH + Duration::from_secs(10_000_000_000)).is_err());
        c.execute(
            "UPDATE managed_files SET managed_area='unknown' WHERE id=1",
            [],
        )
        .unwrap();
        assert!(matches!(
            query_file(&c, A, ManagedFileId(1)),
            Err(FileIndexError::Damaged(_))
        ));
        c.execute(
            "UPDATE managed_files SET managed_area='output',path='a/../b' WHERE id=1",
            [],
        )
        .unwrap();
        assert!(matches!(
            query_file(&c, A, ManagedFileId(1)),
            Err(FileIndexError::Damaged(_))
        ));
        assert!(decode_identity(&[1, 2, 3]).is_err());
    }
    #[test]
    fn every_old_adapter_fails_before_poisoned_sql_or_invalid_paths() {
        let f = Fixture::new();
        let index = f.index.clone();
        let _ = std::thread::spawn(move || {
            let _guard = index.connection.lock().unwrap();
            panic!("poison only test connection");
        })
        .join();
        let file = ManagedFileRegistration {
            path: PathBuf::from("\0invalid"),
            kind: String::new(),
            byte_size: u64::MAX,
            managed: true,
            retention_policy: String::new(),
        };
        let key = PreviewKey {
            source_file_id: ManagedFileId(-1),
            purpose: String::new(),
            longest_edge: 0,
            cache_version: 0,
        };
        let preview = PreviewRegistration {
            key: key.clone(),
            preview_file_id: ManagedFileId(-2),
            source_size: u64::MAX,
            source_mtime_ns: 0,
            status: String::new(),
            last_accessed_at: 0,
        };
        macro_rules! closed {
            ($result:expr) => {
                assert!(matches!($result, Err(FileIndexError::NamespaceRequired)));
            };
        }
        closed!(f.index.register_file(&file));
        closed!(f.index.find_file_by_path(&file.path));
        closed!(f.index.find_file_by_id(key.source_file_id));
        closed!(f.index.mark_pending_delete(key.source_file_id, true));
        closed!(f.index.delete_file(key.source_file_id));
        closed!(f.index.attach_reference(key.source_file_id, "", ""));
        closed!(f.index.detach_reference(key.source_file_id, "", ""));
        closed!(f.index.reference_count(key.source_file_id));
        closed!(f.index.clear_all_references());
        closed!(f.index.replace_all_references(&[]));
        closed!(f.index.stats_by_kind());
        closed!(f.index.upsert_preview(&preview));
        closed!(f.index.find_preview(&key));
        closed!(f.index.touch_preview(&key, 0));
        closed!(f.index.delete_preview(&key));
        closed!(f.index.delete_previews_for_source(key.source_file_id));
        closed!(f.index.least_recently_used_previews(0));
        closed!(f.index.least_recently_used_previews(1));
        closed!(f.index.replace_all_references(&[FileReferenceRegistration {
            file,
            owner_type: String::new(),
            owner_id: String::new()
        }]));
    }
    #[test]
    fn orphan_candidates_are_fixed_scoped_and_do_not_unlink() {
        let f = Fixture::new();
        let a = f.register(&f.a, ManagedUserArea::CanvasUploads, "same");
        let b = f.register(&f.b, ManagedUserArea::CanvasUploads, "same");
        f.index
            .attach_reference_for_namespace(&f.b, b.id, "asset", "held")
            .unwrap();
        let key = ManagedFileKey::new(ManagedUserArea::CanvasUploads, "same").unwrap();
        let file = f.a.open_existing_regular(&key).unwrap();
        let candidate = f
            .index
            .orphan_candidate_for_namespace(&f.a, &file, SystemTime::now())
            .unwrap()
            .unwrap();
        assert_eq!(candidate.indexed_file_id, Some(a.id));
        assert_eq!(candidate.key, key);
        let foreign = f.b.open_existing_regular(&key).unwrap();
        assert!(f
            .index
            .orphan_candidate_for_namespace(&f.b, &foreign, SystemTime::now())
            .unwrap()
            .is_none());
        assert!(f
            .index
            .orphan_candidate_for_namespace(&f.b, &file, SystemTime::now())
            .is_err());
        assert!(f
            .index
            .orphan_candidate_for_namespace(&f.a, &file, UNIX_EPOCH)
            .unwrap()
            .is_none());
        let unindexed =
            f.a.create_new_regular(
                &ManagedFileKey::new(ManagedUserArea::ReferencesLibrary, "unindexed").unwrap(),
            )
            .unwrap();
        assert_eq!(
            f.index
                .orphan_candidate_for_namespace(&f.a, &unindexed, SystemTime::now())
                .unwrap()
                .unwrap()
                .indexed_file_id,
            None
        );
        let wrong =
            f.a.create_new_regular(&ManagedFileKey::new(ManagedUserArea::Output, "wrong").unwrap())
                .unwrap();
        assert!(matches!(
            f.index
                .orphan_candidate_for_namespace(&f.a, &wrong, SystemTime::now()),
            Err(FileIndexError::InvalidValue(_))
        ));
        assert!(f
            .a
            .lease()
            .namespace
            .path(ManagedUserArea::CanvasUploads)
            .join("same")
            .is_file());
    }
    #[test]
    fn namespace_guard_precedes_sql_and_rereads_discovered_snapshot() {
        use std::sync::mpsc;
        let f = Fixture::new();
        let a = f.register(&f.a, ManagedUserArea::Output, "lock");
        let file = f
            .a
            .open_existing_regular(&ManagedFileKey::new(ManagedUserArea::Output, "lock").unwrap())
            .unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let authority = &f.a;
            scope.spawn(move || {
                authority
                    .with_current_regular_files(
                        &[NamespaceManagedFileCheck {
                            file: &file,
                            expected: Some(a.physical_identity),
                        }],
                        |_| {
                            entered_tx.send(()).unwrap();
                            release_rx.recv().unwrap();
                            Ok(())
                        },
                    )
                    .unwrap()
            });
            entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let (started_tx, started_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let index = &f.index;
            let authority = &f.a;
            scope.spawn(move || {
                started_tx.send(()).unwrap();
                done_tx
                    .send(index.find_file_by_id_for_namespace(authority, a.id))
                    .unwrap();
            });
            started_rx.recv().unwrap();
            assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
            // A filesystem wait must leave SQLite available to independent SQL.
            let c = f
                .index
                .connection
                .try_lock()
                .expect("filesystem waiter must not hold SQLite");
            assert_eq!(count(&c, "managed_files"), 1);
            drop(c);
            release_tx.send(()).unwrap();
            assert!(done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .is_some());
        });
        let stale = f.index.owned_file(A, a.id).unwrap().unwrap();
        f.index
            .lock_connection()
            .unwrap()
            .execute(
                "UPDATE managed_files SET kind='changed' WHERE user_public_id=?1 AND id=?2",
                params![A, a.id.0],
            )
            .unwrap();
        assert!(matches!(
            f.index.with_records(&f.a, &[stale], |_, _| Ok(())),
            Err(FileIndexError::ConcurrentChange)
        ));
        fn send_sync<T: Send + Sync>() {}
        send_sync::<FileIndex>();
        send_sync::<NamespaceStorageAuthority>();
    }

    #[test]
    fn schema_validation_checks_indexes_checks_triggers_and_quarantine_completeness() {
        for sql in [
            "DROP INDEX managed_files_v2_user_kind",
            "DROP INDEX managed_files_v2_user_kind; CREATE INDEX managed_files_v2_user_kind ON managed_files(kind,user_public_id)",
            "DROP TRIGGER preview_cache_v2_namespace_update",
            "CREATE TABLE legacy_unassigned_managed_files(id INTEGER)",
        ] {
            let f=Fixture::new();f.index.lock_connection().unwrap().execute_batch(sql).unwrap();
            let before=schema_signature(&f.index.lock_connection().unwrap()).unwrap();
            assert!(FileIndex::initialize(f.index.database_path()).is_err());
            assert_eq!(schema_signature(&f.index.lock_connection().unwrap()).unwrap(),before);
        }
        for schema in [
            SCHEMA_V1.replace("CHECK(byte_size >= 0)", ""),
            SCHEMA_V1.replace("ON DELETE CASCADE", ""),
            SCHEMA_V1.replace("DEFAULT 0 CHECK(managed", "DEFAULT 1 CHECK(managed"),
            SCHEMA_V1.replace(
                "PRIMARY KEY (file_id, owner_type, owner_id)",
                "PRIMARY KEY (file_id, owner_id, owner_type)",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("bad.sqlite3");
            let c = Connection::open(&path).unwrap();
            c.execute_batch(&schema).unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
            let signature = schema_signature(&c).unwrap();
            drop(c);
            assert!(FileIndex::initialize(&path).is_err());
            let c = Connection::open(&path).unwrap();
            assert_eq!(schema_signature(&c).unwrap(), signature);
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }
    #[test]
    fn physical_aliases_hardlinks_and_resolving_case_aliases_cannot_register() {
        let f = Fixture::new();
        let original = f.register(&f.a, ManagedUserArea::Output, "Photo.png");
        let root = f.a.lease().namespace.output_dir();
        let alias = ManagedFileKey::new(ManagedUserArea::Output, "photo.png").unwrap();
        if root.join("photo.png").is_file() {
            assert!(f.a.open_existing_regular(&alias).is_err());
        }
        fs::rename(root.join("Photo.png"), root.join("renamed.png")).unwrap();
        let renamed =
            f.a.open_existing_regular(
                &ManagedFileKey::new(ManagedUserArea::Output, "renamed.png").unwrap(),
            )
            .unwrap();
        let registration =
            NamespacedManagedFileRegistration::new(&f.a, renamed, "image", "user").unwrap();
        assert!(matches!(
            f.index.register_file_for_namespace(&f.a, &registration),
            Err(FileIndexError::Capability(_))
        ));
        assert_eq!(
            query_file(&f.index.lock_connection().unwrap(), A, original.id)
                .unwrap()
                .unwrap()
                .path,
            PathBuf::from("Photo.png")
        );
        fs::hard_link(root.join("renamed.png"), root.join("hardlink.png")).unwrap();
        assert!(f
            .a
            .open_existing_regular(
                &ManagedFileKey::new(ManagedUserArea::Output, "hardlink.png").unwrap()
            )
            .is_err());
        assert!(f
            .a
            .open_existing_regular(
                &ManagedFileKey::new(ManagedUserArea::Output, "renamed.png").unwrap()
            )
            .is_err());
    }
    #[test]
    fn preview_bulk_missing_second_file_and_changed_order_are_all_or_error() {
        let f = Fixture::new();
        let (_, _, first) = f.preview(&f.a, "first");
        let (_, _, second) = f.preview(&f.a, "second");
        f.index.upsert_preview_for_namespace(&f.a, &first).unwrap();
        f.index.upsert_preview_for_namespace(&f.a, &second).unwrap();
        fs::remove_file(
            f.a.lease()
                .namespace
                .preview_dir()
                .join("preview-second.png"),
        )
        .unwrap();
        assert!(matches!(
            f.index.least_recently_used_previews_for_namespace(&f.a, 10),
            Err(FileIndexError::MissingManagedFile)
        ));
        assert_eq!(
            count(&f.index.lock_connection().unwrap(), "preview_cache"),
            2
        );
        let f = Fixture::new();
        let (_, _, first) = f.preview(&f.a, "first");
        let (_, _, second) = f.preview(&f.a, "second");
        let first = f.index.upsert_preview_for_namespace(&f.a, &first).unwrap();
        f.index.upsert_preview_for_namespace(&f.a, &second).unwrap();
        let records =
            query_previews(&f.index.lock_connection().unwrap(), A, None, Some(10)).unwrap();
        f.index
            .lock_connection()
            .unwrap()
            .execute(
                "UPDATE preview_cache SET last_accessed_at=999 WHERE user_public_id=?1 AND id=?2",
                params![A, first.id],
            )
            .unwrap();
        assert!(matches!(
            f.index.with_previews(
                &f.a,
                &records,
                |c| query_previews(c, A, None, Some(10)),
                |_| Ok(())
            ),
            Err(FileIndexError::ConcurrentChange)
        ));
        assert_eq!(
            count(&f.index.lock_connection().unwrap(), "preview_cache"),
            2
        );
    }
    #[test]
    fn sqlite_callback_errors_remain_typed_and_guard_survives_until_commit() {
        use std::sync::mpsc;
        let f = Fixture::new();
        let registration = f.registration(&f.a, ManagedUserArea::Output, "sql-error", b"a");
        f.index.lock_connection().unwrap().execute_batch("CREATE TRIGGER reject_registration BEFORE INSERT ON managed_files BEGIN SELECT RAISE(ABORT,'test SQL failure'); END;").unwrap();
        assert!(matches!(
            f.index.register_file_for_namespace(&f.a, &registration),
            Err(FileIndexError::Sqlite(_))
        ));
        assert_eq!(
            count(&f.index.lock_connection().unwrap(), "managed_files"),
            0
        );
        f.index
            .lock_connection()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_registration")
            .unwrap();
        let record = f
            .index
            .register_file_for_namespace(&f.a, &registration)
            .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let authority = &f.a;
            scope.spawn(move || {
                started_rx.recv().unwrap();
                let result = authority.create_new_regular(
                    &ManagedFileKey::new(ManagedUserArea::Output, "after-commit").unwrap(),
                );
                done_tx.send(result.is_ok()).unwrap();
            });
            f.index.with_records(&f.a,std::slice::from_ref(&record),|tx,_| {
                tx.execute("UPDATE managed_files SET pending_delete=1 WHERE user_public_id=?1 AND id=?2",params![A,record.id.0])?;
                started_tx.send(()).unwrap();
                assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
                assert_eq!(query_file(tx,A,record.id)?.unwrap().pending_delete,true);
                Ok(())
            }).unwrap();
            assert!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        });
        assert!(
            f.index
                .find_file_by_id_for_namespace(&f.a, record.id)
                .unwrap()
                .unwrap()
                .pending_delete
        );
    }
    #[test]
    fn user_only_aggregates_and_resets_need_no_filesystem_guard() {
        let f = Fixture::new();
        let a = f.register(&f.a, ManagedUserArea::Output, "a");
        f.index
            .attach_reference_for_namespace(&f.a, a.id, "asset", "one")
            .unwrap();
        let root = f.a.lease().namespace.root();
        fs::rename(root, root.with_extension("detached")).unwrap();
        assert_eq!(
            f.index.stats_by_kind_for_namespace(&f.a).unwrap()[0].file_count,
            1
        );
        f.index
            .replace_all_references_for_namespace(&f.a, &[])
            .unwrap();
        f.index.clear_all_references_for_namespace(&f.a).unwrap();
        assert_eq!(
            reference_count_owned(&f.index.lock_connection().unwrap(), A, a.id).unwrap(),
            0
        );
        assert!(!root.exists());
    }

    #[test]
    fn fresh_schema_keeps_device_pragmas_and_no_unassigned_rows() {
        let f = Fixture::new();
        let c = f.index.lock_connection().unwrap();
        assert_eq!(
            c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        assert_eq!(
            c.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            c.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3000
        );
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name GLOB 'legacy_unassigned_*'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(count(&c, "managed_files"), 0);
    }

    #[test]
    fn configuration_failure_cannot_follow_a_committed_migration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index.sqlite3");
        seed_v1(&path);
        let before = schema_signature(&Connection::open(&path).unwrap()).unwrap();
        MIGRATION_FAILURE.with(|p| p.set(3));
        assert!(FileIndex::initialize(&path).is_err());
        MIGRATION_FAILURE.with(|p| p.set(0));
        let c = Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(schema_signature(&c).unwrap(), before);
        assert_eq!(count(&c, "managed_files"), 2);
    }

    #[test]
    fn public_discovery_rechecks_rows_and_complete_candidate_sets_after_barrier() {
        use std::sync::mpsc;
        let f = Fixture::new();
        let record = f.register(&f.a, ManagedUserArea::Output, "barrier");
        let (arrived_tx, arrived_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let index = &f.index;
            let authority = &f.a;
            let worker = scope.spawn(move || {
                AFTER_DISCOVERY.with(|slot| {
                    *slot.borrow_mut() = Some(Box::new(move || {
                        arrived_tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                    }))
                });
                index.find_file_by_id_for_namespace(authority, record.id)
            });
            arrived_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            f.index
                .lock_connection()
                .unwrap()
                .execute(
                    "UPDATE managed_files SET pending_delete=1 WHERE user_public_id=?1 AND id=?2",
                    params![A, record.id.0],
                )
                .unwrap();
            resume_tx.send(()).unwrap();
            assert!(matches!(
                worker.join().unwrap(),
                Err(FileIndexError::ConcurrentChange)
            ));
        });
        let (_, _, first) = f.preview(&f.a, "barrier-first");
        f.index.upsert_preview_for_namespace(&f.a, &first).unwrap();
        let (arrived_tx, arrived_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let index = &f.index;
            let authority = &f.a;
            let worker = scope.spawn(move || {
                AFTER_DISCOVERY.with(|slot| {
                    *slot.borrow_mut() = Some(Box::new(move || {
                        arrived_tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                    }))
                });
                index.least_recently_used_previews_for_namespace(authority, 10)
            });
            arrived_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let (_, _, inserted) = f.preview(&f.a, "barrier-inserted");
            f.index
                .upsert_preview_for_namespace(&f.a, &inserted)
                .unwrap();
            resume_tx.send(()).unwrap();
            assert!(matches!(
                worker.join().unwrap(),
                Err(FileIndexError::ConcurrentChange)
            ));
        });
        assert_eq!(
            f.index
                .least_recently_used_previews_for_namespace(&f.a, 10)
                .unwrap()
                .len(),
            2
        );
    }
}
