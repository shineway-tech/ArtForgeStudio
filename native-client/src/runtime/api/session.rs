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
        fs::create_dir_all(parent).map_err(|error| local_state_error("创建登录状态目录", error))?;
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
    owner_user_id: Option<String>,
    auth_epoch: u64,
    refreshing: bool,
    refresh_epoch: u64,
    last_refresh_result: Option<Result<String, ApiError>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionScope {
    pub(crate) owner_user_id: String,
    pub(crate) auth_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionAccess {
    pub(crate) access_token: String,
    pub(crate) auth_epoch: u64,
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

    pub(crate) fn access(&self) -> Option<SessionAccess> {
        let state = self.lock_state();
        state.access_token.as_ref().map(|access_token| SessionAccess {
            access_token: access_token.clone(),
            auth_epoch: state.auth_epoch,
        })
    }

    pub(crate) fn auth_epoch(&self) -> u64 {
        self.lock_state().auth_epoch
    }

    pub(crate) fn access_token_for_epoch(&self, auth_epoch: u64) -> Result<String, ApiError> {
        let mut state = self.lock_state();
        if state.auth_epoch != auth_epoch {
            return Err(ApiError::AuthenticationRequired);
        }
        if let Some(access_token) = state.access_token.as_ref() {
            return Ok(access_token.clone());
        }
        if !state.refreshing {
            return Err(ApiError::AuthenticationRequired);
        }
        let observed_refresh_epoch = state.refresh_epoch;
        while state.refreshing
            && state.refresh_epoch == observed_refresh_epoch
            && state.auth_epoch == auth_epoch
        {
            state = self
                .refresh_finished
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if state.auth_epoch != auth_epoch {
            return Err(ApiError::AuthenticationRequired);
        }
        if let Some(access_token) = state.access_token.as_ref() {
            return Ok(access_token.clone());
        }
        state
            .last_refresh_result
            .clone()
            .unwrap_or(Err(ApiError::AuthenticationRequired))
    }

    pub(crate) fn has_refresh_token(&self) -> Result<bool, ApiError> {
        Ok(self.store.load()?.is_some())
    }

    pub(crate) fn install_tokens(&self, tokens: &TokenSet) -> Result<(), ApiError> {
        self.install_tokens_with_owner(tokens, None)
    }

    pub(crate) fn install_tokens_for_user(
        &self,
        tokens: &TokenSet,
        owner_user_id: &str,
    ) -> Result<SessionScope, ApiError> {
        self.install_tokens_with_owner(tokens, Some(owner_user_id.to_string()))?;
        self.scope_for_user(owner_user_id)
            .ok_or(ApiError::AuthenticationRequired)
    }

    fn install_tokens_with_owner(
        &self,
        tokens: &TokenSet,
        owner_user_id: Option<String>,
    ) -> Result<(), ApiError> {
        let mut state = self.lock_state();
        self.store.save(&tokens.refresh_token)?;
        state.auth_epoch = state.auth_epoch.wrapping_add(1);
        state.access_token = Some(tokens.access_token.clone());
        state.owner_user_id = owner_user_id;
        state.refreshing = false;
        state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
        state.last_refresh_result = Some(Ok(tokens.access_token.clone()));
        self.refresh_finished.notify_all();
        Ok(())
    }

    pub(crate) fn clear(&self) -> Result<(), ApiError> {
        let mut state = self.lock_state();
        invalidate_session_lease(&mut state);
        let clear_result = self.store.clear();
        self.refresh_finished.notify_all();
        clear_result
    }

    pub(crate) fn clear_access_token(&self) {
        self.lock_state().access_token = None;
    }

    pub(crate) fn bind_user(&self, owner_user_id: &str) -> Result<SessionScope, ApiError> {
        let mut state = self.lock_state();
        if state.access_token.is_none() || owner_user_id.trim().is_empty() {
            return Err(ApiError::AuthenticationRequired);
        }
        if state
            .owner_user_id
            .as_deref()
            .is_some_and(|owner| owner != owner_user_id)
        {
            return Err(ApiError::AuthenticationRequired);
        }
        state.owner_user_id = Some(owner_user_id.to_string());
        Ok(SessionScope {
            owner_user_id: owner_user_id.to_string(),
            auth_epoch: state.auth_epoch,
        })
    }

    pub(crate) fn scope_for_user(&self, owner_user_id: &str) -> Option<SessionScope> {
        let state = self.lock_state();
        (state.owner_user_id.as_deref() == Some(owner_user_id)).then(|| SessionScope {
            owner_user_id: owner_user_id.to_string(),
            auth_epoch: state.auth_epoch,
        })
    }

    pub(crate) fn is_scope_current(&self, scope: &SessionScope) -> bool {
        scope_matches(&self.lock_state(), scope)
    }

    pub(crate) fn access_token_for_scope(
        &self,
        scope: &SessionScope,
    ) -> Result<String, ApiError> {
        let mut state = self.lock_state();
        if !scope_matches(&state, scope) {
            return Err(ApiError::AuthenticationRequired);
        }
        if let Some(access_token) = state.access_token.as_ref() {
            return Ok(access_token.clone());
        }
        if !state.refreshing {
            return Err(ApiError::AuthenticationRequired);
        }
        let observed_refresh_epoch = state.refresh_epoch;
        while state.refreshing
            && state.refresh_epoch == observed_refresh_epoch
            && scope_matches(&state, scope)
        {
            state = self
                .refresh_finished
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if !scope_matches(&state, scope) {
            return Err(ApiError::AuthenticationRequired);
        }
        if let Some(access_token) = state.access_token.as_ref() {
            return Ok(access_token.clone());
        }
        state
            .last_refresh_result
            .clone()
            .unwrap_or(Err(ApiError::AuthenticationRequired))
    }

    pub(crate) fn clear_scope(&self, scope: &SessionScope) -> Result<(), ApiError> {
        let mut state = self.lock_state();
        if cleared_lease_matches_epoch(&state, scope.auth_epoch) {
            return self.store.clear();
        }
        if !scope_matches(&state, scope) {
            return Err(ApiError::AuthenticationRequired);
        }
        invalidate_session_lease(&mut state);
        let clear_result = self.store.clear();
        self.refresh_finished.notify_all();
        clear_result
    }

    pub(crate) fn clear_epoch(&self, auth_epoch: u64) -> Result<(), ApiError> {
        let mut state = self.lock_state();
        if cleared_lease_matches_epoch(&state, auth_epoch) {
            return self.store.clear();
        }
        if state.auth_epoch != auth_epoch {
            return Err(ApiError::AuthenticationRequired);
        }
        invalidate_session_lease(&mut state);
        let clear_result = self.store.clear();
        self.refresh_finished.notify_all();
        clear_result
    }

    pub(crate) fn refresh<F>(
        &self,
        rejected_access_token: Option<&str>,
        refresh: F,
    ) -> Result<String, ApiError>
    where
        F: FnOnce(&str) -> Result<TokenSet, ApiError>,
    {
        let (auth_epoch, refresh_token) = {
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

            let refresh_token = self
                .store
                .load()?
                .ok_or(ApiError::AuthenticationRequired)?;
            state.refreshing = true;
            state.access_token = None;
            state.last_refresh_result = None;
            (state.auth_epoch, refresh_token)
        };

        let refreshed = refresh(&refresh_token);
        let mut state = self.lock_state();
        if state.auth_epoch != auth_epoch {
            return Err(ApiError::AuthenticationRequired);
        }
        let result = match refreshed {
            Ok(tokens) => self
                .store
                .save(&tokens.refresh_token)
                .map(|()| tokens.access_token),
            Err(error) => Err(error),
        };
        state.refreshing = false;
        state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
        state.access_token = result.clone().ok();
        state.last_refresh_result = Some(result.clone());
        self.refresh_finished.notify_all();
        result
    }

    pub(crate) fn refresh_scope<F>(
        &self,
        scope: &SessionScope,
        rejected_access_token: Option<&str>,
        refresh: F,
    ) -> Result<String, ApiError>
    where
        F: FnOnce(&str) -> Result<TokenSet, ApiError>,
    {
        let refresh_token = {
            let mut state = self.lock_state();
            if !scope_matches(&state, scope) {
                return Err(ApiError::AuthenticationRequired);
            }
            if let (Some(rejected), Some(current)) =
                (rejected_access_token, state.access_token.as_deref())
            {
                if current != rejected {
                    return Ok(current.to_string());
                }
            }

            if state.refreshing {
                let observed_refresh_epoch = state.refresh_epoch;
                while state.refreshing
                    && state.refresh_epoch == observed_refresh_epoch
                    && scope_matches(&state, scope)
                {
                    state = self
                        .refresh_finished
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                if !scope_matches(&state, scope) {
                    return Err(ApiError::AuthenticationRequired);
                }
                return state
                    .last_refresh_result
                    .clone()
                    .unwrap_or(Err(ApiError::AuthenticationRequired));
            }

            let refresh_token = self
                .store
                .load()?
                .ok_or(ApiError::AuthenticationRequired)?;
            state.refreshing = true;
            state.access_token = None;
            state.last_refresh_result = None;
            refresh_token
        };

        let refreshed = refresh(&refresh_token);
        let mut state = self.lock_state();
        if !scope_matches(&state, scope) {
            return Err(ApiError::AuthenticationRequired);
        }
        let result = match refreshed {
            Ok(tokens) => self
                .store
                .save(&tokens.refresh_token)
                .map(|()| tokens.access_token),
            Err(error) => Err(error),
        };
        state.refreshing = false;
        state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
        state.access_token = result.clone().ok();
        state.last_refresh_result = Some(result.clone());
        self.refresh_finished.notify_all();
        result
    }

    pub(crate) fn refresh_epoch<F>(
        &self,
        auth_epoch: u64,
        rejected_access_token: Option<&str>,
        refresh: F,
    ) -> Result<String, ApiError>
    where
        F: FnOnce(&str) -> Result<TokenSet, ApiError>,
    {
        let refresh_token = {
            let mut state = self.lock_state();
            if state.auth_epoch != auth_epoch {
                return Err(ApiError::AuthenticationRequired);
            }
            if let (Some(rejected), Some(current)) =
                (rejected_access_token, state.access_token.as_deref())
            {
                if current != rejected {
                    return Ok(current.to_string());
                }
            }

            if state.refreshing {
                let observed_refresh_epoch = state.refresh_epoch;
                while state.refreshing
                    && state.refresh_epoch == observed_refresh_epoch
                    && state.auth_epoch == auth_epoch
                {
                    state = self
                        .refresh_finished
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                if state.auth_epoch != auth_epoch {
                    return Err(ApiError::AuthenticationRequired);
                }
                return state
                    .last_refresh_result
                    .clone()
                    .unwrap_or(Err(ApiError::AuthenticationRequired));
            }

            let refresh_token = self
                .store
                .load()?
                .ok_or(ApiError::AuthenticationRequired)?;
            state.refreshing = true;
            state.access_token = None;
            state.last_refresh_result = None;
            refresh_token
        };

        let refreshed = refresh(&refresh_token);
        let mut state = self.lock_state();
        if state.auth_epoch != auth_epoch {
            return Err(ApiError::AuthenticationRequired);
        }
        let result = match refreshed {
            Ok(tokens) => self
                .store
                .save(&tokens.refresh_token)
                .map(|()| tokens.access_token),
            Err(error) => Err(error),
        };
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

fn scope_matches(state: &SessionState, scope: &SessionScope) -> bool {
    state.auth_epoch == scope.auth_epoch
        && state.owner_user_id.as_deref() == Some(scope.owner_user_id.as_str())
}

fn invalidate_session_lease(state: &mut SessionState) {
    state.auth_epoch = state.auth_epoch.wrapping_add(1);
    state.access_token = None;
    state.owner_user_id = None;
    state.refreshing = false;
    state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
    state.last_refresh_result = None;
}

fn cleared_lease_matches_epoch(state: &SessionState, cleared_auth_epoch: u64) -> bool {
    state.auth_epoch == cleared_auth_epoch.wrapping_add(1)
        && state.access_token.is_none()
        && state.owner_user_id.is_none()
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

    #[derive(Default)]
    struct FailingClearStore {
        value: Mutex<Option<String>>,
    }

    impl RefreshTokenStore for FailingClearStore {
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
            Err(ApiError::Credential {
                message: "simulated keychain failure".to_string(),
            })
        }
    }

    #[test]
    fn installing_tokens_persists_refresh_and_keeps_access_in_memory() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = SessionManager::new(store.clone());
        manager
            .install_tokens(&tokens("access-1", "refresh-1"))
            .unwrap();

        assert_eq!(manager.access_token().as_deref(), Some("access-1"));
        assert_eq!(store.load().unwrap().as_deref(), Some("refresh-1"));
    }

    #[test]
    fn scoped_access_token_never_borrows_a_new_accounts_token() {
        let manager = SessionManager::new(Arc::new(MemoryRefreshTokenStore::default()));
        let scope_a = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        let captured_a = manager.access_token_for_scope(&scope_a).unwrap();

        let scope_b = manager
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();

        assert_eq!(captured_a, "access-a");
        assert!(matches!(
            manager.access_token_for_scope(&scope_a),
            Err(ApiError::AuthenticationRequired)
        ));
        assert_eq!(
            manager.access_token_for_scope(&scope_b).unwrap(),
            "access-b"
        );
        assert!(scope_b.auth_epoch > scope_a.auth_epoch);
    }

    #[test]
    fn clear_failure_still_invalidates_the_captured_memory_lease() {
        let store = Arc::new(FailingClearStore::default());
        let manager = SessionManager::new(store.clone());
        let scope = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();

        let result = manager.clear_scope(&scope);

        assert!(matches!(result, Err(ApiError::Credential { .. })));
        assert!(manager.access().is_none());
        assert_eq!(manager.auth_epoch(), scope.auth_epoch.wrapping_add(1));
        assert!(!manager.is_scope_current(&scope));
        assert!(matches!(
            manager.access_token_for_scope(&scope),
            Err(ApiError::AuthenticationRequired)
        ));
        assert_eq!(store.load().unwrap().as_deref(), Some("refresh-a"));
    }

    #[test]
    fn scope_remains_current_while_its_access_token_is_temporarily_absent() {
        let manager = SessionManager::new(Arc::new(MemoryRefreshTokenStore::default()));
        let scope = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();

        manager.clear_access_token();

        assert!(manager.is_scope_current(&scope));
        assert_eq!(manager.scope_for_user("user-a"), Some(scope.clone()));
        assert!(matches!(
            manager.access_token_for_scope(&scope),
            Err(ApiError::AuthenticationRequired)
        ));
    }

    #[test]
    fn scoped_token_reader_waits_for_an_inflight_refresh() {
        let manager = Arc::new(SessionManager::new(Arc::new(
            MemoryRefreshTokenStore::default(),
        )));
        let scope = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        let (refresh_started_tx, refresh_started_rx) = std::sync::mpsc::channel();
        let (continue_refresh_tx, continue_refresh_rx) = std::sync::mpsc::channel();
        let refresh_manager = manager.clone();
        let refresh_scope = scope.clone();
        let refresh_worker = thread::spawn(move || {
            refresh_manager.refresh_scope(&refresh_scope, Some("access-a"), |_| {
                refresh_started_tx.send(()).unwrap();
                continue_refresh_rx.recv().unwrap();
                Ok(tokens("access-a-rotated", "refresh-a-rotated"))
            })
        });
        refresh_started_rx.recv().unwrap();

        let (reader_tx, reader_rx) = std::sync::mpsc::channel();
        let reader_manager = manager.clone();
        let reader_scope = scope.clone();
        let reader = thread::spawn(move || {
            let _ = reader_tx.send(reader_manager.access_token_for_scope(&reader_scope));
        });
        assert!(reader_rx.recv_timeout(Duration::from_millis(20)).is_err());
        continue_refresh_tx.send(()).unwrap();

        assert_eq!(refresh_worker.join().unwrap().unwrap(), "access-a-rotated");
        assert_eq!(
            reader_rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
            "access-a-rotated"
        );
        reader.join().unwrap();
    }

    #[test]
    fn stale_scope_is_rejected_before_refresh_callback_runs() {
        let manager = SessionManager::new(Arc::new(MemoryRefreshTokenStore::default()));
        let scope_a = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        manager
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();
        let calls = AtomicUsize::new(0);

        let result = manager.refresh_scope(&scope_a, Some("access-a"), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(tokens("unexpected", "unexpected"))
        });

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stale_unscoped_request_epoch_cannot_refresh_with_a_new_accounts_token() {
        let manager = SessionManager::new(Arc::new(MemoryRefreshTokenStore::default()));
        manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        let request_access = manager.access().unwrap();
        manager
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();
        let calls = AtomicUsize::new(0);

        let result = manager.refresh_epoch(
            request_access.auth_epoch,
            Some(&request_access.access_token),
            |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(tokens("unexpected", "unexpected"))
            },
        );

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(manager.access_token().as_deref(), Some("access-b"));
    }

    #[test]
    fn stale_refresh_response_cannot_overwrite_a_new_login() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = Arc::new(SessionManager::new(store.clone()));
        let scope_a = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let worker_manager = manager.clone();
        let worker_scope = scope_a.clone();
        let worker = thread::spawn(move || {
            worker_manager.refresh_scope(&worker_scope, Some("access-a"), |refresh_token| {
                assert_eq!(refresh_token, "refresh-a");
                started_tx.send(()).unwrap();
                continue_rx.recv().unwrap();
                Ok(tokens("access-a-rotated", "refresh-a-rotated"))
            })
        });

        started_rx.recv().unwrap();
        assert!(manager.is_scope_current(&scope_a));
        assert_eq!(manager.scope_for_user("user-a"), Some(scope_a.clone()));
        let scope_b = manager
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();
        continue_tx.send(()).unwrap();

        assert!(matches!(
            worker.join().unwrap(),
            Err(ApiError::AuthenticationRequired)
        ));
        assert_eq!(
            manager.access_token_for_scope(&scope_b).unwrap(),
            "access-b"
        );
        assert_eq!(store.load().unwrap().as_deref(), Some("refresh-b"));
    }

    #[test]
    fn transient_refresh_failure_preserves_the_lease_and_refresh_token_for_retry() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = SessionManager::new(store.clone());
        let scope = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();

        let first = manager.refresh_scope(&scope, Some("access-a"), |_| {
            Err(ApiError::Network {
                message: "temporarily offline".to_string(),
                timeout: false,
            })
        });

        assert!(matches!(first, Err(ApiError::Network { .. })));
        assert!(manager.is_scope_current(&scope));
        assert!(manager.access().is_none());
        assert_eq!(store.load().unwrap().as_deref(), Some("refresh-a"));

        let recovered = manager
            .refresh_scope(&scope, None, |refresh_token| {
                assert_eq!(refresh_token, "refresh-a");
                Ok(tokens("access-a-recovered", "refresh-a-rotated"))
            })
            .unwrap();

        assert_eq!(recovered, "access-a-recovered");
        assert_eq!(
            manager.access_token_for_scope(&scope).unwrap(),
            "access-a-recovered"
        );
        assert_eq!(
            store.load().unwrap().as_deref(),
            Some("refresh-a-rotated")
        );
    }

    #[test]
    fn persisted_refresh_token_is_available_to_a_fresh_session_manager() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let first = SessionManager::new(store.clone());
        first
            .install_tokens(&tokens("access-1", "refresh-1"))
            .unwrap();
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
        let dir =
            std::env::temp_dir().join(format!("artforge-session-test-{}", uuid::Uuid::new_v4()));
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
