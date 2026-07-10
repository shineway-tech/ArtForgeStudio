//! ArtAIT 任务运行时。
//!
//! 设计要点：
//! - `TaskRunner` 单例，`tokio::Semaphore` 控全局并发；
//! - 每个任务携带 `CancellationToken`，支持精确取消；
//! - 事件通过 `broadcast` 推送给所有订阅者（UI、日志、持久化）；
//! - 活动任务列表通过 `Mutex<Vec<TaskRecord>>` 暴露给 UI，UI 可订阅
//!   `TaskEvent::TaskStarted/Completed/Cancelled/Failed` 增量更新；
//! - Runner 自身不感知具体 provider，使用方在 closure 里写业务。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use artait_model::{LogLevel, TaskEvent, TaskKind, TaskStatus};
use chrono::{DateTime, Utc};
use tokio::runtime::Handle;
use tokio::sync::{broadcast, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

pub mod saver;

pub use saver::{ResultSaver, SaveError, SaveResult, SavedAsset};

pub const EVENT_CHANNEL_CAP: usize = 256;
pub const DEFAULT_MAX_CONCURRENT: usize = 4;

/// 活动任务的轻量快照，提供给 UI 渲染列表。
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: String,
    pub kind: TaskKind,
    pub label: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub progress: f32,
    pub last_log: Option<String>,
    pub error: Option<String>,
    pub cancel: CancellationToken,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("task cancelled")]
    Cancelled,
    #[error("task failed: {0}")]
    Failed(String),
    #[error("task panicked")]
    Panicked,
}

pub type TaskResult<T> = std::result::Result<T, TaskError>;

/// 任务上下文。Closure 通过它发事件、查取消、报告进度。
#[derive(Clone)]
pub struct TaskContext {
    pub id: String,
    pub kind: TaskKind,
    pub cancel: CancellationToken,
    bus: broadcast::Sender<TaskEvent>,
}

