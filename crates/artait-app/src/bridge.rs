//! 把 TaskRunner 的事件桥接到 Slint AppState。
//!
//! 设计：单独 tokio 任务订阅 broadcast，每次事件通过 invoke_from_event_loop
//! 转回 UI 线程。UI 端持有的 tasks 列表是"活动 + 历史"的合并视图。
//! 已完成/失败/取消的任务会持久化到 task_history.json。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use artait_model::{TaskEvent, TaskKind};
use artait_task::{TaskRecord, TaskRunner};
use chrono::{DateTime, Utc};
use slint::{ComponentHandle, Image, ModelRc, VecModel, Weak};
use tokio::sync::Mutex as TokioMutex;

use crate::task_history::{TaskHistory, TaskHistoryEntry};
use crate::ui::{AppShell, AppState, TaskItem};

use artait_service::TaskMeta;

/// 从任务闭包传给桥接层的元数据（线程安全互斥锁包装）。
pub type TaskMetaMap = Arc<Mutex<HashMap<String, TaskMeta>>>;

/// UI 端任务行的内部完整状态（含已完成的）。
#[derive(Debug, Clone)]
struct UiTaskState {
    id: String,
    kind: TaskKind,
    label: String,
    status: String,
    progress: f32,
    last_log: String,
    error: String,
    finished_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    // 额外元数据
    output_path: String,
    provider_instance_id: String,
    provider_id: String,
    model: String,
    prompt: String,
    provider_task_id: String,
    endpoint: String,
    extra_json: String,
    retry_source_url: String,
}

impl UiTaskState {
    fn from_record(r: &TaskRecord) -> Self {
        Self {
            id: r.id.clone(),
            kind: r.kind,
            label: r.label.clone(),
            status: r.status.label().to_string(),
            progress: r.progress,
            last_log: r.last_log.clone().unwrap_or_default(),
            error: r.error.clone().unwrap_or_default(),
            finished_at: None,
            created_at: r.created_at,
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

    fn to_item(&self) -> TaskItem {
        let kind_str = match self.kind {
            TaskKind::Image => "image",
            TaskKind::Character => "character",
            TaskKind::Video => "video",
            TaskKind::Analysis => "analysis",
            TaskKind::PromptOptimization => "prompt_opt",
            TaskKind::ActionBatch => "action_batch",
            TaskKind::ScriptGeneration => "script_gen",
        };
        let finished = self
            .finished_at
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        let created = self.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
        TaskItem {
            id: self.id.clone().into(),
            label: self.label.clone().into(),
            status: self.status.clone().into(),
            progress: self.progress,
            last_log: self.last_log.clone().into(),
            error: self.error.clone().into(),
            kind: kind_str.into(),
            output_path: self.output_path.clone().into(),
            provider_name: self.provider_instance_id.clone().into(),
            model: self.model.clone().into(),
            mode: task_generation_mode(self).unwrap_or_default().into(),
            prompt: self.prompt.clone().into(),
            thumb: Image::default(),
            has_thumb: false,
            has_provider_task_id: !self.provider_task_id.is_empty(),
            finished_at: finished.into(),
            created_at: created.into(),
        }
    }

    fn to_history_entry(&self) -> Option<TaskHistoryEntry> {
        let finished_at = self.finished_at?;
        if matches!(
            self.status.as_str(),
            "running" | "validating" | "uploading" | "submitted" | "polling" | "saving" | "idle"
        ) {
            return None;
        }
        Some(TaskHistoryEntry {
            id: self.id.clone(),
            kind: task_kind_history_str(self.kind).to_string(),
            label: self.label.clone(),
            status: self.status.clone(),
            error: self.error.clone(),
            progress: self.progress,
            last_log: self.last_log.clone(),
            created_at: self.created_at.to_rfc3339(),
            finished_at: finished_at.to_rfc3339(),
            output_path: self.output_path.clone(),
            provider_instance_id: self.provider_instance_id.clone(),
            provider_id: self.provider_id.clone(),
            model: self.model.clone(),
            prompt: self.prompt.clone(),
            provider_task_id: self.provider_task_id.clone(),
            endpoint: self.endpoint.clone(),
            extra_json: self.extra_json.clone(),
            retry_source_url: self.retry_source_url.clone(),
        })
    }
}

const FINISHED_RETAIN_SECS: i64 = 8;
const MAX_HISTORY_ENTRIES: usize = 500;

fn task_kind_history_str(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Image => "image",
        TaskKind::Character => "character",
        TaskKind::Video => "video",
        TaskKind::Analysis => "analysis",
        TaskKind::PromptOptimization => "prompt_optimization",
        TaskKind::ActionBatch => "action_batch",
        TaskKind::ScriptGeneration => "script_generation",
    }
}

/// 启动事件桥接。在调用方持有 tokio runtime 句柄。
pub fn spawn_bridge(
    runner: Arc<TaskRunner>,
    rt: &tokio::runtime::Handle,
    app_weak: Weak<AppShell>,
    history: Option<Arc<TokioMutex<TaskHistory>>>,
    task_meta_map: Option<TaskMetaMap>,
) {
    // UI 状态由桥接任务维护
    let states: Arc<TokioMutex<HashMap<String, UiTaskState>>> =
        Arc::new(TokioMutex::new(HashMap::new()));

    // 启动后立即把持久化历史推到 UI，避免请求列表首次打开为空，
    // 等到执行任意任务后才突然合并大量历史记录。
    {
        let states_init = states.clone();
        let app_weak_init = app_weak.clone();
        let history_init = history.clone();
        let meta_init = task_meta_map.clone();
        rt.spawn(async move {
            push_to_ui(&states_init, app_weak_init, &history_init, &meta_init).await;
        });
    }

    // 1) 事件流
    let mut rx = runner.subscribe();
    let states_ev = states.clone();
    let app_weak_ev = app_weak.clone();
    let runner_ev = runner.clone();
    let history_ev = history.clone();
    let meta_ev = task_meta_map.clone();
    rt.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    apply_event(&states_ev, &runner_ev, ev, &meta_ev, &history_ev).await;
                    push_to_ui(&states_ev, app_weak_ev.clone(), &history_ev, &meta_ev).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("task bus lagged: {n}");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // 2) 周期性清除活动状态中的已完成任务（历史仍保留）
    let states_clean = states.clone();
    let app_weak_clean = app_weak;
    let history_clean = history;
    let meta_clean = task_meta_map;
    rt.spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            let removed = {
                let mut map = states_clean.lock().await;
                let now = Utc::now();
                let before = map.len();
                map.retain(|_, s| {
                    s.finished_at
                        .map(|t| (now - t).num_seconds() < FINISHED_RETAIN_SECS)
                        .unwrap_or(true)
                });
                before != map.len()
            };
            if removed {
                push_to_ui(
                    &states_clean,
                    app_weak_clean.clone(),
                    &history_clean,
                    &meta_clean,
                )
                .await;
            }
        }
    });
}

