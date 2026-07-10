//! ArtAIT 本地资产懒索引、文件监听。
//!
//! MVP 行为：
//! - 启动只扫元数据（路径、kind、mtime、size）；
//! - notify 监听 `out/` 增量变更；
//! - 通过 mpsc 把变更事件推给上层。
//!
//! 缩略图缓存：首次加载时用 image crate 重采样到 256px 并缓存到
//! 绿色版 `data/cache/thumbnails/`，后续直接从缓存加载。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use artait_model::{Asset, AssetDomain, AssetKind};
use chrono::{DateTime, Utc};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

pub mod metadata;
mod metadata_schema;
mod metadata_write;
pub mod postprocess;
pub mod thumbnail;

pub use metadata::{AssetMetadataStore, GeneratedAssetMetadata, StoredAssetMetadata};
pub use postprocess::{unmult_to_sibling, PostprocessError, PostprocessResult};
pub use thumbnail::ensure as ensure_thumbnail;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];
const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mov", "mkv"];

#[derive(Debug, Clone)]
pub enum AssetEvent {
    /// 初始扫描的一批资产（分块发送，避免一次性分配大 Vec）。
    InitialScanChunk(Vec<Asset>),
    /// 初始扫描完成。
    InitialScanDone,
    /// 单个资产新增 / 修改。
    Upserted(Asset),
    /// 单个资产被删除。
    Removed(PathBuf),
}

#[derive(Debug, Clone, Default)]
pub struct AssetLibrary {
    root: PathBuf,
}

impl AssetLibrary {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 同步全量扫描，返回惰性迭代器（不一次性分配 Vec）。
    /// 对 10w+ 文件场景友好：每次 next() 只读取一个文件。
    pub fn scan(&self) -> Result<AssetIterator> {
        if !self.root.exists() {
            return Ok(AssetIterator::empty());
        }
        let store = AssetMetadataStore::default().ok();
        Ok(AssetIterator::new(self.root.clone(), store))
    }

    /// 启动监听。
    /// `tx` 接收变更事件；watcher 句柄需要保活在调用方。
    pub fn spawn_watcher(
        &self,
        tx: mpsc::UnboundedSender<AssetEvent>,
    ) -> Result<notify::RecommendedWatcher> {
        let root = self.root.clone();
        std::fs::create_dir_all(&root).ok();

        let inflight = Arc::new(Mutex::new(()));
        let inflight_clone = inflight.clone();
        let root_for_watcher = root.clone();
        let tx_for_watcher = tx.clone();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            let _g = inflight_clone.lock().ok();
            for path in event.paths {
                handle_path_event(&root_for_watcher, &path, &event.kind, &tx_for_watcher);
            }
        })?;

        watcher.watch(&root, RecursiveMode::Recursive)?;
        tracing::info!("AssetLibrary watching {}", root.display());

        // 初始全量扫描：惰性迭代 + 分块发送，避免一次性分配大 Vec。
        // 10w+ 文件时每个 chunk 只持有 500 个 Asset 在内存中。
        let root_for_scan = root.clone();
        std::thread::spawn(move || {
            const CHUNK_SIZE: usize = 500;
            let library = AssetLibrary::new(root_for_scan);
            let Ok(iter) = library.scan() else {
                let _ = tx.send(AssetEvent::InitialScanDone);
                return;
            };
            let mut chunk = Vec::with_capacity(CHUNK_SIZE);
            for asset in iter {
                chunk.push(asset);
                if chunk.len() >= CHUNK_SIZE {
                    let batch = std::mem::replace(&mut chunk, Vec::with_capacity(CHUNK_SIZE));
                    let _ = tx.send(AssetEvent::InitialScanChunk(batch));
                }
            }
            if !chunk.is_empty() {
                let _ = tx.send(AssetEvent::InitialScanChunk(chunk));
            }
            let _ = tx.send(AssetEvent::InitialScanDone);
        });

        Ok(watcher)
    }
}

fn handle_path_event(
    root: &Path,
    path: &Path,
    kind: &EventKind,
    tx: &mpsc::UnboundedSender<AssetEvent>,
) {
    if !is_relevant(path) {
        return;
    }
    match kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            let store = AssetMetadataStore::default().ok();
            if let Some(asset) = path_to_asset(root, path, store.as_ref()) {
                let _ = tx.send(AssetEvent::Upserted(asset));
            }
        }
        EventKind::Remove(_) => {
            let _ = tx.send(AssetEvent::Removed(path.to_path_buf()));
        }
        _ => {}
    }
}

/// 资产目录惰性迭代器。
///
/// 深度优先遍历，每次 `next()` 只读一个目录条目。
/// 跳过隐藏文件/目录（`.` 开头、Thumbs.db、desktop.ini 等）；
/// 跳过非图片/视频文件；子目录递归入栈。
pub struct AssetIterator {
    store: Option<AssetMetadataStore>,
    stack: Vec<PathBuf>,
    root: PathBuf,
}

impl AssetIterator {
    fn new(root: PathBuf, store: Option<AssetMetadataStore>) -> Self {
        let stack = vec![root.clone()];
        Self { store, stack, root }
    }

    fn empty() -> Self {
        Self {
            store: None,
            stack: Vec::new(),
            root: PathBuf::new(),
        }
    }
}

