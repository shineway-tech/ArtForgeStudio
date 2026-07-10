use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS assets (
    id TEXT PRIMARY KEY,
    portable_rel_path TEXT,
    abs_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    domain TEXT NOT NULL,
    kind TEXT NOT NULL,
    mime TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    width INTEGER,
    height INTEGER,
    created_at TEXT NOT NULL,
    modified_at TEXT NOT NULL,
    source_task_id TEXT,
    batch_id TEXT,
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS generation_metadata (
    asset_id TEXT PRIMARY KEY,
    prompt TEXT NOT NULL,
    negative_prompt TEXT,
    mode TEXT NOT NULL,
    quality TEXT,
    aspect_ratio TEXT,
    count INTEGER,
    image_index INTEGER,
    provider_instance_id TEXT,
    provider_id TEXT,
    provider_name TEXT,
    model TEXT,
    endpoint TEXT,
    template_file TEXT,
    reference_images_json TEXT NOT NULL DEFAULT '[]',
    provider_metadata_json TEXT NOT NULL DEFAULT '{}',
    request_metadata_json TEXT NOT NULL DEFAULT '{}',
    FOREIGN KEY(asset_id) REFERENCES assets(id)
);
CREATE INDEX IF NOT EXISTS idx_assets_abs_path ON assets(abs_path);
CREATE INDEX IF NOT EXISTS idx_assets_portable_rel ON assets(portable_rel_path);
CREATE INDEX IF NOT EXISTS idx_assets_domain_created ON assets(domain, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_generation_quality ON generation_metadata(quality);
CREATE INDEX IF NOT EXISTS idx_generation_aspect ON generation_metadata(aspect_ratio);
"#;

pub(super) fn init_schema(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute(
        "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
        params![SCHEMA_VERSION, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}