async fn apply_event(
    states: &TokioMutex<HashMap<String, UiTaskState>>,
    runner: &TaskRunner,
    ev: TaskEvent,
    task_meta: &Option<TaskMetaMap>,
    history: &Option<Arc<TokioMutex<TaskHistory>>>,
) {
    let mut map = states.lock().await;
    match ev {
        TaskEvent::TaskStarted { task_id, .. } => {
            let snap = runner.snapshot().await;
            if let Some(r) = snap.into_iter().find(|r| r.id == task_id) {
                let mut s = UiTaskState::from_record(&r);
                s.status = "running".to_string();
                // 合并元数据
                if let Some(meta_map) = task_meta {
                    let meta = meta_map.lock().await.get(&task_id).cloned();
                    let meta = match meta {
                        Some(meta) => Some(meta),
                        None => {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                            meta_map.lock().await.get(&task_id).cloned()
                        }
                    };
                    if let Some(meta) = meta {
                        merge_task_meta(&mut s, meta);
                    }
                }
                map.insert(task_id, s);
            }
        }
        TaskEvent::TaskProgress { task_id, fraction } => {
            if let Some(s) = map.get_mut(&task_id) {
                merge_task_meta_if_available(s, &task_id, task_meta).await;
                s.progress = fraction;
            }
        }
        TaskEvent::TaskLog {
            task_id, message, ..
        } => {
            if let Some(s) = map.get_mut(&task_id) {
                merge_task_meta_if_available(s, &task_id, task_meta).await;
                s.last_log = message;
            }
        }
        TaskEvent::TaskCompleted { task_id } => {
            if let Some(s) = map.get_mut(&task_id) {
                merge_task_meta_if_available(s, &task_id, task_meta).await;
                s.status = "completed".to_string();
                s.progress = 1.0;
                s.finished_at = Some(Utc::now());
                persist_task(s, history).await;
            }
        }
        TaskEvent::TaskFailed { task_id, error } => {
            if let Some(s) = map.get_mut(&task_id) {
                merge_task_meta_if_available(s, &task_id, task_meta).await;
                s.status = "failed".to_string();
                s.error = error;
                s.finished_at = Some(Utc::now());
                persist_task(s, history).await;
            }
        }
        TaskEvent::TaskCancelled { task_id } => {
            if let Some(s) = map.get_mut(&task_id) {
                merge_task_meta_if_available(s, &task_id, task_meta).await;
                s.status = "cancelled".to_string();
                s.finished_at = Some(Utc::now());
                persist_task(s, history).await;
            }
        }
        TaskEvent::TaskOutputCreated { task_id, asset } => {
            if let Some(s) = map.get_mut(&task_id) {
                merge_task_meta_if_available(s, &task_id, task_meta).await;
                s.output_path = asset.path.display().to_string();
                // 同步更新历史中的 output_path
                if let Some(h) = history {
                    let mut hg = h.lock().await;
                    if let Some(mut entry) = hg.get(&task_id).cloned() {
                        entry.output_path = s.output_path.clone();
                        hg.upsert(entry);
                    }
                }
            }
        }
        TaskEvent::TaskRoundUpdate { .. } => {}
    }
}

