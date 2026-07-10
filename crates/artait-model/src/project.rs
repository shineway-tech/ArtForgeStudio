//! 项目数据模型。

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// 项目类型——决定创作流程的结构和导航。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProjectType {
    /// 短片：广告、MV、短视频、微电影。单篇剧本，快速从剧本到成片。
    #[serde(rename = "short_film")]
    ShortFilm,
    /// 电影：动画长片、电影。单篇分场剧本，一次性规划，大量角色和场景。
    #[default]
    #[serde(rename = "movie")]
    Movie,
    /// 剧集：多集连续剧、动画番剧。多集剧本，先建资产库再逐集推进。
    #[serde(rename = "series")]
    Series,
}

impl fmt::Display for ProjectType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShortFilm => write!(f, "短片"),
            Self::Movie => write!(f, "电影"),
            Self::Series => write!(f, "剧集"),
        }
    }
}

impl ProjectType {
    pub fn description(&self) -> &str {
        match self {
            Self::ShortFilm => "广告·MV·短视频·微电影",
            Self::Movie => "动画长片·电影",
            Self::Series => "多集连续剧·动画番剧",
        }
    }
}

/// 项目——创作内容的顶层容器。
///
/// 每个项目对应一个独立目录。项目是可选的——用户可以不建项目直接生图。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// 唯一标识（UUID v4）
    pub id: String,
    /// 项目名称
    pub name: String,
    /// 项目所在目录（绝对路径，IO 层填充）
    #[serde(skip)]
    pub path: PathBuf,
    /// 项目描述（可选）
    #[serde(default)]
    pub description: Option<String>,
    /// 项目类型
    #[serde(default)]
    pub project_type: ProjectType,
    /// 创建时间
    #[serde(default = "now_iso")]
    pub created_at: String,
    /// 最后修改时间
    #[serde(default = "now_iso")]
    pub updated_at: String,
    /// 视频场景列表（SClassScene id 列表）
    #[serde(default)]
    pub scenes: Vec<String>,
    /// 关联剧本文件（相对于项目目录）
    #[serde(default)]
    pub script_file: Option<String>,
    /// 启用音频（项目级默认）
    #[serde(default)]
    pub enable_audio: bool,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

impl Project {
    /// 创建一个新项目（不写磁盘）。
    pub fn new(name: &str, parent_dir: &Path, project_type: ProjectType) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let path = parent_dir.join(sanitize_name(name));
        Self {
            id,
            name: name.to_string(),
            path,
            description: None,
            project_type,
            created_at: now_iso(),
            updated_at: now_iso(),
            scenes: Vec::new(),
            script_file: None,
            enable_audio: false,
        }
    }

    /// 项目目录下的子目录路径。
    pub fn subdir(&self, sub: &str) -> PathBuf {
        self.path.join(sub)
    }

    /// 项目配置文件路径：`<project_dir>/project.toml`
    pub fn config_path(&self) -> PathBuf {
        self.path.join("project.toml")
    }

    /// 创建项目目录结构（如果不存在）。
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for sub in &[
            "scripts",
            "characters",
            "storyboards",
            "frames",
            "videos",
            "scenes",
        ] {
            std::fs::create_dir_all(self.path.join(sub))?;
        }
        Ok(())
    }
}

/// 通配项目——当前无活动项目时使用，资产落到全局输出目录。
pub const WILDCARD_PROJECT_ID: &str = "__wildcard__";

/// 项目引用 TOML 条目，仅存于 `app_config.toml`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    /// 项目 UUID
    pub id: String,
    /// 项目名称
    pub name: String,
    /// 项目所在目录的父目录（相对 portable_data_dir 或绝对）
    pub path: String,
    /// 项目类型
    #[serde(default)]
    pub project_type: ProjectType,
    /// 创建时间
    #[serde(default = "now_iso")]
    pub created_at: String,
}

impl From<&Project> for ProjectEntry {
    fn from(p: &Project) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            path: p.path.display().to_string(),
            project_type: p.project_type,
            created_at: p.created_at.clone(),
        }
    }
}

fn sanitize_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        "untitled".into()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_project_with_id_and_path() {
        let tmp = std::env::temp_dir().join("test_projects");
        let p = Project::new("我的项目", &tmp, ProjectType::Movie);
        assert!(!p.id.is_empty());
        assert_eq!(p.name, "我的项目");
        assert_eq!(p.project_type, ProjectType::Movie);
        assert!(p.path.ends_with("我的项目"));
    }

    #[test]
    fn subdir_joins_correctly() {
        let p = Project::new("test", Path::new("/base"), ProjectType::ShortFilm);
        assert_eq!(p.subdir("videos"), Path::new("/base/test/videos"));
    }

    #[test]
    fn config_path_is_project_toml() {
        let p = Project::new("test", Path::new("/base"), ProjectType::Series);
        assert!(p.config_path().ends_with("project.toml"));
    }

    #[test]
    fn project_type_display_and_default() {
        assert_eq!(ProjectType::ShortFilm.to_string(), "短片");
        assert_eq!(ProjectType::Movie.to_string(), "电影");
        assert_eq!(ProjectType::Series.to_string(), "剧集");
        assert_eq!(ProjectType::default(), ProjectType::Movie);
    }

    #[test]
    fn sanitize_replaces_invalid_chars() {
        assert_eq!(sanitize_name("hello:world?"), "hello_world_");
        assert_eq!(sanitize_name("  abc  "), "abc");
        assert_eq!(sanitize_name(""), "untitled");
    }
}
