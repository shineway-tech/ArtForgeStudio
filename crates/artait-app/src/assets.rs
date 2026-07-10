//! Asset library 与 Slint 桥接。
//!
//! 设计：
//! - `AssetMeta` 在 tokio 任务里维护（Send）；
//! - 推送到 UI 线程时再调 `Image::load_from_path`（Slint Image 非 Send）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use artait_asset::{AssetEvent, AssetLibrary};
use artait_model::{Asset, AssetDomain, AssetKind};
use slint::{ComponentHandle, Image, ModelRc, VecModel, Weak};
use tokio::sync::mpsc;

use crate::ui::{AppShell, AppState, AssetItem};

#[derive(Debug, Clone)]
struct AssetMeta {
    id: String,
    path: PathBuf,
    name: String,
    domain: AssetDomain,
    is_video: bool,
    bytes: u64,
    mtime: SystemTime,
    prompt: String,
    quality: String,
    aspect_ratio: String,
    model: String,
    width: i32,
    height: i32,
}

impl AssetMeta {
    fn from_asset(a: &Asset) -> Self {
        let bytes = std::fs::metadata(&a.path).map(|m| m.len()).unwrap_or(0);
        let mtime = std::fs::metadata(&a.path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let name = a
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        Self {
            id: a.id.clone(),
            path: a.path.clone(),
            name,
            domain: a.domain,
            is_video: matches!(a.kind, AssetKind::Video),
            bytes,
            mtime,
            prompt: a.prompt.clone().unwrap_or_default(),
            quality: a.quality.clone().unwrap_or_default(),
            aspect_ratio: a.aspect_ratio.clone().unwrap_or_default(),
            model: a.model.clone().unwrap_or_default(),
            width: a.width.unwrap_or_default().min(i32::MAX as u32) as i32,
            height: a.height.unwrap_or_default().min(i32::MAX as u32) as i32,
        }
    }
}

type AssetMap = BTreeMap<PathBuf, AssetMeta>;

pub fn spawn_asset_bridge(
    rt: &tokio::runtime::Handle,
    output_dir: PathBuf,
    app_weak: Weak<AppShell>,
) -> notify::RecommendedWatcher {
    let (tx, mut rx) = mpsc::unbounded_channel::<AssetEvent>();

    let library = AssetLibrary::new(&output_dir);
    let watcher = library.spawn_watcher(tx).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "AssetLibrary watcher 启动失败，使用 noop");
        notify::recommended_watcher(|_| {}).expect("dummy watcher")
    });

    let app_weak_for_loop = app_weak;
    rt.spawn(async move {
        let map: Arc<tokio::sync::Mutex<AssetMap>> =
            Arc::new(tokio::sync::Mutex::new(AssetMap::new()));
        while let Some(ev) = rx.recv().await {
            match ev {
                AssetEvent::InitialScanChunk(assets) => {
                    let mut m = map.lock().await;
                    for a in assets {
                        let meta = AssetMeta::from_asset(&a);
                        m.insert(meta.path.clone(), meta);
                    }
                    // 分块阶段不推 UI — 等 InitialScanDone 一次性推送
                }
                AssetEvent::InitialScanDone => {
                    let m = map.lock().await;
                    push_to_ui(&m, app_weak_for_loop.clone());
                }
                AssetEvent::Upserted(a) => {
                    let meta = AssetMeta::from_asset(&a);
                    let mut m = map.lock().await;
                    m.insert(meta.path.clone(), meta);
                    push_to_ui(&m, app_weak_for_loop.clone());
                }
                AssetEvent::Removed(p) => {
                    let mut m = map.lock().await;
                    m.remove(&p);
                    push_to_ui(&m, app_weak_for_loop.clone());
                }
            }
        }
    });

    watcher
}

pub fn refresh_once(output_dir: PathBuf, app_weak: Weak<AppShell>) {
    std::thread::spawn(move || {
        let library = AssetLibrary::new(&output_dir);
        match library.scan() {
            Ok(iter) => {
                let mut map = AssetMap::new();
                for asset in iter {
                    let meta = AssetMeta::from_asset(&asset);
                    map.insert(meta.path.clone(), meta);
                }
                push_to_ui(&map, app_weak);
            }
            Err(e) => {
                tracing::warn!(error = %e, output_dir = %output_dir.display(), "资产刷新失败");
            }
        }
    });
}

