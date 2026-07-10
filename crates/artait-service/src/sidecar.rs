//! 通用 Sidecar 进程管理。
//!
//! 设计目标：
//! - 管理外部辅助进程的完整生命周期（启动、健康检查、空闲回收、退出清理）
//! - 通过 trait 抽象支持任意 sidecar，当前实现 Prompt Optimizer
//! - 按需启动，空闲超时自动关闭，零配置时优雅降级
//!
//! 使用示例：
//! ```ignore
//! let mut mgr = SidecarManager::new(http_client, rt_handle);
//! let client = mgr.ensure_prompt_optimizer(&cfg.sidecar, &provider).await?;
//! let job_id = client.submit_optimization("my prompt", Some("gpt-4o"), Some("gpt-4o")).await?;
//! // ... 在 TaskRunner 闭包中轮询 ...
//! let result = client.poll_until_done(&job_id, &task_ctx).await?;
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use artait_model::SidecarConfig;
use artait_provider::HttpClient;
use artait_task::{TaskContext, TaskError};
use serde::Deserialize;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

// ── 通用 Sidecar 抽象 ────────────────────────────────────────────────────

/// 单个 sidecar 的运行时规格。
#[derive(Debug, Clone)]
pub struct SidecarSpec {
    /// 唯一标识（如 "prompt-optimizer"）
    pub id: String,
    /// 可执行文件路径
    pub exe_path: PathBuf,
    /// 健康检查 URL
    pub health_url: String,
    /// 默认端口
    pub default_port: u16,
    /// 空闲超时（None = 永不自动关闭）
    pub idle_timeout: Option<Duration>,
    /// 额外的环境变量
    pub env: Vec<(String, String)>,
}

/// 管理所有 sidecar 进程。
pub struct SidecarManager {
    http: Arc<dyn HttpClient>,
    /// 当前运行的 sidecar 句柄
    handle: Mutex<Option<SidecarHandle>>,
    /// 上次使用时间（用于空闲回收）
    last_used: Mutex<Instant>,
}

struct SidecarHandle {
    spec: SidecarSpec,
    child: Child,
    port: u16,
}

