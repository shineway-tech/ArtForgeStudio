//! 角色库存储服务。
//!
//! 以 JSON 文件存储在绿色版 `data/characters/character_library.json`，
//! 管理角色的 CRUD、文件夹组织、搜索筛选、视图/变体更新。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use artait_model::{
    Character, CharacterFolder, CharacterStatus, CharacterVariation, CharacterView,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

// ============================================================================
// 持久化格式
// ============================================================================

/// 角色库持久化文件格式。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CharacterLibraryFile {
    version: u32,
    characters: Vec<Character>,
    folders: Vec<CharacterFolder>,
}

// ============================================================================
// CharacterStore
// ============================================================================

/// 角色库存储（非线程安全，调用方自行加锁）。
///
/// 角色数据按 JSON 文件持久化，支持 CRUD、文件夹、搜索和筛选。
pub struct CharacterStore {
    path: PathBuf,
    characters: Vec<Character>,
    folders: Vec<CharacterFolder>,
    /// id → index 快速查找
    char_index: HashMap<String, usize>,
    folder_index: HashMap<String, usize>,
    dirty: bool,
}

impl CharacterStore {
    // ── 生命周期 ────────────────────────────────────────────────────────

    /// 从默认路径加载角色库（不存在则创建空库）。
    pub fn load_or_default() -> Self {
        let path = Self::default_path();
        let mut store = Self::empty_at(path.clone());
        store.reload();
        store
    }

    /// 使用指定路径创建空库（供测试使用）。
    pub fn empty_at(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        Self {
            path,
            characters: Vec::new(),
            folders: Vec::new(),
            char_index: HashMap::new(),
            folder_index: HashMap::new(),
            dirty: false,
        }
    }

    fn reload(&mut self) {
        match fs::read_to_string(&self.path) {
            Ok(content) => match serde_json::from_str::<CharacterLibraryFile>(&content) {
                Ok(file) => {
                    self.characters = file.characters;
                    self.folders = file.folders;
                    self.rebuild_index();
                    info!(
                        chars = self.characters.len(),
                        folders = self.folders.len(),
                        path = %self.path.display(),
                        "加载角色库"
                    );
                }
                Err(e) => {
                    warn!(error = %e, path = %self.path.display(), "解析角色库失败，重建空库");
                    let backup = self.path.with_extension("json.bak");
                    let _ = fs::copy(&self.path, &backup);
                    self.characters.clear();
                    self.folders.clear();
                    self.rebuild_index();
                }
            },
            Err(_) => {
                info!(path = %self.path.display(), "角色库文件不存在，将创建");
            }
        }
        self.dirty = false;
    }

    fn rebuild_index(&mut self) {
        self.char_index.clear();
        for (i, c) in self.characters.iter().enumerate() {
            self.char_index.insert(c.id.clone(), i);
        }
        self.folder_index.clear();
        for (i, f) in self.folders.iter().enumerate() {
            self.folder_index.insert(f.id.clone(), i);
        }
    }

