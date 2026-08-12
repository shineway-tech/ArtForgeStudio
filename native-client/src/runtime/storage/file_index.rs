use rusqlite::{params, Connection, ErrorCode, OptionalExtension, Row};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub(super) const FILE_INDEX_SCHEMA_VERSION: i32 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_millis(3_000);

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

const MANAGED_FILE_SELECT: &str = "
    SELECT id, path, kind, byte_size, managed, retention_policy,
           created_at, last_accessed_at, pending_delete
      FROM managed_files";

const PREVIEW_SELECT: &str = "
    SELECT pc.id,
           pc.source_file_id,
           pc.preview_file_id,
           source.path,
           preview.path,
           preview.byte_size,
           pc.purpose,
           pc.longest_edge,
           pc.source_size,
           pc.source_mtime_ns,
           pc.cache_version,
           pc.status,
           pc.last_accessed_at,
           pc.created_at,
           pc.updated_at
      FROM preview_cache AS pc
      JOIN managed_files AS source ON source.id = pc.source_file_id
      JOIN managed_files AS preview ON preview.id = pc.preview_file_id";

#[derive(Debug, Error)]
pub(super) enum FileIndexError {
    #[error("SQLite file index error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("file index I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("the file index is damaged: {0}")]
    Damaged(String),
    #[error("file index schema version {found} is newer than the supported version {supported}")]
    UnsupportedSchema { found: i32, supported: i32 },
    #[error("invalid file index value: {0}")]
    InvalidValue(String),
    #[error("the file index connection lock was poisoned")]
    LockPoisoned,
}

pub(super) type FileIndexResult<T> = std::result::Result<T, FileIndexError>;

#[derive(Clone)]
pub(super) struct FileIndex {
    database_path: Arc<PathBuf>,
    connection: Arc<Mutex<Connection>>,
}

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