async fn merge_task_meta_if_available(
    state: &mut UiTaskState,
    task_id: &str,
    task_meta: &Option<TaskMetaMap>,
) {
    if let Some(meta_map) = task_meta {
        if let Some(meta) = meta_map.lock().await.get(task_id).cloned() {
            merge_task_meta(state, meta);
        }
    }
}

fn merge_task_meta(state: &mut UiTaskState, meta: TaskMeta) {
    state.output_path = meta.output_path;
    state.provider_instance_id = meta.provider_instance_id;
    state.provider_id = meta.provider_id;
    state.model = meta.model;
    state.prompt = meta.prompt;
    state.provider_task_id = meta.provider_task_id;
    state.endpoint = meta.endpoint;
    state.extra_json = meta.extra_json;
    state.retry_source_url = meta.retry_source_url;
}

async fn persist_task(s: &UiTaskState, history: &Option<Arc<TokioMutex<TaskHistory>>>) {
    if let Some(h) = history {
        if let Some(mut entry) = s.to_history_entry() {
            // 如果错误信息包含 URL，存为 retry_source_url（URL 下载失败时可重新下载）
            if entry.retry_source_url.is_empty() && !entry.error.is_empty() {
                // 从错误中提取 URL：匹配 "from http(s)://..." 或直接 http(s)://
                for pat in &["from http://", "from https://"] {
                    if let Some(pos) = entry.error.rfind(pat) {
                        let start = pos + pat.len() - 7;
                        let end = entry.error[start..]
                            .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
                            .map(|e| start + e)
                            .unwrap_or(entry.error.len());
                        entry.retry_source_url = entry.error[start..end].to_string();
                        break;
                    }
                }
            }
            let mut hg = h.lock().await;
            hg.upsert(entry);
            hg.trim(MAX_HISTORY_ENTRIES);
        }
    }
}