fn push_to_ui(map: &AssetMap, app_weak: Weak<AppShell>) {
    let mut metas: Vec<AssetMeta> = map.values().cloned().collect();
    metas.sort_by(|a, b| b.mtime.cmp(&a.mtime));

    // 在 UI 线程加载 Image（非 Send）
    let _ = slint::invoke_from_event_loop(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let s = app.global::<AppState>();

        let mut all = Vec::with_capacity(metas.len());
        let mut scene = Vec::new();
        let mut character = Vec::new();
        let mut ui_v = Vec::new();
        let mut effect = Vec::new();
        let mut storyboard = Vec::new();

        for m in metas {
            let item = meta_to_item(&m);
            match m.domain {
                AssetDomain::Scene | AssetDomain::AnimationScene => scene.push(item.clone()),
                AssetDomain::Character
                | AssetDomain::AnimationCharacter
                | AssetDomain::CharacterTurnaround => character.push(item.clone()),
                AssetDomain::Ui => ui_v.push(item.clone()),
                AssetDomain::Effect => effect.push(item.clone()),
                AssetDomain::Storyboard => storyboard.push(item.clone()),
                _ => {}
            }
            all.push(item);
        }

        s.set_assets_all(ModelRc::new(VecModel::from(all)));
        s.set_assets_scene(ModelRc::new(VecModel::from(scene)));
        s.set_assets_character(ModelRc::new(VecModel::from(character)));
        s.set_assets_ui(ModelRc::new(VecModel::from(ui_v)));
        s.set_assets_effect(ModelRc::new(VecModel::from(effect)));
        s.set_assets_storyboard(ModelRc::new(VecModel::from(storyboard)));
    });
}

fn meta_to_item(m: &AssetMeta) -> AssetItem {
    let img = if !m.is_video {
        let thumb = artait_asset::ensure_thumbnail(&m.path);
        Image::load_from_path(&thumb).unwrap_or_default()
    } else {
        Image::default()
    };
    AssetItem {
        id: m.id.clone().into(),
        path: m.path.display().to_string().into(),
        name: m.name.clone().into(),
        domain: domain_str(m.domain).into(),
        is_video: m.is_video,
        selected: false,
        bytes: m.bytes.min(i32::MAX as u64) as i32,
        prompt: m.prompt.clone().into(),
        quality: m.quality.clone().into(),
        aspect_ratio: m.aspect_ratio.clone().into(),
        model: m.model.clone().into(),
        width: m.width,
        height: m.height,
        thumb: img,
    }
}

fn domain_str(d: AssetDomain) -> &'static str {
    match d {
        AssetDomain::Scene => "scene",
        AssetDomain::Character => "character",
        AssetDomain::Ui => "ui",
        AssetDomain::Effect => "effect",
        AssetDomain::AnimationScene => "animation_scene",
        AssetDomain::AnimationCharacter => "animation_character",
        AssetDomain::CharacterTurnaround => "character_turnaround",
        AssetDomain::Storyboard => "storyboard",
        AssetDomain::ActionSequence => "action_sequence",
        AssetDomain::AnimationScript => "animation_script",
    }
}

pub fn reveal_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe")
            .arg("/select,")
            .arg(path)
            .spawn();
    }
}

pub fn open_with_default(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", path.to_string_lossy().as_ref()])
            .spawn();
    }
}

/// 重启资产监听（输出目录变更时调用）。
/// 停止旧 watcher → 起新 watcher → 刷新一次。
#[allow(dead_code)]
pub fn restart_asset_watcher(
    slot: &std::rc::Rc<std::cell::RefCell<Option<notify::RecommendedWatcher>>>,
    rt_handle: &tokio::runtime::Handle,
    output_dir: std::path::PathBuf,
    app_weak: slint::Weak<crate::ui::AppShell>,
) {
    let old = slot.borrow_mut().take();
    drop(old); // 停止旧监听

    let watcher = spawn_asset_bridge(rt_handle, output_dir.clone(), app_weak.clone());
    slot.borrow_mut().replace(watcher);

    refresh_once(output_dir, app_weak);
}