/// Opens the index, moving an invalid/corrupt database aside and rebuilding it.
///
/// A schema created by a newer application version is never replaced. This keeps
/// downgrade behavior safe while allowing this cache-derived index to recover
/// automatically from malformed SQLite files or an incomplete local schema.
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
    let index = initialize_file_index(path)?;
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

        match Self::open(&database_path) {
            Ok(index) => Ok(index),
            Err(error) if database_path.is_file() && error.is_rebuildable() => {
                move_damaged_database_aside(&database_path)?;
                Self::open(&database_path)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn database_path(&self) -> &Path {
        self.database_path.as_path()
    }

    fn open(database_path: &Path) -> FileIndexResult<Self> {
        let mut connection = Connection::open(database_path)?;
        configure_connection(&connection)?;
        check_integrity(&connection)?;
        migrate_schema(&mut connection)?;
        validate_schema(&connection)?;

        Ok(Self {
            database_path: Arc::new(database_path.to_path_buf()),
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub(super) fn register_file(
        &self,
        registration: &ManagedFileRegistration,
    ) -> FileIndexResult<ManagedFileRecord> {
        let path = normalized_path_text(&registration.path)?;
        let kind = required_text("kind", &registration.kind)?;
        let retention_policy = required_text("retention_policy", &registration.retention_policy)?;
        let byte_size = to_sql_i64("byte_size", registration.byte_size)?;
        let timestamp = now_millis();
        let connection = self.lock_connection()?;

        connection.execute(
            "INSERT INTO managed_files (
                 path, kind, byte_size, managed, retention_policy,
                 created_at, last_accessed_at, pending_delete
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 0)
             ON CONFLICT(path) DO UPDATE SET
                 kind = excluded.kind,
                 byte_size = excluded.byte_size,
                 managed = excluded.managed,
                 retention_policy = excluded.retention_policy,
                 last_accessed_at = MAX(managed_files.last_accessed_at, excluded.last_accessed_at),
                 pending_delete = 0",
            params![
                path,
                kind,
                byte_size,
                registration.managed,
                retention_policy,
                timestamp
            ],
        )?;

        query_managed_file_by_path(&connection, &path)?.ok_or_else(|| {
            FileIndexError::Damaged("registered file could not be read back".to_string())
        })
    }

    pub(super) fn find_file_by_path(
        &self,
        path: &Path,
    ) -> FileIndexResult<Option<ManagedFileRecord>> {
        let path = normalized_path_text(path)?;
        let connection = self.lock_connection()?;
        query_managed_file_by_path(&connection, &path)
    }

    pub(super) fn find_file_by_id(
        &self,
        file_id: ManagedFileId,
    ) -> FileIndexResult<Option<ManagedFileRecord>> {
        let connection = self.lock_connection()?;
        let sql = format!("{MANAGED_FILE_SELECT} WHERE id = ?1");
        connection
            .query_row(&sql, params![file_id.0], map_managed_file)
            .optional()
            .map_err(Into::into)
    }

    pub(super) fn mark_pending_delete(
        &self,
        file_id: ManagedFileId,
        pending: bool,
    ) -> FileIndexResult<bool> {
        let connection = self.lock_connection()?;
        Ok(connection.execute(
            "UPDATE managed_files SET pending_delete = ?2 WHERE id = ?1",
            params![file_id.0, pending],
        )? > 0)
    }

    pub(super) fn delete_file(&self, file_id: ManagedFileId) -> FileIndexResult<bool> {
        let connection = self.lock_connection()?;
        Ok(connection.execute(
            "DELETE FROM managed_files WHERE id = ?1",
            params![file_id.0],
        )? > 0)
    }

    pub(super) fn attach_reference(
        &self,
        file_id: ManagedFileId,
        owner_type: &str,
        owner_id: &str,
    ) -> FileIndexResult<bool> {
        let owner_type = required_text("owner_type", owner_type)?;
        let owner_id = required_text("owner_id", owner_id)?;
        let connection = self.lock_connection()?;
        Ok(connection.execute(
            "INSERT OR IGNORE INTO file_references (file_id, owner_type, owner_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![file_id.0, owner_type, owner_id, now_millis()],
        )? > 0)
    }

    pub(super) fn detach_reference(
        &self,
        file_id: ManagedFileId,
        owner_type: &str,
        owner_id: &str,
    ) -> FileIndexResult<bool> {
        let owner_type = required_text("owner_type", owner_type)?;
        let owner_id = required_text("owner_id", owner_id)?;
        let connection = self.lock_connection()?;
        Ok(connection.execute(
            "DELETE FROM file_references
              WHERE file_id = ?1 AND owner_type = ?2 AND owner_id = ?3",
            params![file_id.0, owner_type, owner_id],
        )? > 0)
    }

    pub(super) fn reference_count(&self, file_id: ManagedFileId) -> FileIndexResult<u64> {
        let connection = self.lock_connection()?;
        let count = connection.query_row(
            "SELECT COUNT(*) FROM file_references WHERE file_id = ?1",
            params![file_id.0],
            |row| row.get::<_, i64>(0),
        )?;
        nonnegative_u64("reference count", count)
    }

    pub(super) fn clear_all_references(&self) -> FileIndexResult<()> {
        let connection = self.lock_connection()?;
        connection.execute("DELETE FROM file_references", [])?;
        Ok(())
    }

    pub(super) fn replace_all_references(
        &self,
        registrations: &[FileReferenceRegistration],
    ) -> FileIndexResult<()> {
        struct PreparedReference {
            path: String,
            kind: String,
            byte_size: i64,
            managed: bool,
            retention_policy: String,
            owner_type: String,
            owner_id: String,
        }

        let mut prepared = Vec::with_capacity(registrations.len());
        for registration in registrations {
            prepared.push(PreparedReference {
                path: normalized_path_text(&registration.file.path)?,
                kind: required_text("kind", &registration.file.kind)?.to_string(),
                byte_size: to_sql_i64("byte_size", registration.file.byte_size)?,
                managed: registration.file.managed,
                retention_policy: required_text(
                    "retention_policy",
                    &registration.file.retention_policy,
                )?
                .to_string(),
                owner_type: required_text("owner_type", &registration.owner_type)?.to_string(),
                owner_id: required_text("owner_id", &registration.owner_id)?.to_string(),
            });
        }

        let timestamp = now_millis();
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM file_references", [])?;
        for reference in prepared {
            transaction.execute(
                "INSERT INTO managed_files (
                     path, kind, byte_size, managed, retention_policy,
                     created_at, last_accessed_at, pending_delete
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 0)
                 ON CONFLICT(path) DO UPDATE SET
                     kind = excluded.kind,
                     byte_size = excluded.byte_size,
                     managed = excluded.managed,
                     retention_policy = excluded.retention_policy,
                     last_accessed_at = MAX(managed_files.last_accessed_at, excluded.last_accessed_at),
                     pending_delete = 0",
                params![
                    reference.path,
                    reference.kind,
                    reference.byte_size,
                    reference.managed,
                    reference.retention_policy,
                    timestamp
                ],
            )?;
            let file_id = transaction.query_row(
                "SELECT id FROM managed_files WHERE path = ?1",
                params![reference.path],
                |row| row.get::<_, i64>(0),
            )?;
            transaction.execute(
                "INSERT OR IGNORE INTO file_references (file_id, owner_type, owner_id, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    file_id,
                    reference.owner_type,
                    reference.owner_id,
                    timestamp
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn stats_by_kind(&self) -> FileIndexResult<Vec<FileKindStats>> {
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare(
            "SELECT kind,
                    COUNT(*),
                    COALESCE(SUM(byte_size), 0),
                    COALESCE(SUM(managed), 0),
                    COALESCE(SUM(pending_delete), 0)
               FROM managed_files
              GROUP BY kind
              ORDER BY kind",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (kind, file_count, byte_size, managed_count, pending_delete_count) = row?;
            result.push(FileKindStats {
                kind,
                file_count: nonnegative_u64("file count", file_count)?,
                byte_size: nonnegative_u64("byte size", byte_size)?,
                managed_count: nonnegative_u64("managed count", managed_count)?,
                pending_delete_count: nonnegative_u64(
                    "pending-delete count",
                    pending_delete_count,
                )?,
            });
        }
        Ok(result)
    }

    pub(super) fn upsert_preview(
        &self,
        registration: &PreviewRegistration,
    ) -> FileIndexResult<PreviewCacheRecord> {
        validate_preview_key(&registration.key)?;
        if registration.key.source_file_id == registration.preview_file_id {
            return Err(FileIndexError::InvalidValue(
                "source_file_id and preview_file_id must differ".to_string(),
            ));
        }
        let status = required_text("status", &registration.status)?;
        let source_size = to_sql_i64("source_size", registration.source_size)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let timestamp = now_millis();

        transaction.execute(
            "INSERT INTO preview_cache (
                 source_file_id, preview_file_id, purpose, longest_edge,
                 source_size, source_mtime_ns, cache_version, status,
                 last_accessed_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
             ON CONFLICT(source_file_id, purpose, longest_edge, cache_version)
             DO UPDATE SET
                 preview_file_id = excluded.preview_file_id,
                 source_size = excluded.source_size,
                 source_mtime_ns = excluded.source_mtime_ns,
                 status = excluded.status,
                 last_accessed_at = excluded.last_accessed_at,
                 updated_at = excluded.updated_at",
            params![
                registration.key.source_file_id.0,
                registration.preview_file_id.0,
                registration.key.purpose.trim(),
                i64::from(registration.key.longest_edge),
                source_size,
                registration.source_mtime_ns,
                i64::from(registration.key.cache_version),
                status,
                registration.last_accessed_at,
                timestamp,
            ],
        )?;
        transaction.execute(
            "UPDATE managed_files
                SET last_accessed_at = MAX(last_accessed_at, ?2)
              WHERE id = ?1",
            params![
                registration.preview_file_id.0,
                registration.last_accessed_at
            ],
        )?;

        let record = query_preview(&transaction, &registration.key)?.ok_or_else(|| {
            FileIndexError::Damaged("upserted preview could not be read back".to_string())
        })?;
        transaction.commit()?;
        Ok(record)
    }

    pub(super) fn find_preview(
        &self,
        key: &PreviewKey,
    ) -> FileIndexResult<Option<PreviewCacheRecord>> {
        validate_preview_key(key)?;
        let connection = self.lock_connection()?;
        query_preview(&connection, key)
    }

    pub(super) fn touch_preview(
        &self,
        key: &PreviewKey,
        accessed_at: i64,
    ) -> FileIndexResult<bool> {
        validate_preview_key(key)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let preview_file_id = transaction
            .query_row(
                "SELECT preview_file_id
                   FROM preview_cache
                  WHERE source_file_id = ?1
                    AND purpose = ?2
                    AND longest_edge = ?3
                    AND cache_version = ?4",
                preview_key_params(key),
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        let Some(preview_file_id) = preview_file_id else {
            transaction.commit()?;
            return Ok(false);
        };
        transaction.execute(
            "UPDATE preview_cache
                SET last_accessed_at = MAX(last_accessed_at, ?5)
              WHERE source_file_id = ?1
                AND purpose = ?2
                AND longest_edge = ?3
                AND cache_version = ?4",
            params![
                key.source_file_id.0,
                key.purpose.trim(),
                i64::from(key.longest_edge),
                i64::from(key.cache_version),
                accessed_at
            ],
        )?;
        transaction.execute(
            "UPDATE managed_files
                SET last_accessed_at = MAX(last_accessed_at, ?2)
              WHERE id = ?1",
            params![preview_file_id, accessed_at],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// Removes only the preview relationship and returns the file information.
    /// The caller can delete the physical preview first, then remove its
    /// `managed_files` row with [`FileIndex::delete_file`].
    pub(super) fn delete_preview(
        &self,
        key: &PreviewKey,
    ) -> FileIndexResult<Option<PreviewCacheRecord>> {
        validate_preview_key(key)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let record = query_preview(&transaction, key)?;
        if let Some(record) = &record {
            transaction.execute(
                "DELETE FROM preview_cache WHERE id = ?1",
                params![record.id],
            )?;
        }
        transaction.commit()?;
        Ok(record)
    }

    pub(super) fn delete_previews_for_source(
        &self,
        source_file_id: ManagedFileId,
    ) -> FileIndexResult<Vec<PreviewCacheRecord>> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let records = query_previews_for_source(&transaction, source_file_id)?;
        transaction.execute(
            "DELETE FROM preview_cache WHERE source_file_id = ?1",
            params![source_file_id.0],
        )?;
        transaction.commit()?;
        Ok(records)
    }

    pub(super) fn least_recently_used_previews(
        &self,
        limit: usize,
    ) -> FileIndexResult<Vec<PreviewCacheRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit)
            .map_err(|_| FileIndexError::InvalidValue("LRU limit is too large".to_string()))?;
        let connection = self.lock_connection()?;
        let sql = format!("{PREVIEW_SELECT} ORDER BY pc.last_accessed_at, pc.id LIMIT ?1");
        let mut statement = connection.prepare(&sql)?;
        let records = collect_previews(statement.query_map(params![limit], map_preview)?)?;
        Ok(records)
    }

    fn lock_connection(&self) -> FileIndexResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| FileIndexError::LockPoisoned)
    }
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

impl FileIndexError {
    fn is_rebuildable(&self) -> bool {
        match self {
            Self::Damaged(_) => true,
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _)) => matches!(
                error.code,
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase
            ),
            _ => false,
        }
    }
}

fn configure_connection(connection: &Connection) -> FileIndexResult<()> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let journal_mode = connection.query_row("PRAGMA journal_mode = WAL", [], |row| {
        row.get::<_, String>(0)
    })?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(FileIndexError::Damaged(format!(
            "WAL mode could not be enabled (SQLite returned {journal_mode})"
        )));
    }
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

fn check_integrity(connection: &Connection) -> FileIndexResult<()> {
    let result =
        connection.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))?;
    if result.eq_ignore_ascii_case("ok") {
        Ok(())
    } else {
        Err(FileIndexError::Damaged(result))
    }
}

