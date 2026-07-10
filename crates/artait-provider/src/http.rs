//! HTTP client 抽象 + 默认 reqwest 实现。
//!
//! provider trait 不直接依赖 reqwest，方便 mock 和单元测试。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::ProviderResult;
use artait_model::ProviderError;

/// 简化版 HTTP 响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: bytes::Bytes,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> ProviderResult<T> {
        serde_json::from_slice(&self.body)
            .map_err(|e| ProviderError::InvalidResponse(format!("JSON 解析失败: {e}")))
    }
}

/// HTTP 请求构造器。
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<bytes::Bytes>,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: None,
        }
    }

    pub fn post(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: None,
        }
    }

    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    pub fn bearer(mut self, token: &str) -> Self {
        self.headers
            .push(("Authorization".into(), format!("Bearer {token}")));
        self
    }

    pub fn json_body<T: serde::Serialize>(mut self, body: &T) -> ProviderResult<Self> {
        let bytes = serde_json::to_vec(body)
            .map_err(|e| ProviderError::InvalidResponse(format!("JSON 编码失败: {e}")))?;
        self.headers
            .push(("Content-Type".into(), "application/json".into()));
        self.body = Some(bytes::Bytes::from(bytes));
        Ok(self)
    }

    pub fn body(mut self, body: impl Into<bytes::Bytes>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }
}

/// 抽象 HTTP client。
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn execute(&self, req: HttpRequest) -> ProviderResult<HttpResponse>;
}

/// 默认 reqwest 实现。
pub struct ReqwestClient {
    inner: reqwest::Client,
}

impl ReqwestClient {
    pub fn new() -> Self {
        let inner = reqwest::Client::builder()
            .user_agent("ArtAIT/0.1")
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client build failed");
        Self { inner }
    }

    pub fn shared() -> Arc<dyn HttpClient> {
        Arc::new(Self::new())
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn execute(&self, req: HttpRequest) -> ProviderResult<HttpResponse> {
        let mut builder = match req.method {
            HttpMethod::Get => self.inner.get(&req.url),
            HttpMethod::Post => self.inner.post(&req.url),
            HttpMethod::Put => self.inner.put(&req.url),
            HttpMethod::Delete => self.inner.delete(&req.url),
        };
        for (k, v) in req.headers {
            builder = builder.header(k, v);
        }
        if let Some(t) = req.timeout {
            builder = builder.timeout(t);
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let resp = builder.send().await.map_err(map_reqwest_err)?;

        let status = resp.status().as_u16();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
            .collect();
        let body = resp.bytes().await.map_err(map_reqwest_err)?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

fn map_reqwest_err(e: reqwest::Error) -> ProviderError {
    if e.is_timeout() {
        ProviderError::ProviderTimeout
    } else if e.is_connect() {
        ProviderError::ConnectionFailed(e.to_string())
    } else if e.status().map(|s| s.as_u16() == 429).unwrap_or(false) {
        ProviderError::RateLimited
    } else {
        ProviderError::ConnectionFailed(e.to_string())
    }
}
