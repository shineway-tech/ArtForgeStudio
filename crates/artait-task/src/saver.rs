//! 把 GenerationOutput 四种形态归一化到本地文件。
//!
//! 调用方提供输出目录和文件名前缀（含扩展名前缀但不含具体名）。
//! ResultSaver 决定最终文件名（按时间戳 + 序号）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use artait_provider::{request::GenerationOutput, HttpClient, HttpRequest, HttpResponse};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Local;
use std::sync::Arc;

const DOWNLOAD_RETRY_DELAYS: &[Duration] = &[
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
    Duration::from_secs(45),
];

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("download failed: {0}")]
    Download(String),
    #[error("base64 decode: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("AsyncTask 不能直接保存：需要先轮询")]
    AsyncTaskUnsupported,
    #[error("dir create failed {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type SaveResult<T> = std::result::Result<T, SaveError>;

#[derive(Debug, Clone)]
pub struct SavedAsset {
    pub path: PathBuf,
    pub bytes: u64,
    pub mime: String,
    pub provider_metadata: serde_json::Value,
}

pub struct ResultSaver {
    pub output_dir: PathBuf,
    pub prefix: String,
    pub http: Arc<dyn HttpClient>,
}

impl ResultSaver {
    pub fn new(output_dir: PathBuf, prefix: String, http: Arc<dyn HttpClient>) -> Self {
        Self {
            output_dir,
            prefix,
            http,
        }
    }

    pub async fn save(&self, output: GenerationOutput) -> SaveResult<SavedAsset> {
        std::fs::create_dir_all(&self.output_dir).map_err(|e| SaveError::CreateDir {
            path: self.output_dir.clone(),
            source: e,
        })?;

        match output {
            GenerationOutput::File { path, metadata } => {
                // 已是本地文件，复制到目标目录
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("bin");
                let dest = self.target_path(ext);
                std::fs::copy(&path, &dest)?;
                let bytes = std::fs::metadata(&dest)?.len();
                Ok(SavedAsset {
                    path: dest,
                    bytes,
                    mime: mime_for_ext(ext).into(),
                    provider_metadata: metadata,
                })
            }
            GenerationOutput::Url { url, metadata } => {
                let headers = download_headers_from_metadata(&metadata);
                let resp = download_url_with_retries(
                    self.http.as_ref(),
                    &url,
                    &headers,
                    DOWNLOAD_RETRY_DELAYS,
                )
                .await?;
                let mime = mime_from_response(&resp).unwrap_or("image/png");
                let ext = ext_for_mime(mime);
                let dest = self.target_path(ext);
                std::fs::write(&dest, &resp.body)?;
                Ok(SavedAsset {
                    path: dest,
                    bytes: resp.body.len() as u64,
                    mime: mime.into(),
                    provider_metadata: metadata,
                })
            }
            GenerationOutput::Base64 {
                data,
                mime,
                metadata,
            } => {
                let bytes = B64.decode(data)?;
                let ext = ext_for_mime(&mime);
                let dest = self.target_path(ext);
                std::fs::write(&dest, &bytes)?;
                Ok(SavedAsset {
                    path: dest,
                    bytes: bytes.len() as u64,
                    mime,
                    provider_metadata: metadata,
                })
            }
            GenerationOutput::AsyncTask { .. } => Err(SaveError::AsyncTaskUnsupported),
        }
    }

    fn target_path(&self, ext: &str) -> PathBuf {
        let stamp = Local::now().format("%Y%m%d-%H%M%S");
        let mut p = self
            .output_dir
            .join(format!("{}-{stamp}.{ext}", self.prefix));
        let mut n = 1u32;
        while p.exists() {
            p = self
                .output_dir
                .join(format!("{}-{stamp}-{n}.{ext}", self.prefix));
            n += 1;
        }
        p
    }
}

async fn download_url_with_retries(
    http: &dyn HttpClient,
    url: &str,
    headers: &[(String, String)],
    retry_delays: &[Duration],
) -> SaveResult<HttpResponse> {
    let max_attempts = retry_delays.len() + 1;
    for attempt in 1..=max_attempts {
        let mut req = HttpRequest::get(url.to_string());
        for (name, value) in headers {
            req = req.header(name.clone(), value.clone());
        }
        let resp = http
            .execute(req)
            .await
            .map_err(|e| SaveError::Download(format!("HTTP fetch 失败: {e}")))?;
        if resp.is_success() {
            return Ok(resp);
        }

        let can_retry = is_retryable_download_status(resp.status) && attempt < max_attempts;
        if !can_retry {
            return Err(SaveError::Download(format!(
                "HTTP {} from {} after {} attempt(s)",
                resp.status, url, attempt
            )));
        }

        tokio::time::sleep(retry_delays[attempt - 1]).await;
    }

    Err(SaveError::Download(format!("exhausted retries for {url}")))
}