impl SidecarManager {
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self {
            http,
            handle: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
        }
    }

    /// 确保 Prompt Optimizer sidecar 正在运行，返回客户端。
    /// 若已在运行则复用；若空闲过久则重启。
    pub async fn ensure_prompt_optimizer(
        &self,
        cfg: &SidecarConfig,
    ) -> Result<PromptOptimizerClient, SidecarError> {
        // 空闲回收检查
        {
            let last = *self.last_used.lock().await;
            let timeout_dur = if cfg.prompt_optimizer_idle_timeout_secs == 0 {
                None
            } else {
                Some(Duration::from_secs(cfg.prompt_optimizer_idle_timeout_secs))
            };
            if let Some(t) = timeout_dur {
                if last.elapsed() > t {
                    self.shutdown().await;
                }
            }
        }

        // 已在运行 → 直接复用
        {
            let guard = self.handle.lock().await;
            if guard.is_some() {
                drop(guard);
                self.touch().await;
                let port = self.handle.lock().await.as_ref().unwrap().port;
                return Ok(PromptOptimizerClient::new(self.http.clone(), port));
            }
        }

        // 冷启动
        let exe_path = resolve_prompt_optimizer_exe(cfg);
        let port = if cfg.prompt_optimizer_port > 0 {
            cfg.prompt_optimizer_port
        } else {
            find_free_port().await?
        };

        let health_url = format!("http://127.0.0.1:{port}/health");

        let child = Command::new(&exe_path)
            .env("PROMPT_OPTIMIZER_SERVER_ADDR", format!("127.0.0.1:{port}"))
            .env(
                "PROMPT_OPTIMIZER_DB_PATH",
                sidecar_data_dir()
                    .join("prompt-optimizer.db")
                    .display()
                    .to_string(),
            )
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                SidecarError::SpawnFailed(format!("无法启动 {}: {}", exe_path.display(), e))
            })?;

        // 健康检查重试（最多等 10 秒）
        for i in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            match self.http_execute_get(&health_url).await {
                Ok(resp) if resp.status == 200 => {
                    tracing::info!(
                        exe = %exe_path.display(),
                        port,
                        "sidecar prompt-optimizer 已就绪 ({}ms)",
                        (i + 1) * 500
                    );
                    break;
                }
                Ok(resp) => {
                    if i == 19 {
                        return Err(SidecarError::HealthCheckFailed(format!(
                            "HTTP {}",
                            resp.status
                        )));
                    }
                }
                Err(_) if i == 19 => {
                    return Err(SidecarError::HealthCheckFailed("10 秒内未响应".into()));
                }
                Err(_) => continue,
            }
        }

        let handle = SidecarHandle {
            spec: SidecarSpec {
                id: "prompt-optimizer".into(),
                exe_path,
                health_url,
                default_port: port,
                idle_timeout: if cfg.prompt_optimizer_idle_timeout_secs == 0 {
                    None
                } else {
                    Some(Duration::from_secs(cfg.prompt_optimizer_idle_timeout_secs))
                },
                env: vec![
                    (
                        "PROMPT_OPTIMIZER_SERVER_ADDR".into(),
                        format!("127.0.0.1:{port}"),
                    ),
                    (
                        "PROMPT_OPTIMIZER_DB_PATH".into(),
                        sidecar_data_dir()
                            .join("prompt-optimizer.db")
                            .display()
                            .to_string(),
                    ),
                ],
            },
            child,
            port,
        };

        *self.handle.lock().await = Some(handle);
        self.touch().await;

        Ok(PromptOptimizerClient::new(self.http.clone(), port))
    }

    /// 关闭所有 sidecar 进程。
    pub async fn shutdown(&self) {
        if let Some(mut handle) = self.handle.lock().await.take() {
            tracing::info!(id = %handle.spec.id, "关闭 sidecar");
            let _ = handle.child.kill().await;
        }
    }

    /// 标记最近使用时间（防止空闲回收）。
    pub async fn touch(&self) {
        *self.last_used.lock().await = Instant::now();
    }

    async fn http_execute_get(
        &self,
        url: &str,
    ) -> Result<artait_provider::http::HttpResponse, artait_model::ProviderError> {
        use artait_provider::http::HttpRequest;
        self.http
            .execute(
                HttpRequest::get(url)
                    .header("Accept", "application/json")
                    .timeout(std::time::Duration::from_secs(5)),
            )
            .await
    }
}

// ── Prompt Optimizer 客户端 ──────────────────────────────────────────────

/// Prompt Optimizer sidecar 的 HTTP 客户端。
#[derive(Clone)]
pub struct PromptOptimizerClient {
    http: Arc<dyn HttpClient>,
    port: u16,
}

