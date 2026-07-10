//! 设置持久化 + 任务历史集成测试。

use std::path::PathBuf;

use artait_model::{AppConfig, ThemeId};
use artait_service::settings::{apply_basic_settings, BasicSettingsChange};
use artait_service::task_history::{TaskHistory, TaskHistoryEntry};

// ── 设置持久化 ──────────────────────────────────────────────────────────────

fn temp_config_path(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join("artait-integration-test")
        .join(name)
}

#[test]
fn settings_roundtrip_apply_and_reload() {
    let tmp = temp_config_path("settings-rt");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("config")).unwrap_or(());

    let mut cfg = AppConfig::default();
    let change = BasicSettingsChange {
        theme: ThemeId::Light,
        font_family: "Integration Sans".into(),
        font_size: 18,
        input_dir: tmp.join("input"),
        output_dir: tmp.join("out"),
        prompt_dir: tmp.join("prompt"),
        upload_api_url: Some("https://img.example.com/upload".into()),
        upload_api_key: Some("integration-key-123".into()),
    };

    let outcome = apply_basic_settings(&mut cfg, change).unwrap();

    // 新输出目录 → 应该检测到变化
    assert!(outcome.output_dir_changed);
    assert_eq!(cfg.ui.font_family, "Integration Sans");
    assert_eq!(cfg.ui.font_size, 18);
    assert_eq!(cfg.ui.theme, ThemeId::Light);
    assert_eq!(
        cfg.image_upload.api_url.as_deref(),
        Some("https://img.example.com/upload")
    );

    // 持久化
    let config_path = tmp.join("config").join("app_config.toml");
    artait_config::save_to(&config_path, &cfg).unwrap();
    assert!(config_path.exists());

    // 重新加载
    let raw = std::fs::read_to_string(&config_path).unwrap();
    let loaded: AppConfig = toml::from_str(&raw).unwrap();
    assert_eq!(loaded.ui.font_family, "Integration Sans");
    assert_eq!(loaded.paths.output_dir, tmp.join("out"));
    assert_eq!(
        loaded.image_upload.api_key.as_deref(),
        Some("integration-key-123")
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn settings_no_change_to_output_dir_returns_false() {
    let mut cfg = AppConfig::default();
    // 先 apply 一次让 output_dir 有值
    let change = BasicSettingsChange {
        theme: ThemeId::Dark,
        font_family: "Test".into(),
        font_size: 14,
        input_dir: PathBuf::from("/same/input"),
        output_dir: PathBuf::from("/same/output"),
        prompt_dir: PathBuf::from("/same/prompt"),
        upload_api_url: None,
        upload_api_key: None,
    };
    apply_basic_settings(&mut cfg, change).unwrap();

    // 再次 apply 相同目录
    let same_change = BasicSettingsChange {
        theme: ThemeId::Dark,
        font_family: "Test".into(),
        font_size: 14,
        input_dir: PathBuf::from("/same/input"),
        output_dir: PathBuf::from("/same/output"),
        prompt_dir: PathBuf::from("/same/prompt"),
        upload_api_url: None,
        upload_api_key: None,
    };
    let outcome = apply_basic_settings(&mut cfg, same_change).unwrap();
    assert!(!outcome.output_dir_changed);
}

// ── 任务历史 ──────────────────────────────────────────────────────────────

fn make_entry(id: &str, status: &str, finished_at: &str) -> TaskHistoryEntry {
    TaskHistoryEntry {
        id: id.into(),
        kind: "image".into(),
        label: format!("task-{id}"),
        status: status.into(),
        error: String::new(),
        progress: 1.0,
        last_log: String::new(),
        created_at: "2026-01-01T00:00:00Z".into(),
        finished_at: finished_at.into(),
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
fn task_history_upsert_and_query() {
    let dir = std::env::temp_dir()
        .join("artait-int-history")
        .join(format!("{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let mut history = TaskHistory::new_at(dir.join("tasks").join("task_history.json"));
    history.upsert(make_entry("t1", "completed", "2026-06-01T10:00:00Z"));
    history.upsert(make_entry("t2", "failed", "2026-06-02T10:00:00Z"));
    history.upsert(make_entry("t3", "running", "2026-06-03T10:00:00Z"));

    assert!(history.get("t1").is_some());
    assert_eq!(history.get("t1").unwrap().status, "completed");

    let recent = history.recent(2);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].id, "t3"); // most recent first

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn task_history_remove_by_filter() {
    let dir = std::env::temp_dir()
        .join("artait-int-history-remove")
        .join(format!("{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let mut history = TaskHistory::new_at(dir.join("tasks").join("task_history.json"));
    history.upsert(make_entry("c1", "completed", "2026-01-01T00:00:00Z"));
    history.upsert(make_entry("c2", "completed", "2026-01-01T00:00:01Z"));
    history.upsert(make_entry("f1", "failed", "2026-01-01T00:00:02Z"));
    history.upsert(make_entry("x1", "cancelled", "2026-01-01T00:00:03Z"));

    // Remove completed
    let removed = history.remove_by_filter("completed");
    assert_eq!(removed, 2);
    assert!(history.get("c1").is_none());
    assert!(history.get("c2").is_none());
    assert!(history.get("f1").is_some());
    assert!(history.get("x1").is_some());

    // Remove failed (includes cancelled)
    let removed = history.remove_by_filter("failed");
    assert_eq!(removed, 2);
    assert!(history.get("f1").is_none());
    assert!(history.get("x1").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn task_history_trim_limits_entries() {
    let dir = std::env::temp_dir()
        .join("artait-int-history-trim")
        .join(format!("{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let mut history = TaskHistory::new_at(dir.join("tasks").join("task_history.json"));
    for i in 0..20 {
        history.upsert(make_entry(
            &format!("t{i}"),
            "completed",
            &format!("2026-01-01T00:{i:02}:00Z"),
        ));
    }

    history.trim(5);
    assert_eq!(history.recent(100).len(), 5);

    let _ = std::fs::remove_dir_all(&dir);
}
