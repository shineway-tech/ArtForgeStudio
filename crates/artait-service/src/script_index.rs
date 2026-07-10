//! 剧本文件索引与解析缓存。
//!
//! Markdown 仍然是正文源文件；SQLite 只存轻量列表信息和解析结果缓存。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::script::{
    ScriptCharacterSummary, ScriptParseReport, ScriptSceneSummary, ScriptStructureSummary,
};

const DB_REL_PATH: &[&str] = &["index", "script_index.sqlite"];

#[derive(Debug, Clone)]
pub struct ScriptIndexStore {
    db_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ScriptIndexEntry {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub modified_unix: i64,
}

#[derive(Debug, Clone)]
pub struct CachedScriptAnalysis {
    pub report: ScriptParseReport,
    pub structure: ScriptStructureSummary,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedStructureFile {
    scenes: Vec<ScriptSceneSummary>,
    characters: Vec<ScriptCharacterSummary>,
}

impl ScriptIndexStore {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let db_path = db_path(data_dir);
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建剧本索引目录失败: {}", parent.display()))?;
        }
        let store = Self { db_path };
        store.with_conn(init_schema)?;
        Ok(store)
    }

    pub fn default() -> Result<Self> {
        Self::open(&artait_model::portable_data_dir())
    }

    pub fn sync_dir(&self, dir: &Path) -> Result<Vec<ScriptIndexEntry>> {
        let scanned = scan_scripts(dir);
        self.with_conn(|conn| {
            let now = Utc::now().to_rfc3339();
            let dir_prefix = dir.display().to_string();
            conn.execute(
                "UPDATE scripts SET deleted = 1 WHERE path LIKE ?1",
                params![format!("{dir_prefix}%")],
            )?;
            for entry in &scanned {
                conn.execute(
                    "
                    INSERT INTO scripts(path, name, bytes, modified_unix, indexed_at, deleted)
                    VALUES (?1, ?2, ?3, ?4, ?5, 0)
                    ON CONFLICT(path) DO UPDATE SET
                        name = excluded.name,
                        bytes = excluded.bytes,
                        modified_unix = excluded.modified_unix,
                        indexed_at = excluded.indexed_at,
                        deleted = 0
                    ",
                    params![
                        entry.path.display().to_string(),
                        entry.name,
                        u64_to_i64(entry.bytes),
                        entry.modified_unix,
                        now,
                    ],
                )?;
            }
            Ok(())
        })?;
        self.recent(200)
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<ScriptIndexEntry>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "
                SELECT path, name, bytes, modified_unix
                FROM scripts
                WHERE deleted = 0
                ORDER BY modified_unix DESC, indexed_at DESC
                LIMIT ?1
                ",
            )?;
            let rows = stmt.query_map(params![usize_to_i64(limit)], |row| {
                Ok(ScriptIndexEntry {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    name: row.get(1)?,
                    bytes: i64_to_u64(row.get(2)?),
                    modified_unix: row.get(3)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn cached_analysis(&self, path: &Path) -> Result<Option<CachedScriptAnalysis>> {
        let fingerprint = match file_fingerprint(path) {
            Some(value) => value,
            None => return Ok(None),
        };
        self.with_conn(|conn| {
            let row = conn
                .query_row(
                    "
                    SELECT episode_count, scene_count, character_count, dialogue_count,
                           parse_summary, structure_json
                    FROM scripts
                    WHERE path = ?1 AND bytes = ?2 AND modified_unix = ?3 AND parse_status = 'ready'
                    LIMIT 1
                    ",
                    params![
                        path.display().to_string(),
                        u64_to_i64(fingerprint.bytes),
                        fingerprint.modified_unix,
                    ],
                    |row| {
                        let structure_json: String = row.get(5)?;
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, String>(4)?,
                            structure_json,
                        ))
                    },
                )
                .optional()?;

            let Some((episodes, scenes, characters, dialogues, summary, structure_json)) = row
            else {
                return Ok(None);
            };
            let structure_file = serde_json::from_str::<CachedStructureFile>(&structure_json)
                .unwrap_or(CachedStructureFile {
                    scenes: vec![],
                    characters: vec![],
                });
            Ok(Some(CachedScriptAnalysis {
                report: ScriptParseReport {
                    episode_count: i64_to_usize(episodes),
                    scene_count: i64_to_usize(scenes),
                    character_count: i64_to_usize(characters),
                    dialogue_count: i64_to_usize(dialogues),
                    summary,
                },
                structure: ScriptStructureSummary {
                    scenes: structure_file.scenes,
                    characters: structure_file.characters,
                },
            }))
        })
    }

    pub fn upsert_analysis(
        &self,
        path: &Path,
        report: &ScriptParseReport,
        structure: &ScriptStructureSummary,
    ) -> Result<()> {
        let Some(fingerprint) = file_fingerprint(path) else {
            return Ok(());
        };
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let structure_json = serde_json::to_string(&CachedStructureFile {
            scenes: structure.scenes.clone(),
            characters: structure.characters.clone(),
        })?;
        self.with_conn(|conn| {
            conn.execute(
                "
                INSERT INTO scripts(
                    path, name, bytes, modified_unix, indexed_at, parse_status,
                    episode_count, scene_count, character_count, dialogue_count,
                    parse_summary, structure_json, parsed_at, deleted
                )
                VALUES (?1, ?2, ?3, ?4, ?5, 'ready', ?6, ?7, ?8, ?9, ?10, ?11, ?5, 0)
                ON CONFLICT(path) DO UPDATE SET
                    name = excluded.name,
                    bytes = excluded.bytes,
                    modified_unix = excluded.modified_unix,
                    indexed_at = excluded.indexed_at,
                    parse_status = 'ready',
                    episode_count = excluded.episode_count,
                    scene_count = excluded.scene_count,
                    character_count = excluded.character_count,
                    dialogue_count = excluded.dialogue_count,
                    parse_summary = excluded.parse_summary,
                    structure_json = excluded.structure_json,
                    parsed_at = excluded.parsed_at,
                    deleted = 0
                ",
                params![
                    path.display().to_string(),
                    name,
                    u64_to_i64(fingerprint.bytes),
                    fingerprint.modified_unix,
                    Utc::now().to_rfc3339(),
                    usize_to_i64(report.episode_count),
                    usize_to_i64(report.scene_count),
                    usize_to_i64(report.character_count),
                    usize_to_i64(report.dialogue_count),
                    report.summary,
                    structure_json,
                ],
            )?;
            Ok(())
        })
    }

    pub fn mark_parse_error(&self, path: &Path, error: &str) -> Result<()> {
        let Some(fingerprint) = file_fingerprint(path) else {
            return Ok(());
        };
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        self.with_conn(|conn| {
            conn.execute(
                "
                INSERT INTO scripts(path, name, bytes, modified_unix, indexed_at, parse_status, parse_summary, deleted)
                VALUES (?1, ?2, ?3, ?4, ?5, 'error', ?6, 0)
                ON CONFLICT(path) DO UPDATE SET
                    name = excluded.name,
                    bytes = excluded.bytes,
                    modified_unix = excluded.modified_unix,
                    indexed_at = excluded.indexed_at,
                    parse_status = 'error',
                    parse_summary = excluded.parse_summary,
                    deleted = 0
                ",
                params![
                    path.display().to_string(),
                    name,
                    u64_to_i64(fingerprint.bytes),
                    fingerprint.modified_unix,
                    Utc::now().to_rfc3339(),
                    error,
                ],
            )?;
            Ok(())
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = Connection::open(&self.db_path)
            .with_context(|| format!("打开剧本索引失败: {}", self.db_path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        f(&conn)
    }
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS scripts (
            path TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            bytes INTEGER NOT NULL,
            modified_unix INTEGER NOT NULL,
            indexed_at TEXT NOT NULL,
            parse_status TEXT NOT NULL DEFAULT 'idle',
            episode_count INTEGER NOT NULL DEFAULT 0,
            scene_count INTEGER NOT NULL DEFAULT 0,
            character_count INTEGER NOT NULL DEFAULT 0,
            dialogue_count INTEGER NOT NULL DEFAULT 0,
            parse_summary TEXT NOT NULL DEFAULT '',
            structure_json TEXT NOT NULL DEFAULT '{\"scenes\":[],\"characters\":[]}',
            parsed_at TEXT,
            deleted INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_scripts_recent ON scripts(deleted, modified_unix DESC);
        CREATE INDEX IF NOT EXISTS idx_scripts_parse ON scripts(path, bytes, modified_unix, parse_status);
        ",
    )?;
    Ok(())
}

fn db_path(data_dir: &Path) -> PathBuf {
    DB_REL_PATH
        .iter()
        .fold(data_dir.to_path_buf(), |path, part| path.join(part))
}

fn scan_scripts(dir: &Path) -> Vec<ScriptIndexEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<ScriptIndexEntry> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("md"))
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            let modified_unix = system_time_to_unix(meta.modified().ok()?);
            let name = path.file_name()?.to_str()?.to_string();
            Some(ScriptIndexEntry {
                path,
                name,
                bytes: meta.len(),
                modified_unix,
            })
        })
        .collect();
    items.sort_by(|a, b| b.modified_unix.cmp(&a.modified_unix));
    items
}

#[derive(Debug, Clone, Copy)]
struct FileFingerprint {
    bytes: u64,
    modified_unix: i64,
}

fn file_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileFingerprint {
        bytes: meta.len(),
        modified_unix: system_time_to_unix(meta.modified().ok()?),
    })
}