impl PromptOptimizerClient {
    fn new(http: Arc<dyn HttpClient>, port: u16) -> Self {
        Self { http, port }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 健康检查。
    pub async fn health(&self) -> Result<bool, SidecarError> {
        let url = format!("{}/health", self.base_url());
        let resp = self.get(&url).await?;
        Ok(resp.status == 200)
    }

    /// 提交优化任务，返回 job_id。
    pub async fn submit_optimization(
        &self,
        raw_prompt: &str,
        optimizer_model: Option<&str>,
        judge_model: Option<&str>,
    ) -> Result<String, SidecarError> {
        let url = format!("{}/api/jobs", self.base_url());
        let body = serde_json::json!({
            "title": short_title(raw_prompt),
            "rawPrompt": raw_prompt,
            "optimizerModel": optimizer_model,
            "judgeModel": judge_model,
            "runMode": "auto",
        });
        let resp = self.post_json(&url, &body).await?;
        let job: JobCreateResponse = serde_json::from_slice(&resp.body)
            .map_err(|e| SidecarError::Protocol(format!("解析 job 响应失败: {e}")))?;
        Ok(job.id)
    }

    /// 同步 Provider 设置到 sidecar。
    pub async fn sync_provider_settings(
        &self,
        base_url: &str,
        api_key: &str,
    ) -> Result<(), SidecarError> {
        let url = format!("{}/api/settings", self.base_url());
        let body = serde_json::json!({
            "cpamc_base_url": base_url,
            "cpamc_api_key": api_key,
            "api_protocol": "auto",
        });
        self.post_json(&url, &body).await?;
        Ok(())
    }

    /// 获取单个任务状态。
    pub async fn get_job(&self, job_id: &str) -> Result<JobStatus, SidecarError> {
        let url = format!("{}/api/jobs/{job_id}", self.base_url());
        let resp = self.get(&url).await?;
        let job: JobStatus = serde_json::from_slice(&resp.body)
            .map_err(|e| SidecarError::Protocol(format!("解析 job 状态失败: {e}")))?;
        Ok(job)
    }

    /// 轮询直到任务完成/失败（供 TaskRunner 闭包使用）。
    pub async fn poll_until_done(
        &self,
        job_id: &str,
        ctx: &TaskContext,
    ) -> Result<OptimizeResult, TaskError> {
        let poll_interval = Duration::from_secs(3);
        let max_polls = 100; // 最多 5 分钟
        let mut last_round = 0u32;

        ctx.info("提示词优化中…");
        ctx.progress(0.1);

        for poll_count in 0..max_polls {
            ctx.check_cancelled()?;

            let job = self
                .get_job(job_id)
                .await
                .map_err(|e| TaskError::Failed(format!("sidecar: {e}")))?;

            // 进度报告
            if job.current_round > last_round {
                last_round = job.current_round;
                let pct = (poll_count as f32 / max_polls as f32).min(0.95);
                ctx.progress(pct);
                ctx.info(format!(
                    "第 {}/{} 轮 · 当前分数 {}",
                    job.current_round,
                    job.max_rounds.unwrap_or(8),
                    job.current_score.unwrap_or(0.0) as u32
                ));
            }

            match job.status.as_str() {
                "completed" => {
                    ctx.progress(1.0);
                    let prompt = job
                        .optimized_prompt
                        .clone()
                        .unwrap_or_else(|| "优化完成但未返回结果".into());
                    ctx.info(format!("优化完成 · {} 轮", job.current_round));
                    return Ok(OptimizeResult {
                        optimized_prompt: prompt,
                        summary: job.summary.clone(),
                        score: job.current_score,
                        rounds: job.current_round,
                    });
                }
                "manual_review" => {
                    // 等待人工审核 → 取当前最佳结果
                    ctx.progress(1.0);
                    let prompt = job
                        .optimized_prompt
                        .clone()
                        .unwrap_or_else(|| "优化已达上限但未返回结果".into());
                    ctx.warn(format!("优化暂停（需人工审核）· {} 轮", job.current_round));
                    return Ok(OptimizeResult {
                        optimized_prompt: prompt,
                        summary: Some("优化已到上限，建议人工审核。".into()),
                        score: job.current_score,
                        rounds: job.current_round,
                    });
                }
                "failed" | "cancelled" => {
                    return Err(TaskError::Failed(
                        job.error.unwrap_or_else(|| format!("任务 {}", job.status)),
                    ));
                }
                _ => {
                    // pending / running — 继续等待
                    tokio::time::sleep(poll_interval).await;
                }
            }
        }

        Err(TaskError::Failed("提示词优化超时（5 分钟）".into()))
    }

    async fn get(&self, url: &str) -> Result<artait_provider::http::HttpResponse, SidecarError> {
        use artait_provider::http::HttpRequest;
        let resp = self
            .http
            .execute(
                HttpRequest::get(url)
                    .header("Accept", "application/json")
                    .timeout(Duration::from_secs(30)),
            )
            .await
            .map_err(|e| SidecarError::Http(e.to_string()))?;
        if resp.status >= 500 {
            return Err(SidecarError::Http(format!("HTTP {}", resp.status)));
        }
        Ok(resp)
    }

    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<artait_provider::http::HttpResponse, SidecarError> {
        use artait_provider::http::HttpRequest;
        let body_bytes: bytes::Bytes = serde_json::to_vec(body)
            .map_err(|e| SidecarError::Protocol(e.to_string()))?
            .into();
        let resp = self
            .http
            .execute(
                HttpRequest::post(url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json")
                    .body(body_bytes)
                    .timeout(Duration::from_secs(30)),
            )
            .await
            .map_err(|e| SidecarError::Http(e.to_string()))?;
        if resp.status >= 500 {
            return Err(SidecarError::Http(format!("HTTP {}", resp.status)));
        }
        Ok(resp)
    }
}

// ── 数据类型 ──────────────────────────────────────────────────────────────

/// 优化结果（返回给 UI）。
#[derive(Debug, Clone)]
pub struct OptimizeResult {
    pub optimized_prompt: String,
    pub summary: Option<String>,
    pub score: Option<f64>,
    pub rounds: u32,
}

/// Sidecar REST API 返回的任务状态。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub current_round: u32,
    #[serde(default)]
    pub max_rounds: Option<u32>,
    #[serde(default)]
    pub current_score: Option<f64>,
    #[serde(default)]
    pub optimized_prompt: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct JobCreateResponse {
    id: String,
}

// ── 错误类型 ──────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("sidecar 启动失败: {0}")]
    SpawnFailed(String),

    #[error("sidecar 健康检查失败: {0}")]
    HealthCheckFailed(String),

    #[error("sidecar HTTP 错误: {0}")]
    Http(String),

    #[error("sidecar 协议错误: {0}")]
    Protocol(String),

    #[error("未配置 Prompt Optimizer sidecar 路径")]
    NotConfigured,
}

// ── 辅助函数 ──────────────────────────────────────────────────────────────

/// 解析 sidecar 可执行文件路径。
fn resolve_prompt_optimizer_exe(cfg: &SidecarConfig) -> PathBuf {
    if let Some(ref path) = cfg.prompt_optimizer_path {
        let p = PathBuf::from(path);
        if p.exists() {
            return p;
        }
    }

    // 默认：与主程序同目录
    let default_name = if cfg!(windows) {
        "prompt-optimizer-server.exe"
    } else {
        "prompt-optimizer-server"
    };

    // 尝试当前工作目录
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .join(default_name);
    if cwd.exists() {
        return cwd;
    }

    // 尝试与 exe 同目录
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(default_name);
        if sibling.exists() {
            return sibling;
        }
    }

