//! 场景库存储服务。
//!
//! JSON 文件持久化，管理场景 CRUD、文件夹、搜索筛选。
//! 模式与 character_store 一致。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use artait_model::scene::{Scene, SceneFolder};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SceneLibraryFile {
    version: u32,
    scenes: Vec<Scene>,
    folders: Vec<SceneFolder>,
}

pub struct SceneStore {
    path: PathBuf,
    scenes: Vec<Scene>,
    folders: Vec<SceneFolder>,
    index: HashMap<String, usize>,
    folder_index: HashMap<String, usize>,
    dirty: bool,
}

impl SceneStore {
    pub fn load_or_default() -> Self {
        let path = artait_model::portable_data_dir()
            .join("scenes")
            .join("scene_library.json");
        let mut store = Self::empty_at(path.clone());
        store.reload();
        store
    }

    pub fn empty_at(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        Self {
            path,
            scenes: vec![],
            folders: vec![],
            index: HashMap::new(),
            folder_index: HashMap::new(),
            dirty: false,
        }
    }

    fn reload(&mut self) {
        match fs::read_to_string(&self.path) {
            Ok(content) => match serde_json::from_str::<SceneLibraryFile>(&content) {
                Ok(file) => {
                    self.scenes = file.scenes;
                    self.folders = file.folders;
                    self.rebuild_index();
                    info!(scenes = self.scenes.len(), "加载场景库");
                }
                Err(e) => {
                    warn!(error = %e, "解析场景库失败");
                    self.scenes.clear();
                    self.folders.clear();
                    self.rebuild_index();
                }
            },
            Err(_) => {}
        }
        self.dirty = false;
    }

    pub fn reload_from_disk(&mut self) {
        self.reload();
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        for (i, s) in self.scenes.iter().enumerate() {
            self.index.insert(s.id.clone(), i);
        }
        self.folder_index.clear();
        for (i, f) in self.folders.iter().enumerate() {
            self.folder_index.insert(f.id.clone(), i);
        }
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = SceneLibraryFile {
            version: 1,
            scenes: self.scenes.clone(),
            folders: self.folders.clone(),
        };
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(&file) {
            if fs::write(&tmp, &json).is_ok() {
                if fs::rename(&tmp, &self.path).is_err() {
                    let _ = fs::remove_file(&tmp);
                }
            }
        }
        self.dirty = false;
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    // CRUD
    pub fn create_scene(&mut self, mut scene: Scene) -> Result<String> {
        if scene.id.is_empty() {
            scene.id = Uuid::new_v4().to_string();
        }
        let now = Utc::now();
        scene.created_at = now;
        scene.updated_at = now;
        let id = scene.id.clone();
        self.index.insert(id.clone(), self.scenes.len());
        self.scenes.push(scene);
        self.mark_dirty();
        Ok(id)
    }

    pub fn update_scene(&mut self, id: &str, updater: impl FnOnce(&mut Scene)) -> Result<()> {
        let idx = *self.index.get(id).context("场景不存在")?;
        updater(&mut self.scenes[idx]);
        self.scenes[idx].updated_at = Utc::now();
        self.mark_dirty();
        Ok(())
    }

    pub fn delete_scene(&mut self, id: &str) -> Result<()> {
        let idx = *self.index.get(id).context("场景不存在")?;
        self.scenes.remove(idx);
        self.rebuild_index();
        self.mark_dirty();
        Ok(())
    }

    pub fn get_scene(&self, id: &str) -> Option<&Scene> {
        self.index.get(id).map(|&i| &self.scenes[i])
    }

    pub fn all_scenes(&self) -> &[Scene] {
        &self.scenes
    }

    // 查询
    pub fn search(&self, query: &str) -> Vec<&Scene> {
        if query.is_empty() {
            return self.scenes.iter().collect();
        }
        let lower = query.to_lowercase();
        self.scenes
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&lower)
                    || s.location.to_lowercase().contains(&lower)
                    || s.tags.iter().any(|t| t.to_lowercase().contains(&lower))
            })
            .collect()
    }

    pub fn filter_by_project(&self, pid: &str) -> Vec<&Scene> {
        self.scenes
            .iter()
            .filter(|s| s.project_id.as_deref() == Some(pid))
            .collect()
    }

    pub fn filter_by_folder(&self, fid: Option<&str>) -> Vec<&Scene> {
        self.scenes
            .iter()
            .filter(|s| s.folder_id.as_deref() == fid)
            .collect()
    }

    // 文件夹
    pub fn create_folder(&mut self, name: String, project_id: Option<String>) -> Result<String> {
        let folder = SceneFolder {
            id: Uuid::new_v4().to_string(),
            name,
            parent_id: None,
            project_id,
            is_auto_created: false,
            created_at: Utc::now(),
        };
        let id = folder.id.clone();
        self.folders.push(folder);
        self.folder_index.insert(id.clone(), self.folders.len() - 1);
        self.mark_dirty();
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SceneStore {
        let dir = std::env::temp_dir()
            .join("artait-scene-store")
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        SceneStore::empty_at(dir.join("test.json"))
    }

    #[test]
    fn create_and_get() {
        let mut store = test_store();
        let id = store
            .create_scene(Scene::new("".into(), "大厅".into()))
            .unwrap();
        assert!(store.get_scene(&id).is_some());
    }

    #[test]
    fn update_changes_fields() {
        let mut store = test_store();
        let id = store
            .create_scene(Scene::new("".into(), "酒馆".into()))
            .unwrap();
        store
            .update_scene(&id, |s| {
                s.location = "古代酒馆".into();
                s.time_of_day = Some("夜".into());
            })
            .unwrap();
        let s = store.get_scene(&id).unwrap();
        assert_eq!(s.location, "古代酒馆");
        assert_eq!(s.time_of_day.as_deref(), Some("夜"));
    }

    #[test]
    fn search_finds_by_location() {
        let mut store = test_store();
        let mut s = Scene::new("".into(), "酒馆".into());
        s.location = "长安城西酒馆".into();
        store.create_scene(s).unwrap();
        assert_eq!(store.search("长安").len(), 1);
        assert_eq!(store.search("不存在的").len(), 0);
    }

    #[test]
    fn delete_removes() {
        let mut store = test_store();
        let id = store
            .create_scene(Scene::new("".into(), "删除测试".into()))
            .unwrap();
        store.delete_scene(&id).unwrap();
        assert!(store.get_scene(&id).is_none());
    }
}