fn is_retryable_download_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 404 | 408 | 409 | 425 | 429 | 500..=599)
}

fn download_headers_from_metadata(metadata: &serde_json::Value) -> Vec<(String, String)> {
    metadata
        .get("download_headers")
        .and_then(|headers| headers.as_object())
        .map(|headers| {
            headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.to_string(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn mime_from_response(resp: &artait_provider::HttpResponse) -> Option<&str> {
    for (k, v) in &resp.headers {
        if k.eq_ignore_ascii_case("content-type") {
            return Some(v.split(';').next().unwrap_or(v).trim());
        }
    }
    None
}

fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        _ => "bin",
    }
}

fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

#[allow(dead_code)]
fn ensure_path(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use artait_provider::{HttpResponse, ProviderResult};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeHttp;

    #[async_trait]
    impl HttpClient for FakeHttp {
        async fn execute(&self, _req: HttpRequest) -> ProviderResult<HttpResponse> {
            Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "image/png".into())],
                body: bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\nfake-data"),
            })
        }
    }

    struct FlakyHttp {
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl HttpClient for FlakyHttp {
        async fn execute(&self, _req: HttpRequest) -> ProviderResult<HttpResponse> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Ok(HttpResponse {
                    status: 401,
                    headers: Vec::new(),
                    body: bytes::Bytes::new(),
                });
            }
            Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "image/png".into())],
                body: bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\nfake-data"),
            })
        }
    }

    struct AuthRequiredHttp {
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl HttpClient for AuthRequiredHttp {
        async fn execute(&self, req: HttpRequest) -> ProviderResult<HttpResponse> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let authorized = req
                .headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == "Bearer test-key");
            if !authorized {
                return Ok(HttpResponse {
                    status: 401,
                    headers: Vec::new(),
                    body: bytes::Bytes::new(),
                });
            }
            Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "image/png".into())],
                body: bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\nfake-data"),
            })
        }
    }

    #[tokio::test]
    async fn save_url_writes_file() {
        let dir = std::env::temp_dir().join("artait-saver-url");
        let _ = std::fs::remove_dir_all(&dir);
        let saver = ResultSaver::new(dir.clone(), "test".into(), Arc::new(FakeHttp));
        let out = GenerationOutput::Url {
            url: "https://example.com/x.png".into(),
            metadata: serde_json::Value::Null,
        };
        let saved = saver.save(out).await.unwrap();
        assert!(saved.path.exists());
        assert!(saved.path.extension().unwrap() == "png");
        assert!(saved.bytes > 0);
    }

    #[tokio::test]
    async fn url_download_retries_transient_unauthorized() {
        let http = FlakyHttp {
            attempts: AtomicUsize::new(0),
        };
        let resp = download_url_with_retries(
            &http,
            "https://example.com/eventual.png",
            &[],
            &[Duration::ZERO],
        )
        .await
        .unwrap();

        assert_eq!(200, resp.status);
        assert_eq!(2, http.attempts.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn save_url_uses_download_headers_from_metadata() {
        let dir = std::env::temp_dir().join("artait-saver-auth-url");
        let _ = std::fs::remove_dir_all(&dir);
        let http = Arc::new(AuthRequiredHttp {
            attempts: AtomicUsize::new(0),
        });
        let saver = ResultSaver::new(dir.clone(), "test".into(), http.clone());
        let out = GenerationOutput::Url {
            url: "https://example.com/private.png".into(),
            metadata: serde_json::json!({
                "download_headers": {
                    "Authorization": "Bearer test-key"
                }
            }),
        };

        let saved = saver.save(out).await.unwrap();

        assert!(saved.path.exists());
        assert_eq!(1, http.attempts.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn save_base64_writes_file() {
        let dir = std::env::temp_dir().join("artait-saver-b64");
        let _ = std::fs::remove_dir_all(&dir);
        let saver = ResultSaver::new(dir.clone(), "test".into(), Arc::new(FakeHttp));
        let data = B64.encode(b"\xff\xd8\xff\xe0jpeg-fake");
        let out = GenerationOutput::Base64 {
            data,
            mime: "image/jpeg".into(),
            metadata: serde_json::Value::Null,
        };
        let saved = saver.save(out).await.unwrap();
        assert!(saved.path.exists());
        assert!(saved.path.extension().unwrap() == "jpg");
    }
}