    // 回退：返回默认名（启动时会报清晰错误）
    PathBuf::from(default_name)
}

/// 查找一个可用端口。
async fn find_free_port() -> Result<u16, SidecarError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| SidecarError::SpawnFailed(format!("无法绑定端口: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| SidecarError::SpawnFailed(format!("获取端口失败: {e}")))?
        .port();
    drop(listener);
    Ok(port)
}

/// Sidecar 数据目录（绿色版）。
fn sidecar_data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "ArtAIT", "ArtForgeStudio")
        .map(|d| d.data_dir().join("sidecar"))
        .unwrap_or_else(|| PathBuf::from("data/sidecar"))
}

/// 从提示词生成简短标题。
fn short_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let first_line = trimmed.lines().next().unwrap_or(trimmed);
    let chars: Vec<char> = first_line.chars().take(60).collect();
    let s: String = chars.into_iter().collect();
    if s.len() < trimmed.len() {
        format!("{s}…")
    } else {
        s
    }
}

// ── 测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_title_truncates_long_prompt() {
        let long = "a".repeat(100);
        let title = short_title(&long);
        assert!(title.chars().count() <= 61); // 60 chars + '…'
        assert!(title.ends_with('…'));
    }

    #[test]
    fn short_title_uses_first_line() {
        let prompt = "first line\nsecond line\nthird line";
        let title = short_title(prompt);
        assert!(title.starts_with("first line"));
        assert!(!title.contains('\n'));
    }

    #[test]
    fn short_title_trims_whitespace() {
        let title = short_title("  hello world  ");
        assert_eq!(title, "hello world");
    }

    #[test]
    fn resolve_exe_returns_default_if_not_found() {
        let cfg = SidecarConfig {
            prompt_optimizer_path: Some("Z:/nonexistent/path/optimizer.exe".into()),
            ..Default::default()
        };
        let path = resolve_prompt_optimizer_exe(&cfg);
        // 不存在时回退到默认名
        assert!(path.to_string_lossy().contains("prompt-optimizer-server"));
    }

    #[test]
    fn sidecar_config_defaults() {
        let cfg = SidecarConfig::default();
        assert_eq!(cfg.prompt_optimizer_port, 0);
        assert_eq!(cfg.prompt_optimizer_idle_timeout_secs, 300);
        assert!(cfg.prompt_optimizer_path.is_none());
    }
}