fn system_time_to_unix(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn usize_to_i64(value: usize) -> i64 {
    value.min(i64::MAX as usize) as i64
}

fn i64_to_usize(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(0)
}

fn i64_to_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_dir_indexes_markdown_files() {
        let dir = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "# A").unwrap();
        std::fs::write(dir.path().join("b.txt"), "skip").unwrap();

        let store = ScriptIndexStore::open(data.path()).unwrap();
        let entries = store.sync_dir(dir.path()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a.md");
    }

    #[test]
    fn caches_analysis_for_unchanged_file() {
        let dir = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.md");
        std::fs::write(&path, "**第一集：测试**\n\n**1-1 日 内 客厅**\n张三：你好").unwrap();

        let store = ScriptIndexStore::open(data.path()).unwrap();
        let report = ScriptParseReport {
            episode_count: 1,
            scene_count: 1,
            character_count: 1,
            dialogue_count: 1,
            summary: "解析完成".into(),
        };
        let structure = ScriptStructureSummary {
            scenes: vec![ScriptSceneSummary {
                id: "s1".into(),
                episode: "第一集".into(),
                label: "1-1".into(),
                characters: "张三".into(),
                action_preview: "你好".into(),
                dialogue_count: 1,
            }],
            characters: vec![],
        };

        store.upsert_analysis(&path, &report, &structure).unwrap();
        let cached = store.cached_analysis(&path).unwrap().unwrap();

        assert_eq!(cached.report.scene_count, 1);
        assert_eq!(cached.structure.scenes.len(), 1);
    }
}
