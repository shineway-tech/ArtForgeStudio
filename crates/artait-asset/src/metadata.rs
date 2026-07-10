//! 中心化资产元数据索引。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::{metadata_schema::init_schema, metadata_write::upsert_generated};

const DB_REL_PATH: &[&str] = &["index", "asset_index.sqlite"];

#[derive(Debug, Clone)]
pub struct AssetMetadataStore {
    db_path: PathBuf,
    data_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct GeneratedAssetMetadata {
    pub path: PathBuf,
    pub domain: String,
    pub kind: String,
    pub mime: String,
    pub bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub source_task_id: Option<String>,
    pub batch_id: Option<String>,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub mode: String,
    pub quality: Option<String>,
    pub aspect_ratio: Option<String>,
    pub count: Option<u32>,
    pub image_index: Option<u32>,
    pub provider_instance_id: Option<String>,
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub template_file: Option<String>,
    pub reference_images_json: String,
    pub provider_metadata_json: String,
    pub request_metadata_json: String,
}

#[derive(Debug, Clone, Default)]
pub struct StoredAssetMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub source_task_id: Option<String>,
    pub prompt: Option<String>,
    pub quality: Option<String>,
    pub aspect_ratio: Option<String>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub request_metadata_json: Option<String>,
}

impl AssetMetadataStore {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let db_path = db_path(data_dir);
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建资产索引目录失败: {}", parent.display()))?;
        }
        let store = Self {
            db_path,
            data_dir: data_dir.to_path_buf(),
        };
        store.with_conn(init_schema)?;
        Ok(store)
    }

    pub fn default() -> Result<Self> {
        Self::open(&artait_model::portable_data_dir())
    }

    pub fn upsert_generated(&self, item: &GeneratedAssetMetadata) -> Result<()> {
        self.with_conn(|conn| upsert_generated(conn, &self.data_dir, item))
    }

    pub fn find_by_path(&self, path: &Path) -> Result<Option<StoredAssetMetadata>> {
        self.with_conn(|conn| find_by_path(conn, path))
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = Connection::open(&self.db_path)
            .with_context(|| format!("打开资产索引失败: {}", self.db_path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        f(&conn)
    }
}

fn db_path(data_dir: &Path) -> PathBuf {
    DB_REL_PATH
        .iter()
        .fold(data_dir.to_path_buf(), |path, part| path.join(part))
}

fn find_by_path(conn: &Connection, path: &Path) -> Result<Option<StoredAssetMetadata>> {
    let abs_path = absolutize(path).display().to_string();
    let sql = "
        SELECT a.width, a.height, a.source_task_id, g.prompt, g.quality,
               g.aspect_ratio, g.provider_id, g.model, g.request_metadata_json
        FROM assets a
        LEFT JOIN generation_metadata g ON g.asset_id = a.id
        WHERE a.abs_path = ?1 AND a.deleted = 0
        LIMIT 1
    ";
    let item = conn
        .query_row(sql, params![abs_path], |row| {
            Ok(StoredAssetMetadata {
                width: row.get::<_, Option<i64>>(0)?.and_then(to_u32),
                height: row.get::<_, Option<i64>>(1)?.and_then(to_u32),
                source_task_id: row.get(2)?,
                prompt: row.get(3)?,
                quality: row.get(4)?,
                aspect_ratio: row.get(5)?,
                provider_id: row.get(6)?,
                model: row.get(7)?,
                request_metadata_json: row.get(8)?,
            })
        })
        .optional()?;
    Ok(item)
}

fn absolutize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn to_u32(value: i64) -> Option<u32> {
    u32::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(path: PathBuf) -> GeneratedAssetMetadata {
        GeneratedAssetMetadata {
            path,
            domain: "scene".into(),
            kind: "image".into(),
            mime: "image/png".into(),
            bytes: 128,
            width: Some(1024),
            height: Some(768),
            source_task_id: Some("task-1".into()),
            batch_id: None,
            prompt: "a calm room".into(),
            negative_prompt: None,
            mode: "scene".into(),
            quality: Some("2K".into()),
            aspect_ratio: Some("16:9".into()),
            count: Some(1),
            image_index: Some(1),
            provider_instance_id: Some("inst".into()),
            provider_id: Some("openai-compatible".into()),
            provider_name: Some("Demo".into()),
            model: Some("demo-model".into()),
            endpoint: Some("https://example.com".into()),
            template_file: None,
            reference_images_json: "[]".into(),
            provider_metadata_json: "{}".into(),
            request_metadata_json: "{}".into(),
        }
    }

    #[test]
    fn metadata_store_creates_schema() {
        let dir = std::env::temp_dir().join("artait-metadata-schema");
        let _ = std::fs::remove_dir_all(&dir);

        let store = AssetMetadataStore::open(&dir).unwrap();

        assert!(store.db_path().exists());
    }

    #[test]
    fn metadata_store_upserts_and_finds_by_path() {
        let dir = std::env::temp_dir().join("artait-metadata-upsert");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let img = dir.join("out").join("a.png");
        std::fs::write(&img, b"fake").unwrap();
        let store = AssetMetadataStore::open(&dir).unwrap();

        store.upsert_generated(&sample(img.clone())).unwrap();
        let found = store.find_by_path(&img).unwrap().unwrap();

        assert_eq!(Some(1024), found.width);
        assert_eq!(Some(768), found.height);
        assert_eq!(Some("a calm room".into()), found.prompt);
        assert_eq!(Some("2K".into()), found.quality);
        assert_eq!(Some("16:9".into()), found.aspect_ratio);
        assert_eq!(Some("demo-model".into()), found.model);
        assert_eq!(Some("{}".into()), found.request_metadata_json);
    }
}
