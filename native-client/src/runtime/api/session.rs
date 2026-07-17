use super::{ApiError, TokenSet};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const SESSION_DIR: &str = "session";
const REFRESH_TOKEN_FILE: &str = "refresh-token";
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) trait RefreshTokenStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, ApiError>;
    fn save(&self, token: &str) -> Result<(), ApiError>;
    fn clear(&self) -> Result<(), ApiError>;
}

pub(crate) struct FileRefreshTokenStore {
    path: PathBuf,
}

impl FileRefreshTokenStore {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(SESSION_DIR).join(REFRESH_TOKEN_FILE),
        }
    }

    fn prepare_parent(&self) -> Result<(), ApiError> {
        let parent = self.path.parent().ok_or_else(|| ApiError::LocalState {
            message: "刷新令牌文件缺少父目录".to_string(),
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| local_state_error("创建登录状态目录", error))?;
        restrict_directory(parent)
    }

    fn temporary_path(&self) -> PathBuf {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.path.with_file_name(format!(
            ".{REFRESH_TOKEN_FILE}.{}.{}.tmp",
            std::process::id(),
            sequence
        ))
    }
}

impl RefreshTokenStore for FileRefreshTokenStore {
    fn load(&self) -> Result<Option<String>, ApiError> {
        let value = match fs::read_to_string(&self.path) {
            Ok(value) => value,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(local_state_error("读取刷新令牌", error)),
        };
        restrict_file(&self.path)?;
        let value = value.trim();
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(value.to_string()))
        }
    }

    fn save(&self, token: &str) -> Result<(), ApiError> {
        if token.trim().is_empty() {
            return Err(ApiError::LocalState {
                message: "拒绝保存空的刷新令牌".to_string(),
            });
        }
        self.prepare_parent()?;
        let temporary = self.temporary_path();
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options
                .open(&temporary)
                .map_err(|error| local_state_error("创建刷新令牌临时文件", error))?;
            file.write_all(token.as_bytes())
                .map_err(|error| local_state_error("写入刷新令牌", error))?;
            file.sync_all()
                .map_err(|error| local_state_error("同步刷新令牌", error))?;
            drop(file);

            #[cfg(windows)]
            if let Err(error) = fs::remove_file(&self.path) {
                if error.kind() != ErrorKind::NotFound {
                    return Err(local_state_error("替换旧刷新令牌", error));
                }
            }
            fs::rename(&temporary, &self.path)
                .map_err(|error| local_state_error("保存刷新令牌", error))?;
            restrict_file(&self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn clear(&self) -> Result<(), ApiError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(local_state_error("删除刷新令牌", error)),
        }
    }
}

fn local_state_error(action: &str, error: std::io::Error) -> ApiError {
    ApiError::LocalState {
        message: format!("{action}失败：{error}"),
    }
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), ApiError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| local_state_error("设置登录状态目录权限", error))
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), ApiError> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), ApiError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| local_state_error("设置刷新令牌文件权限", error))
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), ApiError> {
    Ok(())
}

#[derive(Default)]
struct SessionState {
    access_token: Option<String>,
    refreshing: bool,
    refresh_epoch: u64,
    last_refresh_result: Option<Result<String, ApiError>>,
}

pub(crate) struct SessionManager {
    store: Arc<dyn RefreshTokenStore>,
    state: Mutex<SessionState>,
    refresh_finished: Condvar,
}

impl SessionManager {
    pub(crate) fn new(store: Arc<dyn RefreshTokenStore>) -> Self {
        Self {
            store,
            state: Mutex::new(SessionState::default()),
            refresh_finished: Condvar::new(),
        }
    }

    pub(crate) fn with_file_store(data_dir: &Path) -> Self {
        Self::new(Arc::new(FileRefreshTokenStore::new(data_dir)))
    }

    pub(crate) fn access_token(&self) -> Option<String> {
        self.lock_state().access_token.clone()
    }

    pub(crate) fn has_refresh_token(&self) -> Result<bool, ApiError> {
        Ok(self.store.load()?.is_some())
    }

    pub(crate) fn install_tokens(&self, tokens: &TokenSet) -> Result<(), ApiError> {
        self.store.save(&tokens.refresh_token)?;
        let mut state = self.lock_state();
        state.access_token = Some(tokens.access_token.clone());
        state.last_refresh_result = Some(Ok(tokens.access_token.clone()));
        Ok(())
    }

    pub(crate) fn clear(&self) -> Result<(), ApiError> {
        self.store.clear()?;
        let mut state = self.lock_state();
        state.access_token = None;
        state.last_refresh_result = None;
        Ok(())
    }

    pub(crate) fn clear_access_token(&self) {
        self.lock_state().access_token = None;
    }

