//! 项目存储：创建、加载、保存项目。

use std::path::{Path, PathBuf};

use artait_model::project::{Project, ProjectEntry, ProjectType};

/// 项目存储——项目的 CRUD 与持久化。
///
/// 项目不依赖 SQLite，每个项目是一个独立目录 + `project.toml`。
pub struct ProjectStore {
    /// 项目根目录（用户配置的 output_dir 下的 `projects/`）
    root: PathBuf,
}

impl ProjectStore {
    /// 以 `output_dir/projects/` 为项目根。
    pub fn new(output_dir: &Path) -> Self {
        Self {
            root: output_dir.join("projects"),
        }
    }

    /// 用指定的项目根目录构造。
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// 项目根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 创建新项目：建立目录结构 + 写 `project.toml`。
    pub fn create(&self, name: &str, project_type: ProjectType) -> Result<Project, String> {
        let project = Project::new(name, &self.root, project_type);
        project
            .ensure_dirs()
            .map_err(|e| format!("创建项目目录失败: {e}"))?;
        self.save(&project)?;
        Ok(project)
    }

    /// 保存项目配置到 `project.toml`。
    pub fn save(&self, project: &Project) -> Result<(), String> {
        let content = toml::to_string_pretty(project)
            .map_err(|e| format!("序列化 project.toml 失败: {e}"))?;
        std::fs::write(project.config_path(), content)
            .map_err(|e| format!("写入 {} 失败: {e}", project.config_path().display()))
    }

    /// 从项目目录加载 `project.toml`。
    pub fn load(&self, path: &Path) -> Result<Project, String> {
        let config_path = path.join("project.toml");
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("读取 {} 失败: {e}", config_path.display()))?;
        let mut project: Project =
            toml::from_str(&content).map_err(|e| format!("解析 project.toml 失败: {e}"))?;
        project.path = path.to_path_buf();
        Ok(project)
    }

    /// 列出项目根目录下所有有效项目。
    pub fn list(&self) -> Vec<Project> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("project.toml").exists())
            .filter_map(|e| self.load(&e.path()).ok())
            .collect()
    }

    /// 根据 ID 查找项目。
    pub fn find_by_id(&self, id: &str) -> Option<Project> {
        self.list().into_iter().find(|p| p.id == id)
    }

    /// 删除项目目录。
    pub fn remove(&self, project: &Project) -> std::io::Result<()> {
        if project.path.exists() {
            std::fs::remove_dir_all(&project.path)?;
        }
        Ok(())
    }

    /// 从 ProjectEntry 加载完整 Project（Entry 只存 parent_dir + id）。
    pub fn load_from_entry(&self, entry: &ProjectEntry) -> Option<Project> {
        // entry.path 存储的是 parent_dir
        let project_dir = Path::new(&entry.path).join(&entry.name);
        if !project_dir.join("project.toml").exists() {
            return None;
        }
        self.load(&project_dir).ok()
    }

    /// 扫描 projects/ 下所有有效项目，返回 ProjectEntry 列表。
    pub fn list_entries(&self) -> Vec<ProjectEntry> {
        self.list().iter().map(|p| p.into()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> PathBuf {
        std::env::temp_dir().join("af_project_test")
    }

    #[test]
    fn create_project_makes_dirs_and_toml() {
        let root = tmp_root().join("create_test");
        let _ = std::fs::remove_dir_all(&root);
        let store = ProjectStore::with_root(root.clone());
        let project = store.create("测试项目", ProjectType::Movie).unwrap();
        assert!(project.path.join("project.toml").exists());
        assert!(project.path.join("videos").exists());
        assert!(project.path.join("scenes").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_and_reload() {
        let root = tmp_root().join("reload_test");
        let _ = std::fs::remove_dir_all(&root);
        let store = ProjectStore::with_root(root.clone());
        let mut project = store.create("reload_test", ProjectType::Movie).unwrap();
        project.description = Some("a test".into());
        store.save(&project).unwrap();
        let loaded = store.load(&project.path).unwrap();
        assert_eq!(loaded.name, "reload_test");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_returns_created_project() {
        let root = tmp_root().join("list_test");
        let _ = std::fs::remove_dir_all(&root);
        let store = ProjectStore::with_root(root.clone());
        store.create("proj_a", ProjectType::Movie).unwrap();
        store.create("proj_b", ProjectType::Movie).unwrap();
        assert_eq!(store.list().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_by_id_works() {
        let root = tmp_root().join("find_test");
        let _ = std::fs::remove_dir_all(&root);
        let store = ProjectStore::with_root(root.clone());
        let p = store.create("find_me", ProjectType::Movie).unwrap();
        let found = store.find_by_id(&p.id);
        assert!(found.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