impl Iterator for AssetIterator {
    type Item = Asset;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let path = self.stack.pop()?;
            if path.is_dir() {
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        // 跳过隐藏/噪音条目，子目录和文件入栈（逆序保证字母序）
                        let mut children: Vec<PathBuf> = entries
                            .flatten()
                            .map(|e| e.path())
                            .filter(|p| !is_skip_entry(p))
                            .collect();
                        children.sort();
                        for child in children.into_iter().rev() {
                            self.stack.push(child);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, dir = %path.display(), "skip dir");
                    }
                }
            } else if is_relevant(&path) {
                return path_to_asset(&self.root, &path, self.store.as_ref());
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}

/// 应跳过的隐藏/系统/噪音文件。
fn is_skip_entry(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return true; // 无文件名 → 跳过
    };
    // 点开头 = 隐藏（.git, .DS_Store, .thumbnails 等）
    if name.starts_with('.') {
        return true;
    }
    // Windows 系统文件
    let lower = name.to_ascii_lowercase();
    matches!(lower.as_str(), "thumbs.db" | "desktop.ini" | "ehthumbs.db")
}

fn is_relevant(path: &Path) -> bool {
    let Some(ext) = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
    else {
        return false;
    };
    IMAGE_EXTS.contains(&ext.as_str()) || VIDEO_EXTS.contains(&ext.as_str())
}

fn path_to_asset(root: &Path, path: &Path, store: Option<&AssetMetadataStore>) -> Option<Asset> {
    let meta = path.metadata().ok()?;
    let created_at: DateTime<Utc> = meta
        .modified()
        .ok()
        .map(|t| t.into())
        .unwrap_or_else(Utc::now);

    let rel = path.strip_prefix(root).unwrap_or(path);
    let domain = guess_domain(rel);
    let kind = guess_kind(path);
    let stored = store.and_then(|s| s.find_by_path(path).ok().flatten());

    Some(Asset {
        id: path.display().to_string(),
        path: path.to_path_buf(),
        kind,
        domain,
        created_at,
        width: stored.as_ref().and_then(|m| m.width),
        height: stored.as_ref().and_then(|m| m.height),
        duration_secs: None,
        source_task_id: stored.as_ref().and_then(|m| m.source_task_id.clone()),
        prompt: stored.as_ref().and_then(|m| m.prompt.clone()),
        quality: stored.as_ref().and_then(|m| m.quality.clone()),
        aspect_ratio: stored.as_ref().and_then(|m| m.aspect_ratio.clone()),
        provider_id: stored.as_ref().and_then(|m| m.provider_id.clone()),
        model: stored.as_ref().and_then(|m| m.model.clone()),
        tags: Vec::new(),
    })
}

fn guess_kind(path: &Path) -> AssetKind {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    if VIDEO_EXTS.contains(&ext.as_str()) {
        AssetKind::Video
    } else {
        AssetKind::Image
    }
}

fn guess_domain(rel: &Path) -> AssetDomain {
    let first = rel
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match first.as_str() {
        "scenes" => AssetDomain::Scene,
        "creations" => AssetDomain::Character,
        "ui" => AssetDomain::Ui,
        "effects" => AssetDomain::Effect,
        "animation_scenes" => AssetDomain::AnimationScene,
        "animation_characters" => AssetDomain::AnimationCharacter,
        "character_turnarounds" => AssetDomain::CharacterTurnaround,
        "storyboards" => AssetDomain::Storyboard,
        "animation_scripts" => AssetDomain::AnimationScript,
        _ => AssetDomain::Scene,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_empty_dir_returns_nothing() {
        let dir = std::env::temp_dir().join("artait-asset-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib = AssetLibrary::new(&dir);
        assert_eq!(lib.scan().unwrap().count(), 0);
    }

    #[test]
    fn scan_finds_images_recursively() {
        let dir = std::env::temp_dir().join("artait-asset-scan");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::create_dir_all(dir.join("creations")).unwrap();
        std::fs::write(dir.join("scenes").join("a.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(dir.join("creations").join("b.jpg"), b"\xff\xd8\xff").unwrap();
        std::fs::write(dir.join("readme.txt"), b"ignore me").unwrap();

        let lib = AssetLibrary::new(&dir);
        let mut assets: Vec<Asset> = lib.scan().unwrap().collect();
        assets.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(assets.len(), 2);
        // creations/b.jpg 排在 scenes/a.png 前面（字典序）
        assert!(assets[0].path.ends_with("b.jpg"));
        assert_eq!(assets[0].kind, AssetKind::Image);
        assert_eq!(assets[0].domain, AssetDomain::Character);
        assert!(assets[1].path.ends_with("a.png"));
        assert_eq!(assets[1].domain, AssetDomain::Scene);
    }

    #[test]
    fn scan_skips_hidden_files_and_dirs() {
        let dir = std::env::temp_dir().join("artait-asset-hidden");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(dir.join(".git").join("config.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(dir.join(".DS_Store"), b"ignore").unwrap();
        std::fs::write(dir.join("Thumbs.db"), b"ignore").unwrap();
        std::fs::write(dir.join("desktop.ini"), b"ignore").unwrap();
        std::fs::write(dir.join("scenes").join("real.png"), b"\x89PNG\r\n\x1a\n").unwrap();

        let lib = AssetLibrary::new(&dir);
        let assets: Vec<Asset> = lib.scan().unwrap().collect();
        assert_eq!(assets.len(), 1, "只有 scenes/real.png 应该被扫描");
        assert!(assets[0].path.ends_with("real.png"));
    }
}