    pub(crate) fn refresh<F>(
        &self,
        rejected_access_token: Option<&str>,
        refresh: F,
    ) -> Result<String, ApiError>
    where
        F: FnOnce(&str) -> Result<TokenSet, ApiError>,
    {
        {
            let mut state = self.lock_state();
            if let (Some(rejected), Some(current)) =
                (rejected_access_token, state.access_token.as_deref())
            {
                if current != rejected {
                    return Ok(current.to_string());
                }
            }

            if state.refreshing {
                let observed_epoch = state.refresh_epoch;
                while state.refreshing && state.refresh_epoch == observed_epoch {
                    state = self
                        .refresh_finished
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                return state
                    .last_refresh_result
                    .clone()
                    .unwrap_or(Err(ApiError::AuthenticationRequired));
            }

            state.refreshing = true;
            state.access_token = None;
            state.last_refresh_result = None;
        }

        let result = self.store.load().and_then(|stored| {
            let token = stored.ok_or(ApiError::AuthenticationRequired)?;
            let tokens = refresh(&token)?;
            self.store.save(&tokens.refresh_token)?;
            Ok(tokens.access_token)
        });

        let mut state = self.lock_state();
        state.refreshing = false;
        state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
        state.access_token = result.clone().ok();
        state.last_refresh_result = Some(result.clone());
        self.refresh_finished.notify_all();
        result
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    #[derive(Default)]
    pub(crate) struct MemoryRefreshTokenStore {
        value: Mutex<Option<String>>,
    }

    impl MemoryRefreshTokenStore {
        pub(crate) fn new(value: Option<&str>) -> Self {
            Self {
                value: Mutex::new(value.map(str::to_string)),
            }
        }
    }

    impl RefreshTokenStore for MemoryRefreshTokenStore {
        fn load(&self) -> Result<Option<String>, ApiError> {
            Ok(self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone())
        }

        fn save(&self, token: &str) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(token.to_string());
            Ok(())
        }

        fn clear(&self) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::MemoryRefreshTokenStore;
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    fn tokens(access: &str, refresh: &str) -> TokenSet {
        TokenSet {
            access_token: access.to_string(),
            access_expires_in_seconds: 1800,
            refresh_token: refresh.to_string(),
            refresh_expires_at: "2099-01-01T00:00:00Z".to_string(),
            token_type: "X-Token".to_string(),
        }
    }

    #[test]
    fn installing_tokens_persists_refresh_and_keeps_access_in_memory() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = SessionManager::new(store.clone());
        manager.install_tokens(&tokens("access-1", "refresh-1")).unwrap();

        assert_eq!(manager.access_token().as_deref(), Some("access-1"));
        assert_eq!(store.load().unwrap().as_deref(), Some("refresh-1"));
    }

    #[test]
    fn persisted_refresh_token_is_available_to_a_fresh_session_manager() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let first = SessionManager::new(store.clone());
        first.install_tokens(&tokens("access-1", "refresh-1")).unwrap();
        drop(first);

        let second = SessionManager::new(store.clone());
        assert!(second.has_refresh_token().unwrap());
        let access = second
            .refresh(None, |refresh| {
                assert_eq!(refresh, "refresh-1");
                Ok(tokens("access-2", "refresh-2"))
            })
            .unwrap();

        assert_eq!(access, "access-2");
        assert_eq!(store.load().unwrap().as_deref(), Some("refresh-2"));
    }

    #[test]
    fn file_store_persists_rotated_token_and_clears_it() {
        let dir = std::env::temp_dir().join(format!(
            "artforge-session-test-{}",
            uuid::Uuid::new_v4()
        ));
        let first = FileRefreshTokenStore::new(&dir);
        first.save("refresh-1").unwrap();
        assert_eq!(first.load().unwrap().as_deref(), Some("refresh-1"));

        let second = FileRefreshTokenStore::new(&dir);
        second.save("refresh-2").unwrap();
        assert_eq!(second.load().unwrap().as_deref(), Some("refresh-2"));
        second.clear().unwrap();
        assert_eq!(second.load().unwrap(), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn file_store_restricts_directory_and_file_permissions() {
        let dir = std::env::temp_dir().join(format!(
            "artforge-session-mode-test-{}",
            uuid::Uuid::new_v4()
        ));
        let store = FileRefreshTokenStore::new(&dir);
        store.save("refresh-secret").unwrap();

        let file_mode = fs::metadata(&store.path).unwrap().permissions().mode() & 0o777;
        let directory_mode = fs::metadata(store.path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(directory_mode, 0o700);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_refresh_is_single_flight() {
        let store = Arc::new(MemoryRefreshTokenStore::new(Some("refresh-old")));
        let manager = Arc::new(SessionManager::new(store.clone()));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        for _ in 0..6 {
            let manager = manager.clone();
            let calls = calls.clone();
            handles.push(thread::spawn(move || {
                manager
                    .refresh(None, |refresh| {
                        assert_eq!(refresh, "refresh-old");
                        calls.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(30));
                        Ok(tokens("access-new", "refresh-new"))
                    })
                    .unwrap()
            }));
        }

        for handle in handles {
            assert_eq!(handle.join().unwrap(), "access-new");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.load().unwrap().as_deref(), Some("refresh-new"));
    }

    #[test]
    fn rotated_access_token_prevents_a_second_refresh() {
        let store = Arc::new(MemoryRefreshTokenStore::new(Some("refresh-new")));
        let manager = SessionManager::new(store);
        manager
            .install_tokens(&tokens("access-new", "refresh-new"))
            .unwrap();
        let value = manager
            .refresh(Some("access-old"), |_| panic!("refresh must not run"))
            .unwrap();
        assert_eq!(value, "access-new");
    }
}