async fn push_to_ui(
    states: &TokioMutex<HashMap<String, UiTaskState>>,
    app_weak: Weak<AppShell>,
    history: &Option<Arc<TokioMutex<TaskHistory>>>,
    task_meta: &Option<TaskMetaMap>,
) {
    let map = states.lock().await;
    let mut items: Vec<UiTaskState> = map.values().cloned().collect();
    if let Some(meta_map) = task_meta {
        let meta_map = meta_map.lock().await;
        for item in &mut items {
            if let Some(meta) = meta_map.get(&item.id).cloned() {
                merge_task_meta(item, meta);
            }
        }
    }

    // 合并历史中不在活动状态的条目
    if let Some(h) = history {
        let hg = h.lock().await;
        for entry in hg.recent(200) {
            if !items.iter().any(|t| t.id == entry.id) {
                let kind = TaskKind::from_history_str(&entry.kind);
                let finished_at = chrono::DateTime::parse_from_rfc3339(&entry.finished_at)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc));
                let created_at = chrono::DateTime::parse_from_rfc3339(&entry.created_at)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(Utc::now);
                items.push(UiTaskState {
                    id: entry.id.clone(),
                    kind,
                    label: entry.label.clone(),
                    status: entry.status.clone(),
                    progress: entry.progress,
                    last_log: entry.last_log.clone(),
                    error: entry.error.clone(),
                    finished_at,
                    created_at,
                    output_path: entry.output_path.clone(),
                    provider_instance_id: entry.provider_instance_id.clone(),
                    provider_id: entry.provider_id.clone(),
                    model: entry.model.clone(),
                    prompt: entry.prompt.clone(),
                    provider_task_id: entry.provider_task_id.clone(),
                    endpoint: entry.endpoint.clone(),
                    extra_json: entry.extra_json.clone(),
                    retry_source_url: entry.retry_source_url.clone(),
                });
            }
        }
    }

    // 排序：进行中优先，再按完成/创建时间倒序；同秒用 id 兜底保证稳定。
    items.sort_by(|a, b| {
        let in_progress = |s: &str| artait_model::is_active_task_status(s);
        let a_active = in_progress(&a.status);
        let b_active = in_progress(&b.status);
        b_active
            .cmp(&a_active)
            .then_with(|| match (a.finished_at, b.finished_at) {
                (Some(af), Some(bf)) => bf.cmp(&af),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b.created_at.cmp(&a.created_at),
            })
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| b.id.cmp(&a.id))
    });

    let mut active_scene_count = 0;
    let mut active_character_count = 0;
    let mut active_ui_count = 0;
    let mut active_effect_count = 0;
    let mut active_animation_scene_count = 0;
    let mut active_animation_character_count = 0;
    let mut active_character_turnaround_count = 0;
    let mut active_video_count = 0;
    for item in items
        .iter()
        .filter(|item| artait_model::is_active_task_status(&item.status))
    {
        if item.kind == TaskKind::Video {
            active_video_count += 1;
            continue;
        }
        if item.kind != TaskKind::Image || !is_gallery_generation_task(item) {
            continue;
        }
        match task_generation_mode(item).as_deref() {
            Some("scene") => active_scene_count += 1,
            Some("character") => active_character_count += 1,
            Some("ui_concept") => active_ui_count += 1,
            Some("effect") => active_effect_count += 1,
            Some("animation_scene") => active_animation_scene_count += 1,
            Some("animation_character") => active_animation_character_count += 1,
            Some("character_turnaround") => active_character_turnaround_count += 1,
            _ => {}
        }
    }
    let active_generation_count = active_scene_count
        + active_character_count
        + active_ui_count
        + active_effect_count
        + active_animation_scene_count
        + active_animation_character_count
        + active_character_turnaround_count
        + active_video_count;
    let running_count = items
        .iter()
        .filter(|t| artait_model::is_active_task_status(&t.status))
        .count() as i32;
    let completed_count = items.iter().filter(|t| t.status == "completed").count() as i32;
    let failed_count = items
        .iter()
        .filter(|t| t.status == "failed" || t.status == "cancelled")
        .count() as i32;
    let prompt_opt_items: Vec<&UiTaskState> = items
        .iter()
        .filter(|t| task_generation_mode(t).as_deref() == Some("prompt_opt"))
        .collect();
    let prompt_opt_count = prompt_opt_items.len() as i32;
    let prompt_opt_running_count = prompt_opt_items
        .iter()
        .filter(|t| artait_model::is_active_task_status(&t.status))
        .count() as i32;
    let prompt_opt_completed_count = prompt_opt_items
        .iter()
        .filter(|t| t.status == "completed")
        .count() as i32;
    let prompt_opt_failed_count = prompt_opt_items
        .iter()
        .filter(|t| t.status == "failed" || t.status == "cancelled")
        .count() as i32;
    drop(map);

    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = app_weak.upgrade() {
            let state = app.global::<AppState>();
            let view: Vec<TaskItem> = items
                .iter()
                .map(|state| {
                    let mut item = state.to_item();
                    if state.output_path.trim().is_empty() {
                        return item;
                    }
                    let source = std::path::Path::new(&state.output_path);
                    if !source.exists() {
                        return item;
                    }
                    let thumb = artait_asset::ensure_thumbnail(source);
                    if let Ok(image) = Image::load_from_path(&thumb) {
                        item.thumb = image;
                        item.has_thumb = true;
                    }
                    item
                })
                .collect();
            state.set_tasks_count_running(running_count);
            state.set_tasks_count_completed(completed_count);
            state.set_tasks_count_failed(failed_count);
            state.set_tasks_count_prompt_opt(prompt_opt_count);
            state.set_tasks_count_prompt_opt_running(prompt_opt_running_count);
            state.set_tasks_count_prompt_opt_completed(prompt_opt_completed_count);
            state.set_tasks_count_prompt_opt_failed(prompt_opt_failed_count);
            state.set_tasks(ModelRc::new(VecModel::from(view)));
            crate::update_request_list_counts(&state);
            update_last_task_status(&state, &items);
            state.set_gallery_generating_count(active_generation_count);
            state.set_gallery_generating_scene_count(active_scene_count);
            state.set_gallery_generating_character_count(active_character_count);
            state.set_gallery_generating_ui_count(active_ui_count);
            state.set_gallery_generating_effect_count(active_effect_count);
            state.set_gallery_generating_animation_scene_count(active_animation_scene_count);
            state
                .set_gallery_generating_animation_character_count(active_animation_character_count);
            state.set_gallery_generating_character_turnaround_count(
                active_character_turnaround_count,
            );
            state.set_gallery_generating_video_count(active_video_count);
        }
    });
}

