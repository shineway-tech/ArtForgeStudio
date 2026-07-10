//! Provider trait、能力子 trait、Registry、HttpClient 抽象。
//!
//! 阶段 3 将完整实现。当前只暴露 trait 定义和占位 Registry。

use std::{collections::HashMap, sync::Arc, time::Duration};

use artait_model::{ConnectionStatus, ProviderError};
use async_trait::async_trait;
use tracing::debug;

pub mod context;
pub mod http;
pub mod meta;
pub mod request;

pub use context::ProviderContext;
pub use http::{HttpClient, HttpMethod, HttpRequest, HttpResponse, ReqwestClient};
pub use meta::ProviderMeta;

pub type ProviderResult<T> = std::result::Result<T, ProviderError>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderModelList {
    pub generation: Vec<String>,
    pub analysis: Vec<String>,
    pub video: Vec<String>,
}

/// 单一 Provider trait + 能力查询。
///
/// `Arc<dyn Provider>` 可直接分发；能力子 trait 不继承 Provider，
/// 通过 `as_xxx()` 返回 `Option<&dyn Capability>` 访问。
#[async_trait]
pub trait Provider: Send + Sync {
    fn meta(&self) -> &ProviderMeta;

    async fn test_connection(&self, ctx: &ProviderContext) -> ProviderResult<ConnectionStatus>;

    async fn list_models(&self, _ctx: &ProviderContext) -> ProviderResult<ProviderModelList> {
        Ok(ProviderModelList::default())
    }

    fn as_image_generator(&self) -> Option<&dyn ImageGenerator> {
        None
    }
    fn as_character_generator(&self) -> Option<&dyn CharacterGenerator> {
        None
    }
    fn as_analyzer(&self) -> Option<&dyn Analyzer> {
        None
    }
    fn as_video_generator(&self) -> Option<&dyn VideoGenerator> {
        None
    }
    fn as_pollable(&self) -> Option<&dyn Pollable> {
        None
    }
}

#[async_trait]
pub trait ImageGenerator: Send + Sync {
    async fn generate(
        &self,
        req: request::ImageGenerationRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<request::GenerationOutput>;
}

#[async_trait]
pub trait CharacterGenerator: Send + Sync {
    async fn generate_character(
        &self,
        req: request::CharacterGenerationRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<request::GenerationOutput>;
}

#[async_trait]
pub trait Analyzer: Send + Sync {
    async fn analyze(
        &self,
        req: request::AnalysisRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<request::AnalysisOutput>;
}

#[async_trait]
pub trait VideoGenerator: Send + Sync {
    async fn generate_video(
        &self,
        req: request::VideoGenerationRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<request::VideoOutput>;
}

#[async_trait]
pub trait Pollable: Send + Sync {
    /// 单次轮询。返回 Some(output) 表示完成，None 表示仍在处理中。
    async fn poll(
        &self,
        provider_task_id: &str,
        ctx: &ProviderContext,
    ) -> ProviderResult<Option<request::GenerationOutput>>;

    /// 完整轮询循环：重复调用 poll() 直到完成、取消或耗尽次数。
    /// 使用 PollingStrategy 统一控制 backoff / retry / max_polls。
    async fn poll_until_done(
        &self,
        provider_task_id: &str,
        ctx: &ProviderContext,
        strategy: &PollingStrategy,
    ) -> ProviderResult<request::GenerationOutput> {
        for attempt in 0..strategy.max_polls {
            if ctx.is_cancelled() {
                return Err(ProviderError::TaskCancelled);
            }

            match self.poll(provider_task_id, ctx).await? {
                Some(output) => return Ok(output),
                None => {
                    let delay = strategy.backoff_at(attempt);
                    debug!(
                        target: "poll",
                        task_id = %provider_task_id,
                        attempt = attempt + 1,
                        max = strategy.max_polls,
                        delay_ms = delay.as_millis(),
                        "轮询未就绪，等待重试"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }

        Err(ProviderError::ProviderTimeout)
    }
}

/// 轮询策略：统一 backoff / retry / max_polls 配置。
#[derive(Debug, Clone)]
pub struct PollingStrategy {
    /// 单次轮询间隔。
    pub interval: Duration,
    /// 最大轮询次数。
    pub max_polls: usize,
    /// 退避倍率（≥1.0）。1.0 = 固定间隔，2.0 = 指数退避。
    pub backoff_multiplier: f64,
    /// 单次间隔上限，防止退避过大。
    pub max_interval: Duration,
}

impl Default for PollingStrategy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(3),
            max_polls: 60,
            backoff_multiplier: 1.0,
            max_interval: Duration::from_secs(30),
        }
    }
}

impl PollingStrategy {
    /// 计算第 n 次轮询（0-based）的退避间隔。
    pub fn backoff_at(&self, attempt: usize) -> Duration {
        if attempt == 0 || self.backoff_multiplier <= 1.0 {
            return self.interval;
        }
        let factor = self.backoff_multiplier.powi(attempt as i32);
        let ms = (self.interval.as_millis() as f64 * factor) as u64;
        Duration::from_millis(ms).min(self.max_interval)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "register() 可能覆盖已有 provider，必须检查冲突"]
pub enum RegisterResult {
    /// 新 provider 首次注册。
    Inserted,
    /// 覆盖了已有 provider，返回被替换的 id（与本次相同）。
    Replaced,
}

/// Provider 注册表。
#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册 provider。若同 id 已存在则覆盖，并通过返回值告知调用方。
    #[must_use = "注册结果必须检查：Replaced 表示冲突覆盖"]
    pub fn register(&mut self, provider: Arc<dyn Provider>) -> RegisterResult {
        let id = provider.meta().id.to_string();
        let old = self.providers.insert(id, provider);
        if old.is_some() {
            RegisterResult::Replaced
        } else {
            RegisterResult::Inserted
        }
    }

    pub fn get(&self, provider_id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(provider_id).cloned()
    }

    pub fn list(&self) -> Vec<Arc<dyn Provider>> {
        self.providers.values().cloned().collect()
    }
}
