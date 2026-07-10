//! 参考图上传缓存：基于文件签名（路径+mtime+大小）去重，避免重复上传同一张图。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const UPLOAD_RECORD_FILE: &str = "uploaded_images.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadRecordEntry {
    pub path: String,
    pub mtime: u64,
    pub size: u64,
    pub cache_key: String,
    pub url: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UploadRecords {
    #[serde(default)]
    pub images: BTreeMap<String, UploadRecordEntry>,
}

/// 文件签名（绝对路径 + mtime + 大小）
struct FileUploadSignature {
    abs_path: String,
    mtime: u64,
    size: u64,
}

fn file_signature(file_path: &Path) -> Option<FileUploadSignature> {
    let abs_path = file_path.canonicalize().ok()?;
    let metadata = std::fs::metadata(&abs_path).ok()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(FileUploadSignature {
        abs_path: abs_path.to_string_lossy().to_string(),
        mtime,
        size: metadata.len(),
    })
}

fn record_path(output_dir: &Path) -> PathBuf {
    output_dir.join(UPLOAD_RECORD_FILE)
}

fn load_records(output_dir: &Path) -> UploadRecords {
    let path = record_path(output_dir);
    if !path.exists() {
        return UploadRecords::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_records(output_dir: &Path, records: &UploadRecords) {
    let path = record_path(output_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(records) {
        let _ = std::fs::write(&path, text);
    }
}

/// 查询缓存：如果文件已上传且未修改，返回缓存的 URL
pub fn lookup_cached_url(file_path: &Path, output_dir: &Path) -> Option<String> {
    let sig = file_signature(file_path)?;
    let records = load_records(output_dir);
    let entry = records.images.get(&sig.abs_path)?;
    // 验证签名一致（mtime + size）
    if entry.mtime == sig.mtime && entry.size == sig.size {
        let url = entry.url.trim();
        if !url.is_empty() {
            return Some(url.to_owned());
        }
    }
    None
}

/// 保存上传记录
pub fn save_cached_url(file_path: &Path, output_dir: &Path, url: &str, provider: &str) {
    let Some(sig) = file_signature(file_path) else {
        return;
    };
    let key = format!("{}|{}|{}", sig.abs_path, sig.mtime, sig.size);
    let mut records = load_records(output_dir);
    records.images.insert(
        sig.abs_path.clone(),
        UploadRecordEntry {
            path: sig.abs_path,
            mtime: sig.mtime,
            size: sig.size,
            cache_key: key,
            url: url.to_owned(),
            provider: provider.to_owned(),
            updated_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        },
    );
    save_records(output_dir, &records);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn cache_round_trip() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("output");
        fs::create_dir_all(&output).unwrap();
        let file = dir.path().join("image.png");
        fs::write(&file, b"abc").unwrap();

        assert!(lookup_cached_url(&file, &output).is_none());

        save_cached_url(&file, &output, "https://example.com/img.png", "test");
        let url = lookup_cached_url(&file, &output);
        assert_eq!(url.as_deref(), Some("https://example.com/img.png"));

        // 修改文件后缓存失效
        fs::write(&file, b"abcd").unwrap();
        assert!(lookup_cached_url(&file, &output).is_none());
    }
}
