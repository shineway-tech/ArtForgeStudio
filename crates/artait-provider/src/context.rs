//! Provider 调用上下文。

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::http::{HttpClient, ReqwestClient};

#[derive(Clone)]
pub struct ProviderContext {
    pub instance_id: String,
    pub provider_id: String,
    pub endpoint: Option<String>,
    pub secret: Option<String>,
    pub extra: serde_json::Value,
    pub output_path: PathBuf,
    pub run_dir: Option<PathBuf>,
    pub cancellation: CancellationToken,
    pub http: Arc<dyn HttpClient>,
}

impl ProviderContext {
    pub fn new_for_test(instance_id: impl Into<String>, provider_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            provider_id: provider_id.into(),
            endpoint: None,
            secret: None,
            extra: serde_json::Value::Null,
            output_path: PathBuf::new(),
            run_dir: None,
            cancellation: CancellationToken::new(),
            http: Arc::new(ReqwestClient::new()),
        }
    }

    pub fn with_http(
        instance_id: impl Into<String>,
        provider_id: impl Into<String>,
        http: Arc<dyn HttpClient>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            provider_id: provider_id.into(),
            endpoint: None,
            secret: None,
            extra: serde_json::Value::Null,
            output_path: PathBuf::new(),
            run_dir: None,
            cancellation: CancellationToken::new(),
            http,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

impl std::fmt::Debug for ProviderContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderContext")
            .field("instance_id", &self.instance_id)
            .field("provider_id", &self.provider_id)
            .field("endpoint", &self.endpoint)
            .field("has_secret", &self.secret.is_some())
            .field("output_path", &self.output_path)
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}
