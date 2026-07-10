//! 任务历史持久化。
//!
//! 以 JSON 文件存储在绿色版 `data/tasks/task_history.json`，
//! 记录所有已完成/失败/取消的任务，供任务面板持久查阅。
//! 活动任务 (running/validating/uploading/submitted/polling/saving) 不持久化。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// 持久化的任务条目（扁平化，不含 cancellation token 等运行时字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskHistoryEntry {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub status: String,
    pub error: String,
    pub progress: f32,
    pub last_log: String,
    pub created_at: String,
    pub finished_at: String,
    pub output_path: String,
    pub provider_instance_id: String,
    pub provider_id: String,
    pub model: String,
    pub prompt: String,
    /// 用于重新获取的 provider_task_id
    pub provider_task_id: String,
    /// 重新获取时需要的 endpoint/extra 信息
    pub endpoint: String,
    pub extra_json: String,
    /// 原始输出的 URL（生图成功但下载失败时可重新下载）
    #[serde(default)]
    pub retry_source_url: String,
}

/// 完整的历史文件格式。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    tasks: Vec<TaskHistoryEntry>,
}

/// 任务历史（非线程安全，调用方自行加锁）。
pub struct TaskHistory {
    path: PathBuf,
    tasks: Vec<TaskHistoryEntry>,
    index: HashMap<String, usize>,
}

impl TaskHistory {
    /// 使用指定路径加载或创建历史（供集成测试使用）。
    pub fn new_at(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let (tasks, index) = match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<HistoryFile>(&content) {
                Ok(file) => {
                    let mut index = HashMap::new();
                    for (i, t) in file.tasks.iter().enumerate() {
                        index.insert(t.id.clone(), i);
                    }
                    (file.tasks, index)
                }
                Err(_) => (Vec::new(), HashMap::new()),
            },
            Err(_) => (Vec::new(), HashMap::new()),
        };
        Self { path, tasks, index }
    }

    /// 创建或加载历史文件。
    pub fn load_or_default() -> Self {
        let path = Self::default_path();
        let (tasks, index) = match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<HistoryFile>(&content) {
                Ok(file) => {
                    let mut index = HashMap::new();
                    for (i, t) in file.tasks.iter().enumerate() {
                        index.insert(t.id.clone(), i);
                    }
                    info!(count = file.tasks.len(), path = %path.display(), "加载任务历史");
                    (file.tasks, index)
                }
                Err(e) => {
                    warn!(error = %e, path = %path.display(), "解析任务历史失败，重建空历史");
                    let backup = path.with_extension("json.bak");
                    let _ = fs::copy(&path, &backup);
                    (Vec::new(), HashMap::new())
                }
            },
            Err(_) => {
                info!(path = %path.display(), "任务历史文件不存在，将创建");
                (Vec::new(), HashMap::new())
            }
        };
        Self { path, tasks, index }
    }

    /// 添加一个条目（如果已存在则更新）。
    pub fn upsert(&mut self, entry: TaskHistoryEntry) {
        if let Some(&idx) = self.index.get(&entry.id) {
            self.tasks[idx] = entry;
        } else {
            self.index.insert(entry.id.clone(), self.tasks.len());
            self.tasks.push(entry);
        }
        self.flush();
    }

    /// 按 id 查找。
    pub fn get(&self, id: &str) -> Option<&TaskHistoryEntry> {
        self.index.get(id).map(|&idx| &self.tasks[idx])
    }

    /// 最近 N 条（按完成时间倒序）。
    pub fn recent(&self, n: usize) -> Vec<&TaskHistoryEntry> {
        let mut v: Vec<&TaskHistoryEntry> = self.tasks.iter().collect();
        v.sort_by(|a, b| b.finished_at.cmp(&a.finished_at));
        v.truncate(n);
        v
    }

    /// 所有条目（按完成时间倒序）。
    #[allow(dead_code)]
    pub fn all(&self) -> Vec<&TaskHistoryEntry> {
        let mut v: Vec<&TaskHistoryEntry> = self.tasks.iter().collect();
        v.sort_by(|a, b| b.finished_at.cmp(&a.finished_at));
        v
    }

    /// 把所有条目持久化到磁盘。
    pub fn remove_by_filter(&mut self, filter: &str) -> usize {
        let before = self.tasks.len();
        self.tasks.retain(|task| match filter {
            "completed" => task.status != "completed",
            "failed" => task.status != "failed" && task.status != "cancelled",
            "all" => false,
            _ => true,
        });
        if self.tasks.len() == before {
            return 0;
        }

        self.index.clear();
        for (i, task) in self.tasks.iter().enumerate() {
            self.index.insert(task.id.clone(), i);
        }
        self.flush();
        before - self.tasks.len()
    }

    /// 按 id 删除单条历史。
    pub fn remove_by_id(&mut self, id: &str) -> bool {
        let Some(index) = self.index.remove(id) else {
            return false;
        };
        self.tasks.remove(index);
        self.index.clear();
        for (i, task) in self.tasks.iter().enumerate() {
            self.index.insert(task.id.clone(), i);
        }
        self.flush();
        true
    }

    fn flush(&self) {
        if let Some(parent) = self.path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                warn!(error = %e, path = %parent.display(), "创建任务历史目录失败");
                return;
            }
        }

        let file = HistoryFile {
            version: 1,
            tasks: self.tasks.clone(),
        };
        let tmp = self.path.with_extension("json.tmp");
        match serde_json::to_string_pretty(&file) {
            Ok(json) => {
                if let Err(e) = fs::write(&tmp, &json) {
                    warn!(error = %e, path = %tmp.display(), "写入临时历史文件失败");
                    return;
                }
                if let Err(e) = fs::rename(&tmp, &self.path) {
                    warn!(error = %e, path = %self.path.display(), "重命名历史文件失败");
                    let _ = fs::remove_file(&tmp);
                }
            }
            Err(e) => warn!(error = %e, "序列化任务历史失败"),
        }
    }

    fn default_path() -> PathBuf {
        let path = artait_model::portable_data_dir()
            .join("tasks")
            .join("task_history.json");
        if !path.exists() {
            if let Some(old) = legacy_history_path().filter(|p| p.exists()) {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if let Err(e) = fs::copy(&old, &path) {
                    warn!(error = %e, old = %old.display(), new = %path.display(), "迁移任务历史失败");
                }
            }
        }
        path
    }

    /// 限制历史条目数（防止无限增长）。
    pub fn trim(&mut self, max_entries: usize) {
        if self.tasks.len() > max_entries {
            self.tasks.sort_by(|a, b| b.finished_at.cmp(&a.finished_at));
            self.tasks.truncate(max_entries);
            self.index.clear();
            for (i, t) in self.tasks.iter().enumerate() {
                self.index.insert(t.id.clone(), i);
            }
            self.flush();
        }
    }
}