impl TaskContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn check_cancelled(&self) -> TaskResult<()> {
        if self.is_cancelled() {
            Err(TaskError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn progress(&self, fraction: f32) {
        let _ = self.bus.send(TaskEvent::TaskProgress {
            task_id: self.id.clone(),
            fraction: fraction.clamp(0.0, 1.0),
        });
    }

    pub fn log(&self, level: LogLevel, message: impl Into<String>) {
        let _ = self.bus.send(TaskEvent::TaskLog {
            task_id: self.id.clone(),
            level,
            message: message.into(),
        });
    }

    pub fn info(&self, message: impl Into<String>) {
        self.log(LogLevel::Info, message);
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.log(LogLevel::Warn, message);
    }
}

/// 调度参数。
#[derive(Debug, Clone)]
pub struct TaskSpec {
    pub label: String,
    pub kind: TaskKind,
    /// 单个任务的硬超时。None = 不超时。
    pub timeout: Option<Duration>,
}

impl TaskSpec {
    pub fn new(label: impl Into<String>, kind: TaskKind) -> Self {
        Self {
            label: label.into(),
            kind,
            timeout: None,
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }
}

/// Runner 单例。
pub struct TaskRunner {
    bus: broadcast::Sender<TaskEvent>,
    semaphore: Arc<Semaphore>,
    active: Arc<Mutex<HashMap<String, TaskRecord>>>,
    handle: Option<Handle>,
}

impl TaskRunner {
    pub fn new(max_concurrent: usize) -> Arc<Self> {
        Self::new_inner(max_concurrent, Handle::try_current().ok())
    }

    pub fn new_with_handle(max_concurrent: usize, handle: Handle) -> Arc<Self> {
        Self::new_inner(max_concurrent, Some(handle))
    }

    fn new_inner(max_concurrent: usize, handle: Option<Handle>) -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(EVENT_CHANNEL_CAP);
        Arc::new(Self {
            bus: tx,
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            active: Arc::new(Mutex::new(HashMap::new())),
            handle,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.bus.subscribe()
    }

    /// 当前活动任务的快照（按创建时间升序）。
    pub async fn snapshot(&self) -> Vec<TaskRecord> {
        let map = self.active.lock().await;
        let mut v: Vec<TaskRecord> = map.values().cloned().collect();
        v.sort_by_key(|r| r.created_at);
        v
    }

    /// 通过 ID 取消单个任务。返回是否找到。
    pub async fn cancel(&self, id: &str) -> bool {
        let map = self.active.lock().await;
        if let Some(r) = map.get(id) {
            r.cancel.cancel();
            true
        } else {
            false
        }
    }

    /// 提交任务。返回 task_id。任务 closure 接收 `TaskContext` 并返回 `TaskResult<()>`。
    pub fn spawn<F, Fut>(self: &Arc<Self>, spec: TaskSpec, body: F) -> String
    where
        F: FnOnce(TaskContext) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = TaskResult<()>> + Send + 'static,
    {
        let id = Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let ctx = TaskContext {
            id: id.clone(),
            kind: spec.kind,
            cancel: cancel.clone(),
            bus: self.bus.clone(),
        };
        let record = TaskRecord {
            id: id.clone(),
            kind: spec.kind,
            label: spec.label.clone(),
            status: TaskStatus::Validating,
            created_at: Utc::now(),
            progress: 0.0,
            last_log: None,
            error: None,
            cancel: cancel.clone(),
        };

        let runner = self.clone();
        let timeout = spec.timeout;
        let id_for_task = id.clone();
        let task = async move {
            let id = id_for_task;
            // 注册
            runner
                .active
                .lock()
                .await
                .insert(id.clone(), record.clone());
            let _ = runner.bus.send(TaskEvent::TaskStarted {
                task_id: id.clone(),
                kind: spec.kind,
            });
            info!(target = "task", id = %id, label = %spec.label, "task started");

            // 并发限制
            let permit = match runner.semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    runner.fail(&id, "semaphore closed").await;
                    return;
                }
            };

            // 取消短路
            if cancel.is_cancelled() {
                drop(permit);
                runner.cancelled(&id).await;
                return;
            }

            // 执行 + 可选超时；与 cancel 竞速，保证长 HTTP 也能立即取消。
            let cancel_for_select = cancel.clone();
            let body_fut = body(ctx);
            let outcome = match timeout {
                Some(t) => {
                    tokio::select! {
                        r = tokio::time::timeout(t, body_fut) => match r {
                            Ok(r) => r,
                            Err(_) => Err(TaskError::Failed("task timeout".into())),
                        },
                        _ = cancel_for_select.cancelled() => Err(TaskError::Cancelled),
                    }
                }
                None => {
                    tokio::select! {
                        r = body_fut => r,
                        _ = cancel_for_select.cancelled() => Err(TaskError::Cancelled),
                    }
                }
            };

            drop(permit);

            match outcome {
                Ok(()) => runner.complete(&id).await,
                Err(TaskError::Cancelled) => runner.cancelled(&id).await,
                Err(TaskError::Failed(msg)) => runner.fail(&id, &msg).await,
                Err(TaskError::Panicked) => runner.fail(&id, "task panicked").await,
            }
        };

        if let Some(handle) = self.handle.clone().or_else(|| Handle::try_current().ok()) {
            handle.spawn(task);
        } else {
            let _ = self.bus.send(TaskEvent::TaskFailed {
                task_id: id.clone(),
                error: "missing tokio runtime".into(),
            });
            warn!(target = "task", id = %id, "task spawn failed: missing tokio runtime");
        }

        id
    }

    async fn complete(&self, id: &str) {
        if let Some(mut r) = self.active.lock().await.remove(id) {
            r.status = TaskStatus::Completed;
            info!(target = "task", id = %id, "task completed");
        }
        let _ = self.bus.send(TaskEvent::TaskCompleted {
            task_id: id.to_string(),
        });
    }

    async fn cancelled(&self, id: &str) {
        if let Some(mut r) = self.active.lock().await.remove(id) {
            r.status = TaskStatus::Cancelled;
            info!(target = "task", id = %id, "task cancelled");
        }
        let _ = self.bus.send(TaskEvent::TaskCancelled {
            task_id: id.to_string(),
        });
    }

    async fn fail(&self, id: &str, error: &str) {
        if let Some(mut r) = self.active.lock().await.remove(id) {
            r.status = TaskStatus::Failed;
            r.error = Some(error.to_string());
            warn!(target = "task", id = %id, error, "task failed");
        }
        let _ = self.bus.send(TaskEvent::TaskFailed {
            task_id: id.to_string(),
            error: error.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn task_completes_emits_started_and_completed() {
        rt().block_on(async {
            let runner = TaskRunner::new(2);
            let mut rx = runner.subscribe();
            let id = runner.spawn(TaskSpec::new("ok", TaskKind::Image), |ctx| async move {
                ctx.info("running");
                Ok(())
            });

            let mut started = false;
            let mut completed = false;
            for _ in 0..10 {
                match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                    Ok(Ok(TaskEvent::TaskStarted { task_id, .. })) if task_id == id => {
                        started = true;
                    }
                    Ok(Ok(TaskEvent::TaskCompleted { task_id })) if task_id == id => {
                        completed = true;
                        break;
                    }
                    Ok(Ok(_)) => continue,
                    _ => break,
                }
            }
            assert!(started, "TaskStarted not received");
            assert!(completed, "TaskCompleted not received");
        });
    }

    #[test]
    fn cancel_emits_cancelled_event() {
        rt().block_on(async {
            let runner = TaskRunner::new(2);
            let mut rx = runner.subscribe();
            let id = runner.spawn(TaskSpec::new("long", TaskKind::Image), |ctx| async move {
                for _ in 0..50 {
                    ctx.check_cancelled()?;
                    sleep(Duration::from_millis(20)).await;
                }
                Ok(())
            });

            // 等启动
            sleep(Duration::from_millis(50)).await;
            assert!(runner.cancel(&id).await);

            let mut cancelled = false;
            for _ in 0..20 {
                if let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                    if let TaskEvent::TaskCancelled { task_id } = ev {
                        if task_id == id {
                            cancelled = true;
                            break;
                        }
                    }
                }
            }
            assert!(cancelled, "TaskCancelled not received");
            assert!(runner.snapshot().await.iter().all(|r| r.id != id));
        });
    }

    #[test]
    fn failure_emits_failed_event() {
        rt().block_on(async {
            let runner = TaskRunner::new(2);
            let mut rx = runner.subscribe();
            let id = runner.spawn(TaskSpec::new("err", TaskKind::Image), |_ctx| async move {
                Err(TaskError::Failed("boom".into()))
            });

            let mut failed = false;
            for _ in 0..10 {
                if let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                    if let TaskEvent::TaskFailed { task_id, error } = ev {
                        if task_id == id {
                            assert!(error.contains("boom"));
                            failed = true;
                            break;
                        }
                    }
                }
            }
            assert!(failed);
        });
    }

    #[test]
    fn timeout_treated_as_failure() {
        rt().block_on(async {
            let runner = TaskRunner::new(2);
            let mut rx = runner.subscribe();
            let id = runner.spawn(
                TaskSpec::new("slow", TaskKind::Image).with_timeout(Duration::from_millis(50)),
                |_ctx| async move {
                    sleep(Duration::from_secs(2)).await;
                    Ok(())
                },
            );

            let mut got = false;
            for _ in 0..10 {
                if let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                    if let TaskEvent::TaskFailed { task_id, error } = ev {
                        if task_id == id && error.contains("timeout") {
                            got = true;
                            break;
                        }
                    }
                }
            }
            assert!(got);
        });
    }

    #[test]
    fn semaphore_serializes_when_capacity_one() {
        rt().block_on(async {
            let runner = TaskRunner::new(1);
            let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let max_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));

            for _ in 0..5 {
                let c = counter.clone();
                let m = max_seen.clone();
                runner.spawn(
                    TaskSpec::new("p", TaskKind::Image),
                    move |_ctx| async move {
                        let cur = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        let prev = m.load(std::sync::atomic::Ordering::SeqCst);
                        if cur > prev {
                            m.store(cur, std::sync::atomic::Ordering::SeqCst);
                        }
                        sleep(Duration::from_millis(30)).await;
                        c.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    },
                );
            }

            // 等所有任务完成
            for _ in 0..50 {
                sleep(Duration::from_millis(20)).await;
                if runner.snapshot().await.is_empty() {
                    break;
                }
            }
            assert_eq!(max_seen.load(std::sync::atomic::Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn runner_with_handle_can_spawn_from_plain_thread() {
        let runtime = rt();
        let runner = TaskRunner::new_with_handle(2, runtime.handle().clone());
        let mut rx = runner.subscribe();
        let runner_for_thread = runner.clone();

        let id = std::thread::spawn(move || {
            runner_for_thread.spawn(
                TaskSpec::new("plain-thread", TaskKind::Analysis),
                |_ctx| async move { Ok(()) },
            )
        })
        .join()
        .unwrap();

        runtime.block_on(async {
            let mut completed = false;
            for _ in 0..10 {
                match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                    Ok(Ok(TaskEvent::TaskCompleted { task_id })) if task_id == id => {
                        completed = true;
                        break;
                    }
                    Ok(Ok(_)) => continue,
                    _ => break,
                }
            }
            assert!(completed, "TaskCompleted not received");
        });
    }
}