/// 根据 tasks（已排序：进行中优先，再按完成时间倒序）推断最近一次已完成任务状态。
/// 写入 `ws-last-task-status`："" 无已完成任务 | "success" | "failed"。
fn update_last_task_status(state: &AppState, items: &[UiTaskState]) {
    let status = items
        .iter()
        .find(|s| !artait_model::is_active_task_status(s.status.as_str()))
        .map(|s| {
            let st = s.status.as_str();
            if st == "completed" {
                "success"
            } else if st == "failed" || st == "cancelled" {
                "failed"
            } else {
                ""
            }
        })
        .unwrap_or("");
    state.set_ws_last_task_status(status.into());
}

fn task_generation_mode(item: &UiTaskState) -> Option<String> {
    let mode = serde_json::from_str::<serde_json::Value>(&item.extra_json)
        .ok()
        .and_then(|json| {
            json.get("mode")
                .and_then(|mode| mode.as_str())
                .map(str::to_owned)
        });
    mode.or_else(|| {
        if item.kind == TaskKind::PromptOptimization {
            Some("prompt_opt".to_string())
        } else if item.kind == TaskKind::ActionBatch {
            Some("action_sequence".to_string())
        } else if item.kind == TaskKind::Image {
            infer_generation_mode_from_legacy_item(item)
        } else {
            None
        }
    })
}

fn is_gallery_generation_task(item: &UiTaskState) -> bool {
    item.label.starts_with("生成 · ") || item.label.starts_with("批量 ")
}

fn infer_generation_mode_from_legacy_item(item: &UiTaskState) -> Option<String> {
    let label = item.label.as_str();
    if label.contains("UI 概念") {
        return Some("ui_concept".to_string());
    }
    if label.contains("创建角色") {
        return Some("character".to_string());
    }
    if label.contains("特效") {
        return Some("effect".to_string());
    }
    if label.contains("动画场景") {
        return Some("animation_scene".to_string());
    }
    if label.contains("动画角色") {
        return Some("animation_character".to_string());
    }
    if label.contains("角色三视图") {
        return Some("character_turnaround".to_string());
    }
    if label.contains("分镜板") {
        return Some("storyboard".to_string());
    }
    if label.contains("动作序列") || label.starts_with("批量 ") {
        return Some("action_sequence".to_string());
    }
    if label.contains("创建场景") {
        return Some("scene".to_string());
    }

    let output = item.output_path.replace('\\', "/");
    if output.contains("/ui") {
        return Some("ui_concept".to_string());
    }
    if output.contains("/creations") {
        return Some("character".to_string());
    }
    if output.contains("/effects") {
        return Some("effect".to_string());
    }
    if output.contains("/animation_scenes") {
        return Some("animation_scene".to_string());
    }
    if output.contains("/animation_characters") {
        return Some("animation_character".to_string());
    }
    if output.contains("/character_turnarounds") {
        return Some("character_turnaround".to_string());
    }
    if output.contains("/storyboards") {
        return Some("storyboard".to_string());
    }
    if output.contains("/batch") {
        return Some("action_sequence".to_string());
    }
    if output.contains("/scenes") {
        return Some("scene".to_string());
    }

    None
}