fn legacy_history_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "ArtAIT", "ArtAIT")
        .map(|d| d.data_dir().join("task_history.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, status: &str) -> TaskHistoryEntry {
        TaskHistoryEntry {
            id: id.into(),
            kind: "image".into(),
            label: "test".into(),
            status: status.into(),
            error: String::new(),
            progress: 1.0,
            last_log: String::new(),
            created_at: "2026-06-10T00:00:00Z".into(),
            finished_at: "2026-06-10T00:00:01Z".into(),
            output_path: String::new(),
            provider_instance_id: String::new(),
            provider_id: String::new(),
            model: String::new(),
            prompt: String::new(),
            provider_task_id: String::new(),
            endpoint: String::new(),
            extra_json: String::new(),
            retry_source_url: String::new(),
        }
    }

    #[test]
    fn flush_creates_parent_directory() {
        let dir = std::env::temp_dir()
            .join("artait-task-history")
            .join(format!("{}", std::process::id()))
            .join("nested");
        let _ = fs::remove_dir_all(dir.parent().unwrap());
        let path = dir.join("task_history.json");
        let mut history = TaskHistory {
            path: path.clone(),
            tasks: Vec::new(),
            index: HashMap::new(),
        };

        history.upsert(entry("task-1", "failed"));

        assert!(path.exists());
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn remove_failed_filter_removes_failed_and_cancelled() {
        let dir = std::env::temp_dir()
            .join("artait-task-history-remove")
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut history = TaskHistory {
            path: dir.join("task_history.json"),
            tasks: Vec::new(),
            index: HashMap::new(),
        };
        history.upsert(entry("completed", "completed"));
        history.upsert(entry("failed", "failed"));
        history.upsert(entry("cancelled", "cancelled"));

        let removed = history.remove_by_filter("failed");

        assert_eq!(2, removed);
        assert!(history.get("completed").is_some());
        assert!(history.get("failed").is_none());
        assert!(history.get("cancelled").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