    /// 将脏数据持久化到磁盘。
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }

        if let Some(parent) = self.path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                warn!(error = %e, path = %parent.display(), "创建角色库目录失败");
                return;
            }
        }

        self.characters
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        let file = CharacterLibraryFile {
            version: 1,
            characters: self.characters.clone(),
            folders: self.folders.clone(),
        };

        let tmp = self.path.with_extension("json.tmp");
        match serde_json::to_string_pretty(&file) {
            Ok(json) => {
                if let Err(e) = fs::write(&tmp, &json) {
                    warn!(error = %e, path = %tmp.display(), "写入临时角色库文件失败");
                    return;
                }
                if let Err(e) = fs::rename(&tmp, &self.path) {
                    warn!(error = %e, path = %self.path.display(), "重命名角色库文件失败");
                    let _ = fs::remove_file(&tmp);
                }
            }
            Err(e) => warn!(error = %e, "序列化角色库失败"),
        }
        self.dirty = false;
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn default_path() -> PathBuf {
        artait_model::portable_data_dir()
            .join("characters")
            .join("character_library.json")
    }

    // ── 角色 CRUD ───────────────────────────────────────────────────────

    /// 创建角色并返回其 ID。
    pub fn create_character(&mut self, mut character: Character) -> Result<String> {
        if character.id.is_empty() {
            character.id = Uuid::new_v4().to_string();
        }
        let now = Utc::now();
        character.created_at = now;
        character.updated_at = now;

        let id = character.id.clone();
        let idx = self.characters.len();
        self.char_index.insert(id.clone(), idx);
        self.characters.push(character);
        self.mark_dirty();
        info!(char_id = %id, "创建角色");
        Ok(id)
    }

    /// 更新已有角色（按 id 匹配，不存在则返回错误）。
    pub fn update_character(
        &mut self,
        id: &str,
        mut updater: impl FnMut(&mut Character),
    ) -> Result<()> {
        let idx = self.char_index.get(id).copied().context("角色不存在")?;
        updater(&mut self.characters[idx]);
        self.characters[idx].updated_at = Utc::now();
        self.mark_dirty();
        Ok(())
    }

    /// 删除角色。
    pub fn delete_character(&mut self, id: &str) -> Result<()> {
        let idx = self.char_index.get(id).copied().context("角色不存在")?;
        self.characters.remove(idx);
        self.rebuild_index();
        self.mark_dirty();
        info!(char_id = %id, "删除角色");
        Ok(())
    }

    /// 获取角色引用。
    pub fn get_character(&self, id: &str) -> Option<&Character> {
        self.char_index.get(id).map(|&idx| &self.characters[idx])
    }

    /// 获取角色可变引用。
    pub fn get_character_mut(&mut self, id: &str) -> Option<&mut Character> {
        self.char_index
            .get(id)
            .map(|&idx| &mut self.characters[idx])
    }

    /// 获取所有角色。
    pub fn all_characters(&self) -> &[Character] {
        &self.characters
    }

    /// 获取角色总数。
    pub fn character_count(&self) -> usize {
        self.characters.len()
    }

    /// 限制角色数量（防止无限膨胀）。
    pub fn trim(&mut self, max_entries: usize) {
        if self.characters.len() > max_entries {
            self.characters
                .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            self.characters.truncate(max_entries);
            self.rebuild_index();
            self.mark_dirty();
        }
    }

    // ── 文件夹 CRUD ─────────────────────────────────────────────────────

    /// 创建文件夹。
    pub fn create_folder(
        &mut self,
        name: String,
        parent_id: Option<String>,
        project_id: Option<String>,
    ) -> Result<String> {
        let folder = CharacterFolder {
            id: Uuid::new_v4().to_string(),
            name,
            parent_id,
            project_id,
            is_auto_created: false,
            created_at: Utc::now(),
        };
        let id = folder.id.clone();
        let idx = self.folders.len();
        self.folder_index.insert(id.clone(), idx);
        self.folders.push(folder);
        self.mark_dirty();
        Ok(id)
    }

    /// 重命名文件夹。
    pub fn rename_folder(&mut self, id: &str, name: &str) -> Result<()> {
        let idx = self.folder_index.get(id).copied().context("文件夹不存在")?;
        self.folders[idx].name = name.to_string();
        self.mark_dirty();
        Ok(())
    }

    /// 删除文件夹（将其下的角色移到根级）。
    pub fn delete_folder(&mut self, id: &str) -> Result<()> {
        let idx = self.folder_index.get(id).copied().context("文件夹不存在")?;
        self.folders.remove(idx);

        // 将删除文件夹下的角色移回根级
        for c in &mut self.characters {
            if c.folder_id.as_deref() == Some(id) {
                c.folder_id = None;
            }
        }

        self.rebuild_index();
        self.mark_dirty();
        Ok(())
    }

    /// 获取所有文件夹。
    pub fn all_folders(&self) -> &[CharacterFolder] {
        &self.folders
    }

    /// 移动角色到指定文件夹。
    pub fn move_to_folder(&mut self, char_id: &str, folder_id: Option<String>) -> Result<()> {
        self.update_character(char_id, |c| {
            c.folder_id = folder_id.clone();
        })
    }

    // ── 视图与变体 ──────────────────────────────────────────────────────

    /// 为角色添加视图。
    pub fn add_view(&mut self, char_id: &str, view: CharacterView) -> Result<()> {
        self.update_character(char_id, |c| {
            c.views.push(view.clone());
            // 首个视图作为缩略图
            if c.thumbnail_url.is_none() {
                c.thumbnail_url = Some(view.image_url.clone());
            }
        })
    }

    /// 为角色添加/更新变体。
    pub fn upsert_variation(&mut self, char_id: &str, variation: CharacterVariation) -> Result<()> {
        let idx = self
            .char_index
            .get(char_id)
            .copied()
            .context("角色不存在")?;
        let c = &mut self.characters[idx];

        if let Some(existing) = c.variations.iter_mut().find(|v| v.id == variation.id) {
            *existing = variation;
        } else {
            c.variations.push(variation);
        }
        c.updated_at = Utc::now();
        self.mark_dirty();
        Ok(())
    }

    /// 删除变体。
    pub fn delete_variation(&mut self, char_id: &str, var_id: &str) -> Result<()> {
        self.update_character(char_id, |c| {
            c.variations.retain(|v| v.id != var_id);
        })
    }

    // ── 查询与筛选 ──────────────────────────────────────────────────────

    /// 按项目筛选角色。
    pub fn filter_by_project(&self, project_id: &str) -> Vec<&Character> {
        self.characters
            .iter()
            .filter(|c| c.project_id.as_deref() == Some(project_id))
            .collect()
    }

    /// 按文件夹筛选角色。
    pub fn filter_by_folder(&self, folder_id: Option<&str>) -> Vec<&Character> {
        self.characters
            .iter()
            .filter(|c| c.folder_id.as_deref() == folder_id)
            .collect()
    }

    /// 按集筛选角色。
    pub fn filter_by_episode(&self, episode_id: Option<&str>) -> Vec<&Character> {
        match episode_id {
            Some(ep) => self
                .characters
                .iter()
                .filter(|c| c.linked_episode_id.as_deref() == Some(ep))
                .collect(),
            None => self.characters.iter().collect(),
        }
    }

    /// 搜索角色（按名称、描述、标签）。
    pub fn search(&self, query: &str) -> Vec<&Character> {
        if query.is_empty() {
            return self.characters.iter().collect();
        }
        let lower = query.to_lowercase();
        self.characters
            .iter()
            .filter(|c| {
                c.name.to_lowercase().contains(&lower)
                    || c.description
                        .as_ref()
                        .map(|d| d.to_lowercase().contains(&lower))
                        .unwrap_or(false)
                    || c.tags.iter().any(|t| t.to_lowercase().contains(&lower))
                    || c.visual_prompt_en
                        .as_ref()
                        .map(|p| p.to_lowercase().contains(&lower))
                        .unwrap_or(false)
                    || c.visual_prompt_zh
                        .as_ref()
                        .map(|p| p.to_lowercase().contains(&lower))
                        .unwrap_or(false)
            })
            .collect()
    }

    /// 按状态筛选角色。
    pub fn filter_by_status(&self, status: CharacterStatus) -> Vec<&Character> {
        self.characters
            .iter()
            .filter(|c| c.status == status)
            .collect()
    }

    /// 获取"未归入任何文件夹"的根级角色。
    pub fn root_characters(&self) -> Vec<&Character> {
        self.characters
            .iter()
            .filter(|c| c.folder_id.is_none())
            .collect()
    }

    // ── 批量操作 ────────────────────────────────────────────────────────

    /// 批量导入角色（从 AI 校准结果）。
    pub fn import_characters(&mut self, characters: Vec<Character>) -> usize {
        let count = characters.len();
        for character in characters {
            // 插入时不触发 flush，最后统一
            let id = character.id.clone();
            let idx = self.characters.len();
            self.char_index.insert(id, idx);
            self.characters.push(character);
        }
        self.mark_dirty();
        info!(count, "批量导入角色");
        count
    }

    /// 获取或创建项目文件夹（自动创建）。
    pub fn ensure_project_folder(
        &mut self,
        project_id: &str,
        project_name: &str,
    ) -> Option<String> {
        // 查找是否已存在
        if let Some(folder) = self
            .folders
            .iter()
            .find(|f| f.project_id.as_deref() == Some(project_id))
        {
            return Some(folder.id.clone());
        }
        // 创建新文件夹
        let id = Uuid::new_v4().to_string();
        let folder = CharacterFolder {
            id: id.clone(),
            name: project_name.to_string(),
            parent_id: None,
            project_id: Some(project_id.to_string()),
            is_auto_created: true,
            created_at: Utc::now(),
        };
        self.folders.push(folder);
        self.folder_index.insert(id.clone(), self.folders.len() - 1);
        self.mark_dirty();
        Some(id)
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_char(id: &str, name: &str) -> Character {
        let mut c = Character::new(id.into(), name.into());
        c.project_id = Some("proj-1".into());
        c
    }

    fn test_store() -> CharacterStore {
        let dir = std::env::temp_dir()
            .join("artait-char-store")
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        CharacterStore::empty_at(dir.join("test_chars.json"))
    }

    #[test]
    fn create_and_get_character() {
        let mut store = test_store();
        let id = store.create_character(make_char("", "云中鹤")).unwrap();
        assert!(!id.is_empty());

        let c = store.get_character(&id).unwrap();
        assert_eq!(c.name, "云中鹤");
    }

    #[test]
    fn update_character_changes_fields() {
        let mut store = test_store();
        let id = store.create_character(make_char("", "李白")).unwrap();

        store
            .update_character(&id, |c| {
                c.gender = Some("男".into());
                c.visual_prompt_en = Some("a tall poet with long black hair".into());
            })
            .unwrap();

        let c = store.get_character(&id).unwrap();
        assert_eq!(c.gender.as_deref(), Some("男"));
        assert!(c.visual_prompt_en.as_ref().unwrap().contains("poet"));
    }

    #[test]
    fn delete_character_removes_from_index() {
        let mut store = test_store();
        let id = store.create_character(make_char("", "删除测试")).unwrap();
        assert!(store.get_character(&id).is_some());

        store.delete_character(&id).unwrap();
        assert!(store.get_character(&id).is_none());
    }

    #[test]
    fn search_finds_by_name_and_tags() {
        let mut store = test_store();
        let mut c = make_char("", "云中鹤");
        c.tags = vec!["#武侠".into(), "#男主".into()];
        c.visual_prompt_en = Some("a swordsman in white".into());
        store.create_character(c).unwrap();

        // 名称匹配
        assert_eq!(store.search("云中").len(), 1);
        // 标签匹配
        assert_eq!(store.search("武侠").len(), 1);
        // 英文 prompt 匹配
        assert_eq!(store.search("swordsman").len(), 1);
        // 不匹配
        assert_eq!(store.search("不存在").len(), 0);
    }

    #[test]
    fn filter_by_project_and_folder() {
        let mut store = test_store();

        let mut c1 = make_char("", "角色A");
        c1.project_id = Some("proj-1".into());
        store.create_character(c1).unwrap();

        let mut c2 = make_char("", "角色B");
        c2.project_id = Some("proj-2".into());
        let id2 = store.create_character(c2).unwrap();

        // 文件夹
        let fid = store
            .create_folder("文件夹1".into(), None, Some("proj-2".into()))
            .unwrap();
        store.move_to_folder(&id2, Some(fid.clone())).unwrap();

        assert_eq!(store.filter_by_project("proj-1").len(), 1);
        assert_eq!(store.filter_by_project("proj-2").len(), 1);
        assert_eq!(store.filter_by_folder(Some(&fid)).len(), 1);
        assert_eq!(store.root_characters().len(), 1);
    }

    #[test]
    fn add_view_sets_thumbnail() {
        let mut store = test_store();
        let id = store.create_character(make_char("", "视图测试")).unwrap();

        let view = CharacterView {
            view_type: artait_model::ViewType::Front,
            image_url: "/path/to/image.png".into(),
            generated_at: Utc::now(),
        };
        store.add_view(&id, view).unwrap();

        let c = store.get_character(&id).unwrap();
        assert_eq!(c.views.len(), 1);
        assert!(c.thumbnail_url.as_ref().unwrap().contains("image.png"));
    }

    #[test]
    fn flush_and_reload_preserves_data() {
        let dir = std::env::temp_dir()
            .join("artait-char-store-persist")
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("chars.json");

        // 创建并写入
        let mut store = CharacterStore::empty_at(path.clone());
        store.create_character(make_char("", "持久化测试")).unwrap();
        store.flush();

        // 重新加载
        let mut store2 = CharacterStore::empty_at(path);
        store2.reload();
        assert!(!store2.characters.is_empty());
        assert_eq!(store2.characters[0].name, "持久化测试");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_project_folder_creates_once() {
        let mut store = test_store();
        let fid1 = store.ensure_project_folder("proj-1", "项目一");
        let fid2 = store.ensure_project_folder("proj-1", "项目一");
        assert_eq!(fid1, fid2);
        assert_eq!(store.all_folders().len(), 1);
    }
}