fn migrate_schema(connection: &mut Connection) -> FileIndexResult<()> {
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))?;
    if version > FILE_INDEX_SCHEMA_VERSION {
        return Err(FileIndexError::UnsupportedSchema {
            found: version,
            supported: FILE_INDEX_SCHEMA_VERSION,
        });
    }

    let existing_known_tables = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master
          WHERE type = 'table'
            AND name IN ('managed_files', 'file_references', 'preview_cache')",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if existing_known_tables > 0 {
        // Validate before CREATE INDEX statements run so a valid SQLite file with an
        // interrupted/incompatible schema is classified as derived-state damage and rebuilt.
        validate_schema(connection)?;
    }

    let transaction = connection.transaction()?;
    transaction.execute_batch(SCHEMA_V1)?;
    transaction.pragma_update(None, "user_version", FILE_INDEX_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn validate_schema(connection: &Connection) -> FileIndexResult<()> {
    // Preparing these queries catches partial or incompatible v1 tables without
    // waiting for a business operation to fail later.
    for query in [
        "SELECT id, path, kind, byte_size, managed, retention_policy,
                created_at, last_accessed_at, pending_delete
           FROM managed_files LIMIT 0",
        "SELECT file_id, owner_type, owner_id, created_at
           FROM file_references LIMIT 0",
        "SELECT id, source_file_id, preview_file_id, purpose, longest_edge,
                source_size, source_mtime_ns, cache_version, status,
                last_accessed_at, created_at, updated_at
           FROM preview_cache LIMIT 0",
    ] {
        connection
            .prepare(query)
            .map_err(|error| FileIndexError::Damaged(error.to_string()))?;
    }

    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    if rows.next()?.is_some() {
        return Err(FileIndexError::Damaged(
            "foreign-key consistency check failed".to_string(),
        ));
    }
    Ok(())
}

fn query_managed_file_by_path(
    connection: &Connection,
    path: &str,
) -> FileIndexResult<Option<ManagedFileRecord>> {
    let sql = format!("{MANAGED_FILE_SELECT} WHERE path = ?1");
    connection
        .query_row(&sql, params![path], map_managed_file)
        .optional()
        .map_err(Into::into)
}

fn query_preview(
    connection: &Connection,
    key: &PreviewKey,
) -> FileIndexResult<Option<PreviewCacheRecord>> {
    let sql = format!(
        "{PREVIEW_SELECT}
         WHERE pc.source_file_id = ?1
           AND pc.purpose = ?2
           AND pc.longest_edge = ?3
           AND pc.cache_version = ?4"
    );
    connection
        .query_row(&sql, preview_key_params(key), map_preview)
        .optional()
        .map_err(Into::into)
}

fn query_previews_for_source(
    connection: &Connection,
    source_file_id: ManagedFileId,
) -> FileIndexResult<Vec<PreviewCacheRecord>> {
    let sql = format!(
        "{PREVIEW_SELECT}
         WHERE pc.source_file_id = ?1
         ORDER BY pc.id"
    );
    let mut statement = connection.prepare(&sql)?;
    let records = collect_previews(statement.query_map(params![source_file_id.0], map_preview)?)?;
    Ok(records)
}

fn collect_previews(
    rows: rusqlite::MappedRows<'_, impl FnMut(&Row<'_>) -> rusqlite::Result<PreviewCacheRecord>>,
) -> FileIndexResult<Vec<PreviewCacheRecord>> {
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

fn map_managed_file(row: &Row<'_>) -> rusqlite::Result<ManagedFileRecord> {
    Ok(ManagedFileRecord {
        id: ManagedFileId(row.get(0)?),
        path: PathBuf::from(row.get::<_, String>(1)?),
        kind: row.get(2)?,
        byte_size: row_nonnegative_u64(row, 3)?,
        managed: row.get(4)?,
        retention_policy: row.get(5)?,
        created_at: row.get(6)?,
        last_accessed_at: row.get(7)?,
        pending_delete: row.get(8)?,
    })
}

fn map_preview(row: &Row<'_>) -> rusqlite::Result<PreviewCacheRecord> {
    Ok(PreviewCacheRecord {
        id: row.get(0)?,
        source_file_id: ManagedFileId(row.get(1)?),
        preview_file_id: ManagedFileId(row.get(2)?),
        source_path: PathBuf::from(row.get::<_, String>(3)?),
        preview_path: PathBuf::from(row.get::<_, String>(4)?),
        preview_byte_size: row_nonnegative_u64(row, 5)?,
        purpose: row.get(6)?,
        longest_edge: row_positive_u32(row, 7)?,
        source_size: row_nonnegative_u64(row, 8)?,
        source_mtime_ns: row.get(9)?,
        cache_version: row_positive_u32(row, 10)?,
        status: row.get(11)?,
        last_accessed_at: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn row_nonnegative_u64(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn row_positive_u32(row: &Row<'_>, index: usize) -> rusqlite::Result<u32> {
    let value = row.get::<_, i64>(index)?;
    let value =
        u32::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))?;
    if value == 0 {
        return Err(rusqlite::Error::IntegralValueOutOfRange(index, 0));
    }
    Ok(value)
}

fn preview_key_params(key: &PreviewKey) -> [rusqlite::types::Value; 4] {
    [
        key.source_file_id.0.into(),
        key.purpose.trim().to_string().into(),
        i64::from(key.longest_edge).into(),
        i64::from(key.cache_version).into(),
    ]
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

fn normalized_path_text(path: &Path) -> FileIndexResult<String> {
    let normalized = absolute_lexical_path(path)?;
    let value = normalized.to_string_lossy().into_owned();
    #[cfg(windows)]
    let value = value.replace('/', "\\").to_lowercase();
    Ok(value)
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

fn move_damaged_database_aside(database_path: &Path) -> FileIndexResult<PathBuf> {
    let backup_path = unused_corrupt_backup_path(database_path);
    fs::rename(database_path, &backup_path)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = append_to_path(database_path, suffix);
        if !sidecar.exists() {
            continue;
        }
        let backup_sidecar = append_to_path(&backup_path, suffix);
        match fs::rename(&sidecar, backup_sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(backup_path)
}

fn unused_corrupt_backup_path(database_path: &Path) -> PathBuf {
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = database_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("storage-index");
    let extension = database_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("sqlite3");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for attempt in 0_u32.. {
        let collision_suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        let candidate = parent.join(format!(
            "{stem}.corrupt-{timestamp}{collision_suffix}.{extension}"
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 backup suffix space was exhausted")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "artforge-file-index-{}-{}-{sequence}",
                std::process::id(),
                now_millis()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn database_path(&self) -> PathBuf {
            self.0.join("storage-index.sqlite3")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn registration(path: PathBuf, kind: &str, byte_size: u64) -> ManagedFileRegistration {
        ManagedFileRegistration {
            path,
            kind: kind.to_string(),
            byte_size,
            managed: true,
            retention_policy: "cache".to_string(),
        }
    }

    fn register_source_and_preview(
        index: &FileIndex,
        directory: &TestDirectory,
        suffix: &str,
    ) -> (ManagedFileRecord, ManagedFileRecord) {
        let source = index
            .register_file(&registration(
                directory.0.join(format!("source-{suffix}.png")),
                "generated_output",
                1_024,
            ))
            .expect("register source");
        let preview = index
            .register_file(&registration(
                directory.0.join(format!("preview-{suffix}.png")),
                "preview",
                128,
            ))
            .expect("register preview");
        (source, preview)
    }

    #[test]
    fn initializes_schema_and_required_pragmas() {
        let directory = TestDirectory::new();
        let index = initialize_file_index(directory.database_path()).expect("initialize index");
        assert_eq!(index.database_path(), directory.database_path());

        let connection = index.lock_connection().expect("lock connection");
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        let synchronous: i64 = connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("synchronous");
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign keys");
        let busy_timeout: i64 = connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy timeout");
        let user_version: i32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user version");

        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(synchronous, 1);
        assert_eq!(foreign_keys, 1);
        assert_eq!(busy_timeout, 3_000);
        assert_eq!(user_version, FILE_INDEX_SCHEMA_VERSION);

        for table in ["managed_files", "file_references", "preview_cache"] {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
                     )",
                    params![table],
                    |row| row.get(0),
                )
                .expect("table existence");
            assert!(exists, "missing table {table}");
        }
    }

    #[test]
    fn registration_is_path_unique_and_updates_metadata() {
        let directory = TestDirectory::new();
        let index = FileIndex::initialize(directory.database_path()).expect("initialize index");
        let path = directory.0.join("images").join("..").join("image.png");
        let first = index
            .register_file(&registration(path, "preview", 100))
            .expect("first registration");

        let mut updated = registration(directory.0.join("image.png"), "generated_output", 250);
        updated.retention_policy = "durable".to_string();
        let second = index.register_file(&updated).expect("updated registration");

        assert_eq!(first.id, second.id);
        assert_eq!(second.byte_size, 250);
        assert_eq!(second.kind, "generated_output");
        assert_eq!(second.retention_policy, "durable");
        let count: i64 = index
            .lock_connection()
            .expect("lock")
            .query_row("SELECT COUNT(*) FROM managed_files", [], |row| row.get(0))
            .expect("count files");
        assert_eq!(count, 1);
    }

    #[test]
    fn references_are_idempotent_and_cascade_with_files() {
        let directory = TestDirectory::new();
        let index = FileIndex::initialize(directory.database_path()).expect("initialize index");
        let file = index
            .register_file(&registration(
                directory.0.join("image.png"),
                "reference",
                64,
            ))
            .expect("register file");

        assert!(index
            .attach_reference(file.id, "generation", "job-1")
            .expect("attach"));
        assert!(!index
            .attach_reference(file.id, "generation", "job-1")
            .expect("duplicate attach"));
        assert!(index
            .attach_reference(file.id, "asset", "asset-1")
            .expect("second attach"));
        assert_eq!(index.reference_count(file.id).expect("reference count"), 2);
        assert!(index
            .detach_reference(file.id, "generation", "job-1")
            .expect("detach"));
        assert!(!index
            .detach_reference(file.id, "generation", "job-1")
            .expect("duplicate detach"));
        assert_eq!(index.reference_count(file.id).expect("reference count"), 1);

        assert!(index.delete_file(file.id).expect("delete file row"));
        assert_eq!(index.reference_count(file.id).expect("reference count"), 0);
    }

    #[test]
    fn statistics_group_files_by_kind() {
        let directory = TestDirectory::new();
        let index = FileIndex::initialize(directory.database_path()).expect("initialize index");
        for (name, kind, size) in [
            ("preview-a", "preview", 100),
            ("preview-b", "preview", 150),
            ("result", "toolbox_result", 500),
        ] {
            index
                .register_file(&registration(directory.0.join(name), kind, size))
                .expect("register file");
        }
        let preview_b = index
            .find_file_by_path(&directory.0.join("preview-b"))
            .expect("find file")
            .expect("preview-b exists");
        index
            .mark_pending_delete(preview_b.id, true)
            .expect("mark pending");

        let stats = index.stats_by_kind().expect("stats");
        assert_eq!(stats.len(), 2);
        assert_eq!(
            stats[0],
            FileKindStats {
                kind: "preview".to_string(),
                file_count: 2,
                byte_size: 250,
                managed_count: 2,
                pending_delete_count: 1,
            }
        );
        assert_eq!(stats[1].kind, "toolbox_result");
        assert_eq!(stats[1].byte_size, 500);
    }

    #[test]
    fn preview_crud_touch_and_lru_are_consistent() {
        let directory = TestDirectory::new();
        let index = FileIndex::initialize(directory.database_path()).expect("initialize index");
        let (source_a, preview_a) = register_source_and_preview(&index, &directory, "a");
        let (source_b, preview_b) = register_source_and_preview(&index, &directory, "b");
        let key_a = PreviewKey {
            source_file_id: source_a.id,
            purpose: "gallery".to_string(),
            longest_edge: 512,
            cache_version: 2,
        };
        let key_b = PreviewKey {
            source_file_id: source_b.id,
            purpose: "gallery".to_string(),
            longest_edge: 512,
            cache_version: 2,
        };

        index
            .upsert_preview(&PreviewRegistration {
                key: key_a.clone(),
                preview_file_id: preview_a.id,
                source_size: source_a.byte_size,
                source_mtime_ns: 11,
                status: "ready".to_string(),
                last_accessed_at: 10,
            })
            .expect("upsert preview a");
        index
            .upsert_preview(&PreviewRegistration {
                key: key_b.clone(),
                preview_file_id: preview_b.id,
                source_size: source_b.byte_size,
                source_mtime_ns: 22,
                status: "ready".to_string(),
                last_accessed_at: 20,
            })
            .expect("upsert preview b");

        let found = index
            .find_preview(&key_a)
            .expect("find preview")
            .expect("preview exists");
        assert_eq!(found.preview_path, preview_a.path);
        assert_eq!(found.source_mtime_ns, 11);
        assert_eq!(
            index.least_recently_used_previews(1).expect("initial LRU")[0].source_file_id,
            source_a.id
        );

        assert!(index.touch_preview(&key_a, 30).expect("touch preview"));
        assert_eq!(
            index.least_recently_used_previews(1).expect("updated LRU")[0].source_file_id,
            source_b.id
        );
        assert!(!index
            .touch_preview(
                &PreviewKey {
                    purpose: "missing".to_string(),
                    ..key_a.clone()
                },
                40,
            )
            .expect("touch missing preview"));

        let deleted = index
            .delete_preview(&key_a)
            .expect("delete preview")
            .expect("deleted preview");
        assert_eq!(deleted.preview_file_id, preview_a.id);
        assert!(index.find_preview(&key_a).expect("find deleted").is_none());
        let deleted_for_source = index
            .delete_previews_for_source(source_b.id)
            .expect("delete source previews");
        assert_eq!(deleted_for_source.len(), 1);
        assert!(index
            .least_recently_used_previews(10)
            .expect("empty LRU")
            .is_empty());
    }

    #[test]
    fn corrupt_database_is_renamed_and_rebuilt() {
        let directory = TestDirectory::new();
        let database_path = directory.database_path();
        fs::write(&database_path, b"this is not sqlite").expect("write corrupt database");

        let index = initialize_file_index(&database_path).expect("rebuild corrupt database");
        assert!(index
            .stats_by_kind()
            .expect("query rebuilt index")
            .is_empty());
        let corrupt_backups = fs::read_dir(&directory.0)
            .expect("read test directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("storage-index.corrupt-")
            })
            .count();
        assert_eq!(corrupt_backups, 1);
    }

    #[test]
    fn incomplete_valid_sqlite_schema_is_renamed_and_rebuilt() {
        let directory = TestDirectory::new();
        let database_path = directory.database_path();
        let connection = Connection::open(&database_path).expect("open partial database");
        connection
            .execute("CREATE TABLE managed_files(id INTEGER PRIMARY KEY)", [])
            .expect("create partial table");
        drop(connection);

        let index = initialize_file_index(&database_path).expect("rebuild partial database");
        assert!(index.stats_by_kind().expect("query rebuilt index").is_empty());
        assert!(fs::read_dir(&directory.0)
            .expect("read test directory")
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("storage-index.corrupt-")));
    }

    #[cfg(unix)]
    #[test]
    fn database_symlink_is_rejected_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new();
        let target = directory.0.join("external.sqlite3");
        let connection = Connection::open(&target).expect("create target database");
        connection
            .execute("CREATE TABLE user_data(value TEXT)", [])
            .expect("create user table");
        drop(connection);
        let database_path = directory.database_path();
        symlink(&target, &database_path).expect("create database symlink");

        let error = match initialize_file_index(&database_path) {
            Ok(_) => panic!("database symlink must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(error, FileIndexError::InvalidValue(_)));
        let connection = Connection::open(&target).expect("reopen target database");
        let user_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'user_data')",
                [],
                |row| row.get(0),
            )
            .expect("query user table");
        assert!(user_table_exists);
    }

    #[test]
    fn newer_schema_is_not_replaced() {
        let directory = TestDirectory::new();
        let database_path = directory.database_path();
        let connection = Connection::open(&database_path).expect("open database");
        connection
            .pragma_update(None, "user_version", FILE_INDEX_SCHEMA_VERSION + 1)
            .expect("set newer version");
        drop(connection);

        let error = match initialize_file_index(&database_path) {
            Ok(_) => panic!("newer schema must not be opened"),
            Err(error) => error,
        };
        assert!(matches!(error, FileIndexError::UnsupportedSchema { .. }));
        assert!(!fs::read_dir(&directory.0)
            .expect("read test directory")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt-")));
    }

    #[test]
    fn cloned_index_is_send_sync_and_serializes_concurrent_writes() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FileIndex>();

        let directory = TestDirectory::new();
        let index = FileIndex::initialize(directory.database_path()).expect("initialize index");
        let file = index
            .register_file(&registration(
                directory.0.join("shared.png"),
                "reference",
                42,
            ))
            .expect("register file");
        let mut threads = Vec::new();
        for owner in 0..8 {
            let index = index.clone();
            threads.push(std::thread::spawn(move || {
                index
                    .attach_reference(file.id, "worker", &owner.to_string())
                    .expect("attach reference");
            }));
        }
        for thread in threads {
            thread.join().expect("join writer");
        }
        assert_eq!(index.reference_count(file.id).expect("reference count"), 8);
    }
}
