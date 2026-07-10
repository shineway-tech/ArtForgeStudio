use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};

use crate::metadata::GeneratedAssetMetadata;

const UPSERT_ASSET_SQL: &str = r#"
INSERT INTO assets (
    id, portable_rel_path, abs_path, file_name, domain, kind, mime, bytes,
    width, height, created_at, modified_at, source_task_id, batch_id, deleted
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0)
ON CONFLICT(id) DO UPDATE SET
    portable_rel_path = excluded.portable_rel_path,
    abs_path = excluded.abs_path,
    file_name = excluded.file_name,
    domain = excluded.domain,
    kind = excluded.kind,
    mime = excluded.mime,
    bytes = excluded.bytes,
    width = excluded.width,
    height = excluded.height,
    modified_at = excluded.modified_at,
    source_task_id = excluded.source_task_id,
    batch_id = excluded.batch_id,
    deleted = 0
"#;

const UPSERT_GENERATION_SQL: &str = r#"
INSERT INTO generation_metadata (
    asset_id, prompt, negative_prompt, mode, quality, aspect_ratio, count, image_index,
    provider_instance_id, provider_id, provider_name, model, endpoint, template_file,
    reference_images_json, provider_metadata_json, request_metadata_json
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
ON CONFLICT(asset_id) DO UPDATE SET
    prompt = excluded.prompt,
    negative_prompt = excluded.negative_prompt,
    mode = excluded.mode,
    quality = excluded.quality,
    aspect_ratio = excluded.aspect_ratio,
    count = excluded.count,
    image_index = excluded.image_index,
    provider_instance_id = excluded.provider_instance_id,
    provider_id = excluded.provider_id,
    provider_name = excluded.provider_name,
    model = excluded.model,
    endpoint = excluded.endpoint,
    template_file = excluded.template_file,
    reference_images_json = excluded.reference_images_json,
    provider_metadata_json = excluded.provider_metadata_json,
    request_metadata_json = excluded.request_metadata_json
"#;

pub(super) fn upsert_generated(
    conn: &Connection,
    data_dir: &Path,
    item: &GeneratedAssetMetadata,
) -> Result<()> {
    let record = AssetRecord::from_item(data_dir, item);
    upsert_asset(conn, item, &record)?;
    upsert_generation(conn, item, &record)?;
    Ok(())
}

fn upsert_asset(
    conn: &Connection,
    item: &GeneratedAssetMetadata,
    record: &AssetRecord,
) -> Result<()> {
    conn.execute(
        UPSERT_ASSET_SQL,
        params![
            record.id,
            record.rel_path,
            record.abs_path,
            record.file_name,
            item.domain,
            item.kind,
            item.mime,
            record.bytes,
            record.width,
            record.height,
            record.now,
            record.now,
            item.source_task_id,
            item.batch_id
        ],
    )?;
    Ok(())
}

fn upsert_generation(
    conn: &Connection,
    item: &GeneratedAssetMetadata,
    record: &AssetRecord,
) -> Result<()> {
    conn.execute(
        UPSERT_GENERATION_SQL,
        params![
            record.id,
            item.prompt,
            item.negative_prompt,
            item.mode,
            item.quality,
            item.aspect_ratio,
            item.count.map(i64::from),
            item.image_index.map(i64::from),
            item.provider_instance_id,
            item.provider_id,
            item.provider_name,
            item.model,
            item.endpoint,
            item.template_file,
            item.reference_images_json,
            item.provider_metadata_json,
            item.request_metadata_json
        ],
    )?;
    Ok(())
}

struct AssetRecord {
    id: String,
    abs_path: String,
    rel_path: Option<String>,
    file_name: String,
    bytes: i64,
    width: Option<i64>,
    height: Option<i64>,
    now: String,
}

impl AssetRecord {
    fn from_item(data_dir: &Path, item: &GeneratedAssetMetadata) -> Self {
        let abs = absolutize(&item.path);
        let dimensions = image::image_dimensions(&abs).ok();
        Self {
            id: abs.display().to_string(),
            abs_path: abs.display().to_string(),
            rel_path: rel_path(&abs, data_dir),
            file_name: file_name(&item.path),
            bytes: clamp_i64(item.bytes),
            width: item
                .width
                .or_else(|| dimensions.map(|(w, _)| w))
                .map(i64::from),
            height: item
                .height
                .or_else(|| dimensions.map(|(_, h)| h))
                .map(i64::from),
            now: Utc::now().to_rfc3339(),
        }
    }
}

fn rel_path(path: &Path, data_dir: &Path) -> Option<String> {
    path.strip_prefix(data_dir)
        .ok()
        .map(|p| p.display().to_string())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

fn absolutize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn clamp_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}
