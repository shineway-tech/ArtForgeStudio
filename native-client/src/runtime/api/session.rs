use super::{ApiError, TokenSet};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

#[cfg(all(unix, test))]
use std::os::unix::fs::PermissionsExt;

const SESSION_DIR: &str = "session";
const REFRESH_SESSION_FILE: &str = "refresh-session.json";
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedRefreshSession {
    pub(crate) owner_user_id: String,
    pub(crate) refresh_token: String,
}

pub(crate) trait RefreshTokenStore: Send + Sync {
    fn load(&self) -> Result<Option<PersistedRefreshSession>, ApiError>;
    fn save(&self, session: &PersistedRefreshSession) -> Result<(), ApiError>;
    fn clear(&self) -> Result<(), ApiError>;
    fn replace_if_current(
        &self,
        expected: &PersistedRefreshSession,
        replacement: &PersistedRefreshSession,
    ) -> Result<StoreMutation, ApiError>;
    fn clear_if_current(
        &self,
        expected: &PersistedRefreshSession,
    ) -> Result<StoreMutation, ApiError>;
}

pub(crate) enum StoreMutation {
    Applied,
    Conflict(Option<PersistedRefreshSession>),
}

pub(crate) struct FileRefreshTokenStore {
    data_dir: PathBuf,
    #[cfg(test)]
    path: PathBuf,
    #[cfg(test)]
    temporary_path_override: Option<PathBuf>,
}

impl FileRefreshTokenStore {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            #[cfg(test)]
            path: data_dir.join(SESSION_DIR).join(REFRESH_SESSION_FILE),
            #[cfg(test)]
            temporary_path_override: None,
        }
    }

    #[cfg(test)]
    fn with_temporary_path_for_test(data_dir: &Path, temporary_path: PathBuf) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            path: data_dir.join(SESSION_DIR).join(REFRESH_SESSION_FILE),
            temporary_path_override: Some(temporary_path),
        }
    }

    fn temporary_name(&self) -> Result<OsString, ApiError> {
        #[cfg(test)]
        if let Some(path) = self.temporary_path_override.as_ref() {
            return path
                .file_name()
                .map(|name| name.to_os_string())
                .ok_or_else(|| local_state_message("刷新会话临时文件名无效"));
        }
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Ok(OsString::from(format!(
            ".{REFRESH_SESSION_FILE}.{}.{}.tmp",
            std::process::id(),
            sequence
        )))
    }

    fn validate_and_serialize(
        session: &PersistedRefreshSession,
    ) -> Result<Vec<u8>, ApiError> {
        let canonical_owner = canonical_owner_user_id(&session.owner_user_id)?;
        if canonical_owner != session.owner_user_id || session.refresh_token.trim().is_empty() {
            return Err(local_state_message("拒绝保存无效的刷新会话"));
        }
        serde_json::to_vec(session).map_err(|_| local_state_message("序列化刷新会话失败"))
    }

    fn parse(value: &[u8]) -> Result<PersistedRefreshSession, ApiError> {
        let session: PersistedRefreshSession = serde_json::from_slice(value)
            .map_err(|_| protocol_error("本地刷新会话格式无效"))?;
        let canonical_owner = canonical_owner_user_id(&session.owner_user_id)?;
        if canonical_owner != session.owner_user_id || session.refresh_token.trim().is_empty() {
            return Err(protocol_error("本地刷新会话格式无效"));
        }
        Ok(session)
    }
}

impl RefreshTokenStore for FileRefreshTokenStore {
    fn load(&self) -> Result<Option<PersistedRefreshSession>, ApiError> {
        let Some(store) = file_store_platform::LockedStore::open(&self.data_dir, false)? else {
            return Ok(None);
        };
        store
            .load()?
            .map(|value| Self::parse(&value))
            .transpose()
    }

    fn save(&self, session: &PersistedRefreshSession) -> Result<(), ApiError> {
        let serialized = Self::validate_and_serialize(session)?;
        let store = file_store_platform::LockedStore::open(&self.data_dir, true)?
            .ok_or_else(|| local_state_message("创建登录状态目录失败"))?;
        store.replace(&self.temporary_name()?, &serialized)
    }

    fn clear(&self) -> Result<(), ApiError> {
        if let Some(store) = file_store_platform::LockedStore::open(&self.data_dir, false)? {
            let current = store
                .load()?
                .map(|value| Self::parse(&value))
                .transpose()?;
            if current.is_some() {
                store.remove()?;
            }
        }
        Ok(())
    }

    fn replace_if_current(
        &self,
        expected: &PersistedRefreshSession,
        replacement: &PersistedRefreshSession,
    ) -> Result<StoreMutation, ApiError> {
        let serialized = Self::validate_and_serialize(replacement)?;
        let Some(store) = file_store_platform::LockedStore::open(&self.data_dir, false)? else {
            return Ok(StoreMutation::Conflict(None));
        };
        let current = store
            .load()?
            .map(|value| Self::parse(&value))
            .transpose()?;
        if current.as_ref() != Some(expected) {
            return Ok(StoreMutation::Conflict(current));
        }
        store.replace(&self.temporary_name()?, &serialized)?;
        Ok(StoreMutation::Applied)
    }

    fn clear_if_current(
        &self,
        expected: &PersistedRefreshSession,
    ) -> Result<StoreMutation, ApiError> {
        let Some(store) = file_store_platform::LockedStore::open(&self.data_dir, false)? else {
            return Ok(StoreMutation::Conflict(None));
        };
        let current = store
            .load()?
            .map(|value| Self::parse(&value))
            .transpose()?;
        if current.as_ref() != Some(expected) {
            return Ok(StoreMutation::Conflict(current));
        }
        store.remove()?;
        Ok(StoreMutation::Applied)
    }
}

fn canonical_owner_user_id(owner_user_id: &str) -> Result<String, ApiError> {
    uuid::Uuid::parse_str(owner_user_id)
        .map(|owner| owner.to_string())
        .map_err(|_| protocol_error("本地刷新会话所有者无效"))
}

fn protocol_error(message: &str) -> ApiError {
    ApiError::Protocol {
        message: message.to_string(),
        request_id: None,
    }
}

fn local_state_error(action: &str, error: std::io::Error) -> ApiError {
    ApiError::LocalState {
        message: format!("{action}失败：{error}"),
    }
}

fn local_state_message(message: &str) -> ApiError {
    ApiError::LocalState {
        message: message.to_string(),
    }
}

#[cfg(unix)]
mod file_store_platform {
    use super::{local_state_error, local_state_message, REFRESH_SESSION_FILE, SESSION_DIR};
    use crate::runtime::api::ApiError;
    use rustix::fd::OwnedFd;
    use rustix::fs::{
        self as rfs, AtFlags, FileType, FlockOperation, Mode, OFlags,
    };
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::{Component, Path};

    const LOCK_FILE: &str = ".refresh-session.lock";

    #[derive(Clone, Copy, Eq, PartialEq)]
    struct FileIdentity {
        device: u64,
        inode: u64,
    }

    pub(super) struct LockedStore {
        directory: OwnedFd,
        _lock: OwnedFd,
    }

    impl LockedStore {
        pub(super) fn open(
            data_dir: &Path,
            create: bool,
        ) -> Result<Option<Self>, ApiError> {
            let Some(data_root) = open_absolute_data_root(data_dir, create)? else {
                return Ok(None);
            };
            require_directory(&data_root, "应用数据目录不是普通目录")?;

            let directory = match open_session_directory(&data_root) {
                Ok(directory) => directory,
                Err(rustix::io::Errno::NOENT) if !create => return Ok(None),
                Err(rustix::io::Errno::NOENT) => {
                    match rfs::mkdirat(
                        &data_root,
                        SESSION_DIR,
                        Mode::from_bits_truncate(0o700),
                    ) {
                        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                        Err(error) => {
                            return Err(local_state_error(
                                "创建登录状态目录",
                                error.into(),
                            ));
                        }
                    }
                    open_session_directory(&data_root)
                        .map_err(|error| local_state_error("打开登录状态目录", error.into()))?
                }
                Err(error) => {
                    return Err(local_state_error("打开登录状态目录", error.into()));
                }
            };
            require_directory(&directory, "登录状态目录不是普通目录")?;
            rfs::fchmod(&directory, Mode::from_bits_truncate(0o700))
                .map_err(|error| local_state_error("设置登录状态目录权限", error.into()))?;

            let lock = rfs::openat(
                &directory,
                LOCK_FILE,
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC
                    | OFlags::NONBLOCK,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|error| local_state_error("打开刷新会话锁", error.into()))?;
            require_regular(&lock, "刷新会话锁不是独占普通文件")?;
            rfs::fchmod(&lock, Mode::from_bits_truncate(0o600))
                .map_err(|error| local_state_error("设置刷新会话锁权限", error.into()))?;
            rfs::flock(&lock, FlockOperation::LockExclusive)
                .map_err(|error| local_state_error("锁定刷新会话", error.into()))?;

            Ok(Some(Self {
                directory,
                _lock: lock,
            }))
        }

        pub(super) fn load(&self) -> Result<Option<Vec<u8>>, ApiError> {
            let descriptor = match rfs::openat(
                &self.directory,
                REFRESH_SESSION_FILE,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            ) {
                Ok(descriptor) => descriptor,
                Err(rustix::io::Errno::NOENT) => return Ok(None),
                Err(error) => {
                    return Err(local_state_error("打开刷新会话", error.into()));
                }
            };
            require_regular(&descriptor, "刷新会话不是独占普通文件")?;
            rfs::fchmod(&descriptor, Mode::from_bits_truncate(0o600))
                .map_err(|error| local_state_error("设置刷新令牌文件权限", error.into()))?;
            let mut file = File::from(descriptor);
            let mut value = Vec::new();
            file.read_to_end(&mut value)
                .map_err(|error| local_state_error("读取刷新会话", error))?;
            Ok(Some(value))
        }

        pub(super) fn replace(
            &self,
            temporary_name: &OsStr,
            value: &[u8],
        ) -> Result<(), ApiError> {
            require_leaf_name(temporary_name)?;
            let descriptor = rfs::openat(
                &self.directory,
                temporary_name,
                OFlags::WRONLY
                    | OFlags::CREATE
                    | OFlags::EXCL
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|error| local_state_error("创建刷新会话临时文件", error.into()))?;
            let identity = require_regular(&descriptor, "刷新会话临时文件不是独占普通文件")?;
            if let Err(error) = rfs::fchmod(&descriptor, Mode::from_bits_truncate(0o600)) {
                drop(descriptor);
                self.cleanup_created_temporary(temporary_name, identity);
                return Err(local_state_error(
                    "设置刷新会话临时文件权限",
                    error.into(),
                ));
            }

            let mut file = File::from(descriptor);
            let write_result = file
                .write_all(value)
                .and_then(|()| file.sync_all())
                .map_err(|error| local_state_error("写入并同步刷新会话", error));
            drop(file);
            if let Err(error) = write_result {
                self.cleanup_created_temporary(temporary_name, identity);
                return Err(error);
            }

            if let Err(error) = rfs::renameat(
                &self.directory,
                temporary_name,
                &self.directory,
                REFRESH_SESSION_FILE,
            ) {
                self.cleanup_created_temporary(temporary_name, identity);
                return Err(local_state_error("保存刷新会话", error.into()));
            }
            // There are deliberately no fallible operations after rename. An Err from this
            // method therefore always means the replacement was not committed.
            Ok(())
        }

        pub(super) fn remove(&self) -> Result<(), ApiError> {
            match rfs::unlinkat(&self.directory, REFRESH_SESSION_FILE, AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
                Err(error) => Err(local_state_error("删除刷新会话", error.into())),
            }
        }

        fn cleanup_created_temporary(&self, name: &OsStr, expected: FileIdentity) {
            let Ok(descriptor) = rfs::openat(
                &self.directory,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            ) else {
                return;
            };
            let Ok(actual) = file_identity(&descriptor) else {
                return;
            };
            if actual == expected {
                let _ = rfs::unlinkat(&self.directory, name, AtFlags::empty());
            }
        }
    }

    fn open_session_directory(data_root: &OwnedFd) -> rustix::io::Result<OwnedFd> {
        rfs::openat(
            data_root,
            SESSION_DIR,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
    }

    fn open_absolute_data_root(
        path: &Path,
        required: bool,
    ) -> Result<Option<OwnedFd>, ApiError> {
        if !path.is_absolute() {
            return Err(local_state_message("应用数据目录必须是绝对路径"));
        }
        let mut directory = rfs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| local_state_error("打开文件系统根目录", error.into()))?;
        for component in path.components() {
            let name = match component {
                Component::RootDir => continue,
                Component::Normal(name) => name,
                _ => return Err(local_state_message("应用数据目录包含非规范路径组件")),
            };
            directory = match rfs::openat(
                &directory,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(next) => next,
                Err(rustix::io::Errno::NOENT) if !required => return Ok(None),
                Err(error) => {
                    return Err(local_state_error("打开应用数据目录组件", error.into()));
                }
            };
            require_directory(&directory, "应用数据目录组件不是普通目录")?;
        }
        Ok(Some(directory))
    }

    fn require_directory(descriptor: &OwnedFd, message: &str) -> Result<(), ApiError> {
        let stat = rfs::fstat(descriptor)
            .map_err(|error| local_state_error("检查登录状态目录", error.into()))?;
        if !FileType::from_raw_mode(stat.st_mode).is_dir() {
            return Err(local_state_message(message));
        }
        Ok(())
    }

    fn require_regular(
        descriptor: &OwnedFd,
        message: &str,
    ) -> Result<FileIdentity, ApiError> {
        let stat = rfs::fstat(descriptor)
            .map_err(|error| local_state_error("检查刷新会话文件", error.into()))?;
        if !FileType::from_raw_mode(stat.st_mode).is_file() || stat.st_nlink != 1 {
            return Err(local_state_message(message));
        }
        Ok(FileIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
        })
    }

    fn file_identity(descriptor: &OwnedFd) -> Result<FileIdentity, ApiError> {
        let stat = rfs::fstat(descriptor)
            .map_err(|error| local_state_error("检查刷新会话临时文件", error.into()))?;
        Ok(FileIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
        })
    }

    fn require_leaf_name(name: &OsStr) -> Result<(), ApiError> {
        let mut components = Path::new(name).components();
        if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
            Ok(())
        } else {
            Err(local_state_message("刷新会话临时文件名无效"))
        }
    }
}

#[cfg(windows)]
mod file_store_platform {
    use super::{local_state_error, local_state_message, REFRESH_SESSION_FILE, SESSION_DIR};
    use crate::runtime::api::ApiError;
    use std::ffi::{c_void, OsStr};
    use std::fs::File;
    use std::io::{ErrorKind, Read, Write};
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use std::path::{Component, Path, PathBuf};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, GetFileInformationByHandleEx, LockFileEx,
        SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ADD_FILE,
        FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_READ_DATA, FILE_RENAME_INFO, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_WRITE_DATA, FileAttributeTagInfo,
        FileDispositionInfo, LOCKFILE_EXCLUSIVE_LOCK, OPEN_EXISTING, SYNCHRONIZE,
        UnlockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    const LOCK_FILE: &str = ".refresh-session.lock";
    const FILE_OPEN: u32 = 1;
    const FILE_CREATE: u32 = 2;
    const FILE_OPEN_IF: u32 = 3;
    const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
    const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
    const FILE_OPEN_REPARSE_POINT_NT: u32 = 0x0020_0000;
    const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;
    const FILE_RENAME_INFORMATION: u32 = 10;
    const ANCESTOR_DIRECTORY_ACCESS: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
    const DATA_ROOT_DIRECTORY_ACCESS: u32 =
        ANCESTOR_DIRECTORY_ACCESS | FILE_ADD_SUBDIRECTORY;
    const SESSION_DIRECTORY_ACCESS: u32 = ANCESTOR_DIRECTORY_ACCESS | FILE_ADD_FILE;
    const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    const SHARE_LOCK: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: HANDLE,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }

    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: usize,
        information: usize,
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtCreateFile(
            file_handle: *mut HANDLE,
            desired_access: u32,
            object_attributes: *mut ObjectAttributes,
            io_status_block: *mut IoStatusBlock,
            allocation_size: *mut i64,
            file_attributes: u32,
            share_access: u32,
            create_disposition: u32,
            create_options: u32,
            ea_buffer: *mut c_void,
            ea_length: u32,
        ) -> i32;
        fn NtSetInformationFile(
            file_handle: HANDLE,
            io_status_block: *mut IoStatusBlock,
            file_information: *const c_void,
            length: u32,
            information_class: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }

    pub(super) struct LockedStore {
        directory: OwnedHandle,
        _lock: File,
    }

    impl Drop for LockedStore {
        fn drop(&mut self) {
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            let _ = unsafe {
                UnlockFileEx(
                    self._lock.as_raw_handle() as HANDLE,
                    0,
                    u32::MAX,
                    u32::MAX,
                    &mut overlapped,
                )
            };
        }
    }

    impl LockedStore {
        pub(super) fn open(
            data_dir: &Path,
            create: bool,
        ) -> Result<Option<Self>, ApiError> {
            let Some(data_root) = open_absolute_data_root(data_dir, create)? else {
                return Ok(None);
            };
            let directory = match nt_open_relative(
                &data_root,
                OsStr::new(SESSION_DIR),
                SESSION_DIRECTORY_ACCESS,
                SHARE_ALL,
                if create { FILE_OPEN_IF } else { FILE_OPEN },
                FILE_DIRECTORY_FILE
                    | FILE_SYNCHRONOUS_IO_NONALERT
                    | FILE_OPEN_REPARSE_POINT_NT,
                FILE_ATTRIBUTE_DIRECTORY,
            ) {
                Ok(directory) => directory,
                Err(error) if error.kind() == ErrorKind::NotFound && !create => return Ok(None),
                Err(error) => return Err(local_state_error("打开登录状态目录", error)),
            };
            require_directory(&directory, "登录状态目录不是非重解析普通目录")?;

            let lock_handle = nt_open_relative(
                &directory,
                OsStr::new(LOCK_FILE),
                FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                SHARE_LOCK,
                FILE_OPEN_IF,
                FILE_NON_DIRECTORY_FILE
                    | FILE_SYNCHRONOUS_IO_NONALERT
                    | FILE_OPEN_REPARSE_POINT_NT,
                FILE_ATTRIBUTE_NORMAL,
            )
            .map_err(|error| local_state_error("打开刷新会话锁", error))?;
            require_regular(&lock_handle, "刷新会话锁不是独占普通文件")?;
            let lock = File::from(lock_handle);
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            let locked = unsafe {
                LockFileEx(
                    lock.as_raw_handle() as HANDLE,
                    LOCKFILE_EXCLUSIVE_LOCK,
                    0,
                    u32::MAX,
                    u32::MAX,
                    &mut overlapped,
                )
            };
            if locked == 0 {
                return Err(local_state_error(
                    "锁定刷新会话",
                    std::io::Error::last_os_error(),
                ));
            }
            Ok(Some(Self {
                directory,
                _lock: lock,
            }))
        }

        pub(super) fn load(&self) -> Result<Option<Vec<u8>>, ApiError> {
            let handle = match nt_open_relative(
                &self.directory,
                OsStr::new(REFRESH_SESSION_FILE),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                SHARE_ALL,
                FILE_OPEN,
                FILE_NON_DIRECTORY_FILE
                    | FILE_SYNCHRONOUS_IO_NONALERT
                    | FILE_OPEN_REPARSE_POINT_NT,
                FILE_ATTRIBUTE_NORMAL,
            ) {
                Ok(handle) => handle,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(local_state_error("打开刷新会话", error)),
            };
            require_regular(&handle, "刷新会话不是独占普通文件")?;
            let mut file = File::from(handle);
            let mut value = Vec::new();
            file.read_to_end(&mut value)
                .map_err(|error| local_state_error("读取刷新会话", error))?;
            Ok(Some(value))
        }

        pub(super) fn replace(
            &self,
            temporary_name: &OsStr,
            value: &[u8],
        ) -> Result<(), ApiError> {
            require_leaf_name(temporary_name)?;
            let handle = nt_open_relative(
                &self.directory,
                temporary_name,
                FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
                SHARE_ALL,
                FILE_CREATE,
                FILE_NON_DIRECTORY_FILE
                    | FILE_SYNCHRONOUS_IO_NONALERT
                    | FILE_OPEN_REPARSE_POINT_NT,
                FILE_ATTRIBUTE_NORMAL,
            )
            .map_err(|error| local_state_error("创建刷新会话临时文件", error))?;
            require_regular(&handle, "刷新会话临时文件不是独占普通文件")?;
            let mut file = File::from(handle);
            if let Err(error) = file.write_all(value).and_then(|()| file.sync_all()) {
                delete_open_file(&file);
                return Err(local_state_error("写入并同步刷新会话", error));
            }
            if let Err(error) = rename_open_file(
                &file,
                &self.directory,
                OsStr::new(REFRESH_SESSION_FILE),
            ) {
                delete_open_file(&file);
                return Err(local_state_error("保存刷新会话", error));
            }
            // There are deliberately no fallible operations after rename. An Err from this
            // method therefore always means the replacement was not committed.
            Ok(())
        }

        pub(super) fn remove(&self) -> Result<(), ApiError> {
            let handle = match nt_open_relative(
                &self.directory,
                OsStr::new(REFRESH_SESSION_FILE),
                FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
                SHARE_ALL,
                FILE_OPEN,
                FILE_NON_DIRECTORY_FILE
                    | FILE_SYNCHRONOUS_IO_NONALERT
                    | FILE_OPEN_REPARSE_POINT_NT,
                FILE_ATTRIBUTE_NORMAL,
            ) {
                Ok(handle) => handle,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(local_state_error("打开待删除刷新会话", error)),
            };
            require_regular(&handle, "待删除刷新会话不是独占普通文件")?;
            let file = File::from(handle);
            mark_delete(&file).map_err(|error| local_state_error("删除刷新会话", error))
        }
    }

    fn open_absolute_data_root(
        path: &Path,
        required: bool,
    ) -> Result<Option<OwnedHandle>, ApiError> {
        if !path.is_absolute() {
            return Err(local_state_message("应用数据目录必须是绝对路径"));
        }
        let mut components = path.components();
        let prefix = match components.next() {
            Some(Component::Prefix(prefix)) => prefix,
            _ => return Err(local_state_message("应用数据目录缺少 Windows 根前缀")),
        };
        if !matches!(components.next(), Some(Component::RootDir)) {
            return Err(local_state_message("应用数据目录缺少 Windows 根目录"));
        }
        let mut anchor = PathBuf::from(prefix.as_os_str());
        anchor.push("\\");
        let names: Vec<_> = components
            .map(|component| match component {
                Component::Normal(name) => Ok(name.to_os_string()),
                _ => Err(local_state_message("应用数据目录包含非规范路径组件")),
            })
            .collect::<Result<_, _>>()?;
        let anchor_access = if names.is_empty() {
            DATA_ROOT_DIRECTORY_ACCESS
        } else {
            ANCESTOR_DIRECTORY_ACCESS
        };
        let mut directory = open_anchor(&anchor, anchor_access)
            .map_err(|error| local_state_error("打开 Windows 文件系统根目录", error))?;
        require_directory(&directory, "Windows 文件系统根目录是重解析点")?;

        for (index, name) in names.iter().enumerate() {
            let desired_access = if index + 1 == names.len() {
                DATA_ROOT_DIRECTORY_ACCESS
            } else {
                ANCESTOR_DIRECTORY_ACCESS
            };
            directory = match nt_open_relative(
                &directory,
                name,
                desired_access,
                SHARE_ALL,
                FILE_OPEN,
                FILE_DIRECTORY_FILE
                    | FILE_SYNCHRONOUS_IO_NONALERT
                    | FILE_OPEN_REPARSE_POINT_NT,
                FILE_ATTRIBUTE_DIRECTORY,
            ) {
                Ok(next) => next,
                Err(error) if error.kind() == ErrorKind::NotFound && !required => return Ok(None),
                Err(error) => return Err(local_state_error("打开应用数据目录组件", error)),
            };
            require_directory(&directory, "应用数据目录组件是重解析点或非目录")?;
        }
        Ok(Some(directory))
    }

    fn open_anchor(path: &Path, desired_access: u32) -> std::io::Result<OwnedHandle> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                desired_access,
                SHARE_ALL,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
        }
    }

    fn nt_open_relative(
        parent: &OwnedHandle,
        name: &OsStr,
        desired_access: u32,
        share_access: u32,
        disposition: u32,
        options: u32,
        attributes: u32,
    ) -> std::io::Result<OwnedHandle> {
        require_leaf_name_io(name)?;
        let mut wide: Vec<u16> = name.encode_wide().collect();
        let byte_length = wide
            .len()
            .checked_mul(2)
            .and_then(|length| u16::try_from(length).ok())
            .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "name is too long"))?;
        let mut unicode = UnicodeString {
            length: byte_length,
            maximum_length: byte_length,
            buffer: wide.as_mut_ptr(),
        };
        let mut object_attributes = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root_directory: parent.as_raw_handle() as HANDLE,
            object_name: &mut unicode,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: null_mut(),
            security_quality_of_service: null_mut(),
        };
        let mut io_status = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let mut handle: HANDLE = null_mut();
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                desired_access,
                &mut object_attributes,
                &mut io_status,
                null_mut(),
                attributes,
                share_access,
                disposition,
                options,
                null_mut(),
                0,
            )
        };
        if status < 0 {
            let code = unsafe { RtlNtStatusToDosError(status) };
            Err(std::io::Error::from_raw_os_error(code as i32))
        } else if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(std::io::Error::new(
                ErrorKind::Other,
                "Windows returned an invalid file handle",
            ))
        } else {
            Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
        }
    }

    fn require_directory(handle: &OwnedHandle, message: &str) -> Result<(), ApiError> {
        let attributes = query_attributes(handle)?;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(local_state_message(message));
        }
        Ok(())
    }

    fn require_regular(handle: &OwnedHandle, message: &str) -> Result<(), ApiError> {
        let attributes = query_attributes(handle)?;
        let information = query_file_information(handle)?;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || attributes & FILE_ATTRIBUTE_DIRECTORY != 0
            || information.nNumberOfLinks != 1
        {
            return Err(local_state_message(message));
        }
        Ok(())
    }

    fn query_attributes(handle: &OwnedHandle) -> Result<u32, ApiError> {
        let mut value = FILE_ATTRIBUTE_TAG_INFO::default();
        let success = unsafe {
            GetFileInformationByHandleEx(
                handle.as_raw_handle() as HANDLE,
                FileAttributeTagInfo,
                (&mut value as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        if success == 0 {
            Err(local_state_error(
                "检查 Windows 刷新会话对象",
                std::io::Error::last_os_error(),
            ))
        } else {
            Ok(value.FileAttributes)
        }
    }

    fn query_file_information(
        handle: &OwnedHandle,
    ) -> Result<BY_HANDLE_FILE_INFORMATION, ApiError> {
        let mut value = BY_HANDLE_FILE_INFORMATION::default();
        let success = unsafe {
            GetFileInformationByHandle(
                handle.as_raw_handle() as HANDLE,
                &mut value,
            )
        };
        if success == 0 {
            Err(local_state_error(
                "检查 Windows 刷新会话文件",
                std::io::Error::last_os_error(),
            ))
        } else {
            Ok(value)
        }
    }

    fn rename_open_file(
        file: &File,
        destination_parent: &OwnedHandle,
        destination_name: &OsStr,
    ) -> std::io::Result<()> {
        require_leaf_name_io(destination_name)?;
        let wide: Vec<u16> = destination_name.encode_wide().collect();
        let byte_length = wide
            .len()
            .checked_mul(2)
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "name is too long"))?;
        let header_length = offset_of!(FILE_RENAME_INFO, FileName);
        let total_length = header_length
            .checked_add(byte_length as usize)
            .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "name is too long"))?
            .max(size_of::<FILE_RENAME_INFO>());
        let word_count = total_length.div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; word_count];
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*info).Anonymous.ReplaceIfExists = true;
            (*info).RootDirectory = destination_parent.as_raw_handle() as HANDLE;
            (*info).FileNameLength = byte_length;
            std::ptr::copy_nonoverlapping(
                wide.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                wide.len(),
            );
        }
        // Win32 FileRenameInfo requires a null RootDirectory. The native class
        // accepts our retained parent handle, keeping replacement atomic without
        // reopening a pathname that could have been redirected.
        let mut io_status: IoStatusBlock = unsafe { std::mem::zeroed() };
        let status = unsafe {
            NtSetInformationFile(
                file.as_raw_handle() as HANDLE,
                &mut io_status,
                info.cast(),
                total_length as u32,
                FILE_RENAME_INFORMATION,
            )
        };
        if status < 0 {
            Err(std::io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32))
        } else {
            Ok(())
        }
    }

    fn delete_open_file(file: &File) {
        let _ = mark_delete(file);
    }

    fn mark_delete(file: &File) -> std::io::Result<()> {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        let success = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        if success == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn require_leaf_name(name: &OsStr) -> Result<(), ApiError> {
        require_leaf_name_io(name).map_err(|_| local_state_message("刷新会话文件名无效"))
    }

    fn require_leaf_name_io(name: &OsStr) -> std::io::Result<()> {
        let mut components = Path::new(name).components();
        if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "relative leaf name required",
            ))
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod file_store_platform {
    use super::local_state_message;
    use crate::runtime::api::ApiError;
    use std::ffi::OsStr;
    use std::path::Path;

    pub(super) struct LockedStore;

    impl LockedStore {
        pub(super) fn open(_: &Path, _: bool) -> Result<Option<Self>, ApiError> {
            Err(local_state_message("当前平台不支持安全的刷新会话存储"))
        }

        pub(super) fn load(&self) -> Result<Option<Vec<u8>>, ApiError> {
            Err(local_state_message("当前平台不支持安全的刷新会话存储"))
        }

        pub(super) fn replace(&self, _: &OsStr, _: &[u8]) -> Result<(), ApiError> {
            Err(local_state_message("当前平台不支持安全的刷新会话存储"))
        }

        pub(super) fn remove(&self) -> Result<(), ApiError> {
            Err(local_state_message("当前平台不支持安全的刷新会话存储"))
        }
    }
}

#[derive(Default)]
struct SessionState {
    // A failed startup read is unknown state, never an authorized empty slot.
    startup_read_failure: Option<ApiError>,
    access_token: Option<String>,
    owner_user_id: Option<String>,
    persisted_owner_user_id: Option<String>,
    // Only records captured by startup, install, or an owner-confirmed refresh grant
    // mutation authority. A conflicting record is observed as metadata only.
    authorized_persisted_session: Option<PersistedRefreshSession>,
    scope_published: bool,
    epoch_exhausted: bool,
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
        let (authorized_persisted_session, startup_read_failure) = match store.load() {
            Ok(record) => (record, None),
            Err(error) => (None, Some(error)),
        };
        let persisted_owner_user_id = authorized_persisted_session
            .as_ref()
            .map(|session| session.owner_user_id.clone());
        let owner_user_id = persisted_owner_user_id.clone();
        Self {
            store,
            state: Mutex::new(SessionState {
                owner_user_id,
                persisted_owner_user_id,
                authorized_persisted_session,
                startup_read_failure,
                ..SessionState::default()
            }),
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
        if state.auth_epoch != auth_epoch || !state.scope_published {
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
        if state.auth_epoch != auth_epoch || !state.scope_published {
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

    pub(crate) fn persisted_owner_user_id(&self) -> Option<String> {
        self.lock_state().persisted_owner_user_id.clone()
    }

    pub(crate) fn install_tokens_for_user(
        &self,
        tokens: &TokenSet,
        owner_user_id: &str,
    ) -> Result<SessionScope, ApiError> {
        let mut state = self.lock_state();
        if let Some(error) = &state.startup_read_failure { return Err(error.clone()); }
        if state.authorized_persisted_session.is_none() && state.persisted_owner_user_id.is_some() {
            return Err(ApiError::AuthenticationRequired);
        }
        let (next_auth, next_refresh) = self.next_epochs(&mut state, true)?;
        let persisted_session = PersistedRefreshSession {
            owner_user_id: owner_user_id.to_string(),
            refresh_token: tokens.refresh_token.clone(),
        };
        self.store.save(&persisted_session)?;
        state.auth_epoch = next_auth;
        state.access_token = Some(tokens.access_token.clone());
        state.owner_user_id = Some(owner_user_id.to_string());
        state.persisted_owner_user_id = Some(owner_user_id.to_string());
        state.authorized_persisted_session = Some(persisted_session);
        state.scope_published = true;
        state.refreshing = false;
        state.refresh_epoch = next_refresh;
        state.last_refresh_result = Some(Ok(tokens.access_token.clone()));
        self.refresh_finished.notify_all();
        Ok(SessionScope {
            owner_user_id: owner_user_id.to_string(),
            auth_epoch: state.auth_epoch,
        })
    }

    pub(crate) fn refresh_persisted_owner<F>(
        &self,
        owner_user_id: &str,
        refresh: F,
    ) -> Result<SessionScope, ApiError>
    where
        F: FnOnce(&str) -> Result<TokenSet, ApiError>,
    {
        let canonical_owner = canonical_owner_user_id(owner_user_id)
            .ok()
            .filter(|canonical| canonical == owner_user_id)
            .ok_or(ApiError::AuthenticationRequired)?;
        let (auth_epoch, refresh_session) = {
            let mut state = self.lock_state();
            if let Some(error) = &state.startup_read_failure { return Err(error.clone()); }
            if state.authorized_persisted_session.is_none() && state.persisted_owner_user_id.is_some() {
                return Err(ApiError::AuthenticationRequired);
            }
            self.next_epochs(&mut state, true)?;
            if state.scope_published
                || state.access_token.is_some()
                || state.refreshing
            {
                return Err(ApiError::AuthenticationRequired);
            }
            let current = self.store.load()?;
            state.persisted_owner_user_id = current
                .as_ref()
                .map(|record| record.owner_user_id.clone());
            let refresh_session = match current {
                Some(record) if record.owner_user_id == canonical_owner => record,
                _ => {
                    state.owner_user_id = state.persisted_owner_user_id.clone();
                    return Err(ApiError::AuthenticationRequired);
                }
            };
            state.authorized_persisted_session = Some(refresh_session.clone());
            state.owner_user_id = Some(canonical_owner.clone());
            state.refreshing = true;
            state.last_refresh_result = None;
            (state.auth_epoch, refresh_session)
        };

        let refreshed = refresh(&refresh_session.refresh_token);
        match refreshed {
            Ok(tokens) => {
                let mut state = self.lock_state();
                if state.auth_epoch != auth_epoch
                    || state.owner_user_id.as_deref() != Some(canonical_owner.as_str())
                    || state.scope_published
                    || !state.refreshing
                {
                    return Err(ApiError::AuthenticationRequired);
                }
                let (next_auth, next_refresh) = self.next_epochs(&mut state, true)?;
                let replacement = PersistedRefreshSession {
                    owner_user_id: canonical_owner.clone(),
                    refresh_token: tokens.refresh_token.clone(),
                };
                match self
                    .store
                    .replace_if_current(&refresh_session, &replacement)
                {
                    Ok(StoreMutation::Applied) => {
                        state.persisted_owner_user_id = Some(canonical_owner.clone());
                        state.authorized_persisted_session = Some(replacement);
                    }
                    Ok(StoreMutation::Conflict(current)) => {
                        self.record_store_conflict(&mut state, current);
                        return Err(ApiError::AuthenticationRequired);
                    }
                    Err(error) => {
                        self.record_refresh_failure(&mut state, error.clone());
                        return Err(error);
                    }
                }
                state.auth_epoch = next_auth;
                state.access_token = Some(tokens.access_token.clone());
                state.owner_user_id = Some(canonical_owner.clone());
                state.scope_published = true;
                state.refreshing = false;
                state.refresh_epoch = next_refresh;
                state.last_refresh_result = Some(Ok(tokens.access_token));
                self.refresh_finished.notify_all();
                Ok(SessionScope {
                    owner_user_id: canonical_owner,
                    auth_epoch: state.auth_epoch,
                })
            }
            Err(error) => {
                let mut state = self.lock_state();
                if state.auth_epoch == auth_epoch
                    && state.owner_user_id.as_deref() == Some(canonical_owner.as_str())
                    && !state.scope_published
                    && state.refreshing
                {
                    state.refreshing = false;
                    state.refresh_epoch = self.next_epochs(&mut state, false)?.1;
                    state.last_refresh_result = Some(Err(error.clone()));
                    self.refresh_finished.notify_all();
                    Err(error)
                } else {
                    Err(ApiError::AuthenticationRequired)
                }
            }
        }
    }

    pub(crate) fn clear(&self) -> Result<(), ApiError> {
        let mut state = self.lock_state();
        self.clear_locked(&mut state)
    }

    pub(crate) fn clear_access_token(&self) {
        self.lock_state().access_token = None;
    }

    pub(crate) fn bind_user(&self, owner_user_id: &str) -> Result<SessionScope, ApiError> {
        let state = self.lock_state();
        if state.access_token.is_none()
            || !state.scope_published
            || state.owner_user_id.as_deref() != Some(owner_user_id)
        {
            return Err(ApiError::AuthenticationRequired);
        }
        Ok(SessionScope {
            owner_user_id: owner_user_id.to_string(),
            auth_epoch: state.auth_epoch,
        })
    }

    pub(crate) fn scope_for_user(&self, owner_user_id: &str) -> Option<SessionScope> {
        let state = self.lock_state();
        (state.scope_published && state.owner_user_id.as_deref() == Some(owner_user_id)).then(|| {
            SessionScope {
                owner_user_id: owner_user_id.to_string(),
                auth_epoch: state.auth_epoch,
            }
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
            return Ok(());
        }
        if !scope_matches(&state, scope) {
            return Err(ApiError::AuthenticationRequired);
        }
        self.clear_locked(&mut state)
    }

    pub(crate) fn clear_epoch(&self, auth_epoch: u64) -> Result<(), ApiError> {
        let mut state = self.lock_state();
        if cleared_lease_matches_epoch(&state, auth_epoch) {
            return Ok(());
        }
        if state.auth_epoch != auth_epoch {
            return Err(ApiError::AuthenticationRequired);
        }
        self.clear_locked(&mut state)
    }

    pub(crate) fn refresh<F>(
        &self,
        rejected_access_token: Option<&str>,
        refresh: F,
    ) -> Result<String, ApiError>
    where
        F: FnOnce(&str) -> Result<TokenSet, ApiError>,
    {
        let scope = {
            let state = self.lock_state();
            if !state.scope_published {
                return Err(ApiError::AuthenticationRequired);
            }
            SessionScope {
                owner_user_id: state
                    .owner_user_id
                    .clone()
                    .ok_or(ApiError::AuthenticationRequired)?,
                auth_epoch: state.auth_epoch,
            }
        };
        self.refresh_scope(&scope, rejected_access_token, refresh)
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
        let refresh_session = {
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

            self.next_epochs(&mut state, false)?;
            let expected = match state.authorized_persisted_session.clone() {
                Some(record) if record.owner_user_id == scope.owner_user_id => record,
                _ => {
                    invalidate_session_lease(&mut state);
                    self.refresh_finished.notify_all();
                    return Err(ApiError::AuthenticationRequired);
                }
            };
            state.access_token = None;
            let current = match self.store.load() {
                Ok(current) => current,
                Err(error) => {
                    self.record_refresh_failure(&mut state, error.clone());
                    return Err(error);
                }
            };
            if current.as_ref() != Some(&expected) {
                self.record_store_conflict(&mut state, current);
                return Err(ApiError::AuthenticationRequired);
            }
            state.refreshing = true;
            state.last_refresh_result = None;
            expected
        };

        match refresh(&refresh_session.refresh_token) {
            Ok(tokens) => {
                self.persist_refreshed_tokens_if_current(scope, &refresh_session, &tokens)
            }
            Err(error) => self.finish_refresh(scope, Err(error)),
        }
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
        let scope = {
            let state = self.lock_state();
            if state.auth_epoch != auth_epoch || !state.scope_published {
                return Err(ApiError::AuthenticationRequired);
            }
            SessionScope {
                owner_user_id: state
                    .owner_user_id
                    .clone()
                    .ok_or(ApiError::AuthenticationRequired)?,
                auth_epoch,
            }
        };
        self.refresh_scope(&scope, rejected_access_token, refresh)
    }

    pub(crate) fn persist_refreshed_tokens(
        &self,
        scope: &SessionScope,
        tokens: &TokenSet,
    ) -> Result<String, ApiError> {
        let expected = {
            let mut state = self.lock_state();
            if !scope_matches(&state, scope) {
                return Err(ApiError::AuthenticationRequired);
            }
            match state.authorized_persisted_session.clone() {
                Some(record) if record.owner_user_id == scope.owner_user_id => record,
                _ => {
                    invalidate_session_lease(&mut state);
                    self.refresh_finished.notify_all();
                    return Err(ApiError::AuthenticationRequired);
                }
            }
        };
        self.persist_refreshed_tokens_if_current(scope, &expected, tokens)
    }

    fn persist_refreshed_tokens_if_current(
        &self,
        scope: &SessionScope,
        expected: &PersistedRefreshSession,
        tokens: &TokenSet,
    ) -> Result<String, ApiError> {
        let mut state = self.lock_state();
        if !scope_matches(&state, scope) {
            return Err(ApiError::AuthenticationRequired);
        }
        let next_refresh = self.next_epochs(&mut state, false)?.1;
        let replacement = PersistedRefreshSession {
            owner_user_id: scope.owner_user_id.clone(),
            refresh_token: tokens.refresh_token.clone(),
        };
        match self.store.replace_if_current(expected, &replacement) {
            Ok(StoreMutation::Applied) => {
                state.persisted_owner_user_id = Some(scope.owner_user_id.clone());
                state.authorized_persisted_session = Some(replacement);
            }
            Ok(StoreMutation::Conflict(current)) => {
                self.record_store_conflict(&mut state, current);
                return Err(ApiError::AuthenticationRequired);
            }
            Err(error) => {
                self.record_refresh_failure(&mut state, error.clone());
                return Err(error);
            }
        }
        state.refreshing = false;
        state.refresh_epoch = next_refresh;
        state.access_token = Some(tokens.access_token.clone());
        state.last_refresh_result = Some(Ok(tokens.access_token.clone()));
        self.refresh_finished.notify_all();
        Ok(tokens.access_token.clone())
    }

    fn clear_locked(&self, state: &mut SessionState) -> Result<(), ApiError> {
        if let Some(error) = &state.startup_read_failure { return Err(error.clone()); }
        let Some(expected) = state.authorized_persisted_session.clone() else {
            invalidate_session_lease(state);
            self.refresh_finished.notify_all();
            if state.persisted_owner_user_id.is_some() {
                // A previously observed replacement is not an authorized empty slot.
                // Never adopt, delete, or overwrite it on a later retry.
                return Err(ApiError::AuthenticationRequired);
            }
            return Ok(());
        };
        let result = self.store.clear_if_current(&expected);
        match result {
            Ok(StoreMutation::Applied) => {
                state.persisted_owner_user_id = None;
                state.authorized_persisted_session = None;
                invalidate_session_lease(state);
                self.refresh_finished.notify_all();
                Ok(())
            }
            Ok(StoreMutation::Conflict(current)) => {
                self.record_store_conflict(state, current);
                Err(ApiError::AuthenticationRequired)
            }
            Err(error) => {
                // Access is invalidated even when durable cleanup fails, but the exact known
                // authorized record remains so cleanup can be retried without acquiring
                // authority over a different durable record.
                invalidate_session_lease(state);
                self.refresh_finished.notify_all();
                Err(error)
            }
        }
    }

    fn record_store_conflict(
        &self,
        state: &mut SessionState,
        current: Option<PersistedRefreshSession>,
    ) {
        state.persisted_owner_user_id = current
            .as_ref()
            .map(|session| session.owner_user_id.clone());
        state.authorized_persisted_session = None;
        if state.scope_published {
            invalidate_session_lease(state);
            self.refresh_finished.notify_all();
        } else {
            state.owner_user_id = state.persisted_owner_user_id.clone();
            self.record_refresh_failure(state, ApiError::AuthenticationRequired);
        }
    }

    fn record_refresh_failure(&self, state: &mut SessionState, error: ApiError) {
        state.refreshing = false;
        let Ok((_, next_refresh)) = self.next_epochs(state, false) else { return; };
        state.refresh_epoch = next_refresh;
        state.last_refresh_result = Some(Err(error));
        self.refresh_finished.notify_all();
    }

    fn finish_refresh(
        &self,
        scope: &SessionScope,
        result: Result<String, ApiError>,
    ) -> Result<String, ApiError> {
        let mut state = self.lock_state();
        if !scope_matches(&state, scope) {
            return Err(ApiError::AuthenticationRequired);
        }
        state.refreshing = false;
        state.refresh_epoch = self.next_epochs(&mut state, false)?.1;
        state.access_token = result.clone().ok();
        state.last_refresh_result = Some(result.clone());
        self.refresh_finished.notify_all();
        result
    }

    #[cfg(test)]
    fn persisted_refresh_token_for_test(&self) -> String {
        self.store
            .load()
            .ok()
            .flatten()
            .map(|session| session.refresh_token)
            .unwrap_or_default()
    }

    fn next_epochs(&self, state: &mut SessionState, advance_auth: bool) -> Result<(u64, u64), ApiError> {
        let auth = if advance_auth { state.auth_epoch.checked_add(1) } else { Some(state.auth_epoch) };
        if !state.epoch_exhausted {
            if let (Some(auth), Some(refresh)) = (auth, state.refresh_epoch.checked_add(1)) {
                return Ok((auth, refresh));
            }
        }
        close_exhausted_session(state);
        self.refresh_finished.notify_all();
        Err(ApiError::AuthenticationRequired)
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn scope_matches(state: &SessionState, scope: &SessionScope) -> bool {
    !state.epoch_exhausted && state.scope_published
        && state.auth_epoch == scope.auth_epoch
        && state.owner_user_id.as_deref() == Some(scope.owner_user_id.as_str())
}

fn close_exhausted_session(state: &mut SessionState) {
    state.epoch_exhausted = true;
    state.access_token = None;
    state.owner_user_id = None;
    state.scope_published = false;
    state.refreshing = false;
    state.last_refresh_result = Some(Err(ApiError::AuthenticationRequired));
}

fn invalidate_session_lease(state: &mut SessionState) {
    let next = state.auth_epoch.checked_add(1).zip(state.refresh_epoch.checked_add(1));
    if state.epoch_exhausted || next.is_none() {
        close_exhausted_session(state);
        return;
    }
    let (auth, refresh) = next.unwrap();
    state.auth_epoch = auth;
    state.access_token = None;
    state.owner_user_id = None;
    state.scope_published = false;
    state.refreshing = false;
    state.refresh_epoch = refresh;
    state.last_refresh_result = None;
}

fn cleared_lease_matches_epoch(state: &SessionState, cleared_auth_epoch: u64) -> bool {
    !state.epoch_exhausted && Some(state.auth_epoch) == cleared_auth_epoch.checked_add(1)
        && state.access_token.is_none()
        && state.owner_user_id.is_none()
        && !state.scope_published
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    #[derive(Default)]
    pub(crate) struct MemoryRefreshTokenStore {
        value: Mutex<Option<PersistedRefreshSession>>,
    }

    impl MemoryRefreshTokenStore {
        pub(crate) fn with_session(owner_user_id: &str, refresh_token: &str) -> Self {
            Self {
                value: Mutex::new(Some(PersistedRefreshSession {
                    owner_user_id: owner_user_id.to_string(),
                    refresh_token: refresh_token.to_string(),
                })),
            }
        }
    }

    impl RefreshTokenStore for MemoryRefreshTokenStore {
        fn load(&self) -> Result<Option<PersistedRefreshSession>, ApiError> {
            Ok(self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone())
        }

        fn save(&self, session: &PersistedRefreshSession) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session.clone());
            Ok(())
        }

        fn clear(&self) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            Ok(())
        }

        fn replace_if_current(
            &self,
            expected: &PersistedRefreshSession,
            replacement: &PersistedRefreshSession,
        ) -> Result<StoreMutation, ApiError> {
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if value.as_ref() != Some(expected) {
                return Ok(StoreMutation::Conflict(value.clone()));
            }
            *value = Some(replacement.clone());
            Ok(StoreMutation::Applied)
        }

        fn clear_if_current(
            &self,
            expected: &PersistedRefreshSession,
        ) -> Result<StoreMutation, ApiError> {
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if value.as_ref() != Some(expected) {
                return Ok(StoreMutation::Conflict(value.clone()));
            }
            *value = None;
            Ok(StoreMutation::Applied)
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

    #[cfg(windows)]
    use std::io::ErrorKind;
    #[cfg(windows)]
    use std::os::windows::ffi::OsStrExt;
    #[cfg(windows)]
    use std::ptr::null_mut;
    #[cfg(windows)]
    use windows_sys::Win32::Foundation::LocalFree;
    #[cfg(windows)]
    use windows_sys::Win32::Security::Authorization::{
        DENY_ACCESS, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW,
        NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW,
        TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    #[cfg(windows)]
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID,
    };
    #[cfg(windows)]
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_TRAVERSE, SYNCHRONIZE,
    };

    const USER_A: &str = "11111111-1111-4111-8111-111111111111";
    const USER_B: &str = "22222222-2222-4222-8222-222222222222";
    #[test]
    fn core_session_epoch_exhaustion_does_not_replace_credentials_or_reuse_scope() {
        for auth_exhausted in [true, false] {
            let store = Arc::new(MemoryRefreshTokenStore::default());
            let manager = SessionManager::new(store.clone());
            manager.install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A).unwrap();
            {
                let mut state = manager.lock_state();
                if auth_exhausted { state.auth_epoch = u64::MAX; }
                else { state.refresh_epoch = u64::MAX; }
            }
            assert!(manager.install_tokens_for_user(&tokens("access-b", "refresh-b"), USER_B).is_err());
            let retained = store.load().unwrap().unwrap();
            assert_eq!(retained.owner_user_id, USER_A);
            assert_eq!(retained.refresh_token, "refresh-a");
            assert!(manager.access().is_none());
            assert!(manager.install_tokens_for_user(&tokens("access-c", "refresh-c"), USER_A).is_err());
            assert!(manager.refresh_persisted_owner(USER_A, |_| panic!("exhausted authority cannot dispatch refresh")).is_err());
        }
    }

    fn tokens(access: &str, refresh: &str) -> TokenSet {
        TokenSet {
            access_token: access.to_string(),
            access_expires_in_seconds: 1800,
            refresh_token: refresh.to_string(),
            refresh_expires_at: "2099-01-01T00:00:00Z".to_string(),
            token_type: "X-Token".to_string(),
        }
    }

    fn temporary_directory() -> tempfile::TempDir {
        let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
        tempfile::tempdir_in(parent).unwrap()
    }

    #[cfg(windows)]
    struct TemporaryTraverseOnlyAcl {
        path: Vec<u16>,
        original_descriptor: PSECURITY_DESCRIPTOR,
        original_dacl: *mut ACL,
        active: bool,
    }

    #[cfg(windows)]
    impl TemporaryTraverseOnlyAcl {
        fn install(path: &Path) -> std::io::Result<Self> {
            let mut path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut owner: PSID = null_mut();
            let mut original_dacl: *mut ACL = null_mut();
            let mut original_descriptor: PSECURITY_DESCRIPTOR = null_mut();
            let read_status = unsafe {
                GetNamedSecurityInfoW(
                    path.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut owner,
                    null_mut(),
                    &mut original_dacl,
                    null_mut(),
                    &mut original_descriptor,
                )
            };
            if read_status != 0 {
                return Err(std::io::Error::from_raw_os_error(read_status as i32));
            }
            if owner.is_null() || original_descriptor.is_null() {
                unsafe {
                    LocalFree(original_descriptor.cast());
                }
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    "temporary directory security descriptor has no owner",
                ));
            }

            let trustee = TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: owner.cast(),
            };
            let entries = [
                EXPLICIT_ACCESS_W {
                    grfAccessPermissions: FILE_LIST_DIRECTORY,
                    grfAccessMode: DENY_ACCESS,
                    grfInheritance: NO_INHERITANCE,
                    Trustee: trustee,
                },
                EXPLICIT_ACCESS_W {
                    grfAccessPermissions: FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    grfAccessMode: GRANT_ACCESS,
                    grfInheritance: NO_INHERITANCE,
                    Trustee: trustee,
                },
            ];
            let mut temporary_dacl: *mut ACL = null_mut();
            let acl_status = unsafe {
                SetEntriesInAclW(
                    entries.len() as u32,
                    entries.as_ptr(),
                    original_dacl,
                    &mut temporary_dacl,
                )
            };
            if acl_status != 0 {
                unsafe {
                    LocalFree(temporary_dacl.cast());
                    LocalFree(original_descriptor.cast());
                }
                return Err(std::io::Error::from_raw_os_error(acl_status as i32));
            }
            let set_status = unsafe {
                SetNamedSecurityInfoW(
                    path.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    temporary_dacl,
                    null_mut(),
                )
            };
            unsafe {
                LocalFree(temporary_dacl.cast());
            }
            if set_status != 0 {
                unsafe {
                    LocalFree(original_descriptor.cast());
                }
                return Err(std::io::Error::from_raw_os_error(set_status as i32));
            }
            Ok(Self {
                path,
                original_descriptor,
                original_dacl,
                active: true,
            })
        }

        fn restore(&mut self) -> std::io::Result<()> {
            if !self.active {
                return Ok(());
            }
            let status = unsafe {
                SetNamedSecurityInfoW(
                    self.path.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    self.original_dacl,
                    null_mut(),
                )
            };
            if status != 0 {
                return Err(std::io::Error::from_raw_os_error(status as i32));
            }
            self.release_descriptor();
            Ok(())
        }

        fn release_descriptor(&mut self) {
            unsafe {
                LocalFree(self.original_descriptor.cast());
            }
            self.original_descriptor = null_mut();
            self.original_dacl = null_mut();
            self.active = false;
        }
    }

    #[cfg(windows)]
    impl Drop for TemporaryTraverseOnlyAcl {
        fn drop(&mut self) {
            if self.active {
                let _ = unsafe {
                    SetNamedSecurityInfoW(
                        self.path.as_mut_ptr(),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION,
                        null_mut(),
                        null_mut(),
                        self.original_dacl,
                        null_mut(),
                    )
                };
                self.release_descriptor();
            }
        }
    }

    fn stored_refresh(store: &dyn RefreshTokenStore) -> Option<String> {
        store
            .load()
            .unwrap()
            .map(|session| session.refresh_token)
    }

    fn session_manager_with_owner(owner_user_id: &str, refresh_token: &str) -> SessionManager {
        let manager = SessionManager::new(Arc::new(MemoryRefreshTokenStore::default()));
        manager
            .install_tokens_for_user(&tokens("access", refresh_token), owner_user_id)
            .unwrap();
        manager
    }

    #[test]
    fn refresh_record_round_trips_with_owner_without_debugging_token() {
        let directory = temporary_directory();
        let store = FileRefreshTokenStore::new(directory.path());
        store
            .save(&PersistedRefreshSession {
                owner_user_id: "11111111-1111-4111-8111-111111111111".to_string(),
                refresh_token: "refresh-secret".to_string(),
            })
            .unwrap();
        let restored = store.load().unwrap().unwrap();
        assert_eq!(
            restored.owner_user_id,
            "11111111-1111-4111-8111-111111111111"
        );
        assert!(restored.refresh_token == "refresh-secret");
    }

    #[test]
    fn ownerless_legacy_refresh_token_is_never_installed() {
        let directory = temporary_directory();
        fs::create_dir_all(directory.path().join("session")).unwrap();
        fs::write(
            directory.path().join("session/refresh-token"),
            "legacy-secret",
        )
        .unwrap();

        let manager = SessionManager::with_file_store(directory.path());

        assert!(manager.persisted_owner_user_id().is_none());
        assert!(manager.access().is_none());
        assert_eq!(
            fs::read_to_string(directory.path().join("session/refresh-token")).unwrap(),
            "legacy-secret"
        );
    }

    #[test]
    fn startup_loads_only_the_owner_bound_record_without_publishing_a_scope() {
        let directory = temporary_directory();
        FileRefreshTokenStore::new(directory.path())
            .save(&PersistedRefreshSession {
                owner_user_id: USER_A.to_string(),
                refresh_token: "refresh-a".to_string(),
            })
            .unwrap();

        let manager = SessionManager::with_file_store(directory.path());

        assert_eq!(manager.persisted_owner_user_id().as_deref(), Some(USER_A));
        assert!(manager.access().is_none());
        assert!(manager.scope_for_user(USER_A).is_none());
    }

    #[test]
    fn invalid_persisted_owner_is_a_protocol_error_without_token_disclosure() {
        let directory = temporary_directory();
        fs::create_dir_all(directory.path().join("session")).unwrap();
        fs::write(
            directory.path().join("session/refresh-session.json"),
            r#"{"owner_user_id":"not-a-uuid","refresh_token":"never-disclose-this"}"#,
        )
        .unwrap();

        let error = FileRefreshTokenStore::new(directory.path())
            .load()
            .err()
            .unwrap();

        assert!(matches!(error, ApiError::Protocol { .. }));
        assert!(!error.to_string().contains("never-disclose-this"));
    }

    #[cfg(unix)]
    #[test]
    fn file_store_rejects_final_symlink_before_reading_or_repairing_permissions() {
        use std::os::unix::fs::symlink;

        let directory = temporary_directory();
        let session_dir = directory.path().join(SESSION_DIR);
        fs::create_dir_all(&session_dir).unwrap();
        let outside = directory.path().join("outside-refresh-session.json");
        fs::write(
            &outside,
            format!(
                r#"{{"owner_user_id":"{USER_A}","refresh_token":"outside-refresh"}}"#
            ),
        )
        .unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&outside, session_dir.join(REFRESH_SESSION_FILE)).unwrap();

        let result = FileRefreshTokenStore::new(directory.path()).load();

        assert!(result.is_err());
        assert_eq!(
            fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(fs::symlink_metadata(session_dir.join(REFRESH_SESSION_FILE))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_rejects_a_symlinked_data_root_ancestor_without_external_writes() {
        use std::os::unix::fs::symlink;

        let parent = temporary_directory();
        let external = temporary_directory();
        let external_data_root = external.path().join("data-root");
        fs::create_dir(&external_data_root).unwrap();
        let alias = parent.path().join("alias-parent");
        symlink(external.path(), &alias).unwrap();
        let store = FileRefreshTokenStore::new(&alias.join("data-root"));

        let result = store.save(&PersistedRefreshSession {
            owner_user_id: USER_A.to_string(),
            refresh_token: "refresh-a".to_string(),
        });

        assert!(result.is_err());
        assert!(!external_data_root.join(SESSION_DIR).exists());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_rejects_symlinked_session_directory_and_lock_file() {
        use std::os::unix::fs::symlink;

        let directory = temporary_directory();
        let external = temporary_directory();
        symlink(external.path(), directory.path().join(SESSION_DIR)).unwrap();
        let session_link_result = FileRefreshTokenStore::new(directory.path()).save(
            &PersistedRefreshSession {
                owner_user_id: USER_A.to_string(),
                refresh_token: "refresh-a".to_string(),
            },
        );
        assert!(session_link_result.is_err());
        assert!(!external.path().join(REFRESH_SESSION_FILE).exists());

        fs::remove_file(directory.path().join(SESSION_DIR)).unwrap();
        fs::create_dir(directory.path().join(SESSION_DIR)).unwrap();
        let external_lock = external.path().join("external-lock");
        fs::write(&external_lock, b"foreign-lock").unwrap();
        symlink(
            &external_lock,
            directory
                .path()
                .join(SESSION_DIR)
                .join(".refresh-session.lock"),
        )
        .unwrap();
        let lock_link_result = FileRefreshTokenStore::new(directory.path()).save(
            &PersistedRefreshSession {
                owner_user_id: USER_A.to_string(),
                refresh_token: "refresh-a".to_string(),
            },
        );
        assert!(lock_link_result.is_err());
        assert_eq!(fs::read(&external_lock).unwrap(), b"foreign-lock");
    }

    #[test]
    fn file_store_rejects_a_non_regular_refresh_session_entry() {
        let directory = temporary_directory();
        fs::create_dir_all(
            directory
                .path()
                .join(SESSION_DIR)
                .join(REFRESH_SESSION_FILE),
        )
        .unwrap();

        assert!(FileRefreshTokenStore::new(directory.path()).load().is_err());
    }

    #[test]
    fn save_collision_never_removes_or_changes_the_existing_temporary_entry() {
        let directory = temporary_directory();
        let session_dir = directory.path().join(SESSION_DIR);
        fs::create_dir_all(&session_dir).unwrap();
        let collision = session_dir.join("preexisting-refresh-session.tmp");
        let original = b"belongs-to-an-earlier-writer";
        fs::write(&collision, original).unwrap();
        let store = FileRefreshTokenStore::with_temporary_path_for_test(
            directory.path(),
            collision.clone(),
        );

        let result = store.save(&PersistedRefreshSession {
            owner_user_id: USER_A.to_string(),
            refresh_token: "refresh-a".to_string(),
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&collision).unwrap(), original);
    }

    #[test]
    fn persisted_owner_refresh_cannot_replace_a_newer_owner_from_another_manager() {
        let directory = temporary_directory();
        let store_a = Arc::new(FileRefreshTokenStore::new(directory.path()));
        store_a
            .save(&PersistedRefreshSession {
                owner_user_id: USER_A.to_string(),
                refresh_token: "refresh-a".to_string(),
            })
            .unwrap();
        let manager_a = SessionManager::new(store_a.clone());
        let store_b = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_b = SessionManager::new(store_b.clone());

        let result = manager_a.refresh_persisted_owner(USER_A, |refresh_token| {
            assert!(refresh_token == "refresh-a");
            manager_b
                .install_tokens_for_user(&tokens("access-b", "refresh-b"), USER_B)
                .unwrap();
            Ok(tokens("access-a-new", "refresh-a-new"))
        });

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        let current = store_b.load().unwrap().unwrap();
        assert_eq!(current.owner_user_id, USER_B);
        assert!(current.refresh_token == "refresh-b");
        assert!(manager_a.access().is_none());
        assert_eq!(manager_a.persisted_owner_user_id().as_deref(), Some(USER_B));
    }

    struct SwapAfterLoadStore {
        value: Mutex<Option<PersistedRefreshSession>>,
        replacement_after_load: Mutex<Option<PersistedRefreshSession>>,
    }

    impl SwapAfterLoadStore {
        fn new(owner_user_id: &str, refresh_token: &str) -> Self {
            Self {
                value: Mutex::new(Some(PersistedRefreshSession {
                    owner_user_id: owner_user_id.to_string(),
                    refresh_token: refresh_token.to_string(),
                })),
                replacement_after_load: Mutex::new(None),
            }
        }

        fn replace_after_next_load(&self, owner_user_id: &str, refresh_token: &str) {
            *self
                .replacement_after_load
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(PersistedRefreshSession {
                    owner_user_id: owner_user_id.to_string(),
                    refresh_token: refresh_token.to_string(),
                });
        }
    }

    impl RefreshTokenStore for SwapAfterLoadStore {
        fn load(&self) -> Result<Option<PersistedRefreshSession>, ApiError> {
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let loaded = value.clone();
            if let Some(replacement) = self
                .replacement_after_load
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                *value = Some(replacement);
            }
            Ok(loaded)
        }

        fn save(&self, session: &PersistedRefreshSession) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session.clone());
            Ok(())
        }

        fn clear(&self) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            Ok(())
        }

        fn replace_if_current(
            &self,
            expected: &PersistedRefreshSession,
            replacement: &PersistedRefreshSession,
        ) -> Result<StoreMutation, ApiError> {
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if value.as_ref() != Some(expected) {
                return Ok(StoreMutation::Conflict(value.clone()));
            }
            *value = Some(replacement.clone());
            Ok(StoreMutation::Applied)
        }

        fn clear_if_current(
            &self,
            expected: &PersistedRefreshSession,
        ) -> Result<StoreMutation, ApiError> {
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if value.as_ref() != Some(expected) {
                return Ok(StoreMutation::Conflict(value.clone()));
            }
            *value = None;
            Ok(StoreMutation::Applied)
        }
    }

    #[test]
    fn scoped_refresh_uses_one_atomic_durable_compare_and_replace() {
        let store = Arc::new(SwapAfterLoadStore::new(USER_A, "refresh-a"));
        let manager = SessionManager::new(store.clone());
        let scope = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();
        store.replace_after_next_load(USER_A, "refresh-newer");

        let result = manager.refresh_scope(&scope, Some("access-a"), |refresh_token| {
            assert!(refresh_token == "refresh-a");
            Ok(tokens("access-a-rotated", "refresh-a-rotated"))
        });

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-newer"));
    }

    #[test]
    fn a_scope_invalidated_by_store_conflict_cannot_clear_the_new_record() {
        let store = Arc::new(SwapAfterLoadStore::new(USER_A, "refresh-a"));
        let manager = SessionManager::new(store.clone());
        let scope = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();
        store.replace_after_next_load(USER_B, "refresh-b");

        let refresh_result = manager.refresh_scope(&scope, Some("access-a"), |_| {
            Ok(tokens("access-stale", "refresh-stale"))
        });
        assert!(matches!(
            refresh_result,
            Err(ApiError::AuthenticationRequired)
        ));

        manager.clear_scope(&scope).unwrap();
        let current = store.load().unwrap().unwrap();
        assert_eq!(current.owner_user_id, USER_B);
        assert!(current.refresh_token == "refresh-b");
    }

    #[test]
    fn refresh_conflict_does_not_authorize_unscoped_clear_of_a_new_owner() {
        let directory = temporary_directory();
        let store_a = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_a = SessionManager::new(store_a);
        let scope_a = manager_a
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();
        let store_b = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_b = SessionManager::new(store_b.clone());

        let refresh_result = manager_a.refresh_scope(&scope_a, Some("access-a"), |_| {
            manager_b
                .install_tokens_for_user(&tokens("access-b", "refresh-b"), USER_B)
                .unwrap();
            Ok(tokens("access-stale", "refresh-stale"))
        });
        assert!(matches!(
            refresh_result,
            Err(ApiError::AuthenticationRequired)
        ));

        assert!(matches!(manager_a.clear(), Err(ApiError::AuthenticationRequired)));
        assert!(manager_a.install_tokens_for_user(&tokens("forbidden","forbidden"), USER_A).is_err());
        let current = store_b.load().unwrap().unwrap();
        assert_eq!(current.owner_user_id, USER_B);
        assert!(current.refresh_token == "refresh-b");
    }

    #[test]
    fn refresh_conflict_does_not_authorize_unscoped_clear_of_a_new_same_owner_record() {
        let directory = temporary_directory();
        let store_a = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_a = SessionManager::new(store_a);
        let scope_a = manager_a
            .install_tokens_for_user(&tokens("access-old", "refresh-old"), USER_A)
            .unwrap();
        let store_b = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_b = SessionManager::new(store_b.clone());

        let refresh_result = manager_a.refresh_scope(&scope_a, Some("access-old"), |_| {
            manager_b
                .install_tokens_for_user(&tokens("access-new", "refresh-new"), USER_A)
                .unwrap();
            Ok(tokens("access-stale", "refresh-stale"))
        });
        assert!(matches!(
            refresh_result,
            Err(ApiError::AuthenticationRequired)
        ));

        assert!(matches!(manager_a.clear(), Err(ApiError::AuthenticationRequired)));
        assert!(manager_a.install_tokens_for_user(&tokens("forbidden","forbidden"), USER_A).is_err());
        let current = store_b.load().unwrap().unwrap();
        assert_eq!(current.owner_user_id, USER_A);
        assert!(current.refresh_token == "refresh-new");
    }

    #[test]
    fn stale_conditional_clear_never_deletes_a_newer_owner() {
        let directory = temporary_directory();
        let store_a = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_a = SessionManager::new(store_a.clone());
        let scope_a = manager_a
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();
        let store_b = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_b = SessionManager::new(store_b.clone());
        manager_b
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), USER_B)
            .unwrap();

        let result = manager_a.clear_scope(&scope_a);

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        let current = store_b.load().unwrap().unwrap();
        assert_eq!(current.owner_user_id, USER_B);
        assert!(current.refresh_token == "refresh-b");
        assert_eq!(manager_a.persisted_owner_user_id().as_deref(), Some(USER_B));

        manager_a.clear_scope(&scope_a).unwrap();
        let after_duplicate_clear = store_b.load().unwrap().unwrap();
        assert_eq!(after_duplicate_clear.owner_user_id, USER_B);
        assert!(after_duplicate_clear.refresh_token == "refresh-b");
    }

    #[test]
    fn stale_conditional_clear_compares_the_exact_same_owner_record() {
        let directory = temporary_directory();
        let store_a = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_a = SessionManager::new(store_a);
        let scope_a = manager_a
            .install_tokens_for_user(&tokens("access-a", "refresh-old"), USER_A)
            .unwrap();
        let store_b = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_b = SessionManager::new(store_b.clone());
        manager_b
            .install_tokens_for_user(&tokens("access-new", "refresh-new"), USER_A)
            .unwrap();

        let result = manager_a.clear_scope(&scope_a);

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        let current = store_b.load().unwrap().unwrap();
        assert_eq!(current.owner_user_id, USER_A);
        assert!(current.refresh_token == "refresh-new");
    }

    #[test]
    fn conflict_does_not_authorize_clear_at_the_managers_new_current_epoch() {
        let directory = temporary_directory();
        let store_a = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_a = SessionManager::new(store_a);
        let scope_a = manager_a
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();
        let store_b = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_b = SessionManager::new(store_b.clone());
        manager_b
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), USER_B)
            .unwrap();

        assert!(matches!(
            manager_a.clear_scope(&scope_a),
            Err(ApiError::AuthenticationRequired)
        ));
        let current_epoch = manager_a.auth_epoch();
        assert!(matches!(manager_a.clear_epoch(current_epoch), Err(ApiError::AuthenticationRequired)));
        assert!(manager_a.install_tokens_for_user(&tokens("forbidden","forbidden"), USER_A).is_err());
        let current = store_b.load().unwrap().unwrap();
        assert_eq!(current.owner_user_id, USER_B);
        assert!(current.refresh_token == "refresh-b");
    }

    #[test]
    fn stale_unscoped_clear_never_deletes_a_newer_owner() {
        let directory = temporary_directory();
        let store_a = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_a = SessionManager::new(store_a);
        manager_a
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();
        let store_b = Arc::new(FileRefreshTokenStore::new(directory.path()));
        let manager_b = SessionManager::new(store_b.clone());
        manager_b
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), USER_B)
            .unwrap();

        let result = manager_a.clear();

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        let current = store_b.load().unwrap().unwrap();
        assert_eq!(current.owner_user_id, USER_B);
        assert!(current.refresh_token == "refresh-b");

        assert!(matches!(manager_a.clear(), Err(ApiError::AuthenticationRequired)));
        assert!(manager_a.install_tokens_for_user(&tokens("forbidden","forbidden"), USER_A).is_err());
        let current_after_retry = store_b.load().unwrap().unwrap();
        assert_eq!(current_after_retry.owner_user_id, USER_B);
        assert!(current_after_retry.refresh_token == "refresh-b");
    }

    #[cfg(unix)]
    #[test]
    fn file_store_compare_and_replace_serializes_two_store_instances() {
        let directory = temporary_directory();
        let initial_store = FileRefreshTokenStore::new(directory.path());
        let expected = PersistedRefreshSession {
            owner_user_id: USER_A.to_string(),
            refresh_token: "refresh-initial".to_string(),
        };
        initial_store.save(&expected).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for suffix in ["one", "two"] {
            let store = FileRefreshTokenStore::new(directory.path());
            let expected = expected.clone();
            let barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                let replacement = PersistedRefreshSession {
                    owner_user_id: USER_A.to_string(),
                    refresh_token: format!("refresh-{suffix}"),
                };
                barrier.wait();
                matches!(
                    store.replace_if_current(&expected, &replacement).unwrap(),
                    StoreMutation::Applied
                )
            }));
        }
        barrier.wait();

        let applied = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|applied| *applied)
            .count();

        assert_eq!(applied, 1);
        let current = initial_store.load().unwrap().unwrap();
        assert!(matches!(
            current.refresh_token.as_str(),
            "refresh-one" | "refresh-two"
        ));
    }

    #[test]
    fn persisted_owner_refresh_requires_the_same_canonical_owner() {
        let store = Arc::new(MemoryRefreshTokenStore::with_session(USER_A, "refresh-a"));
        let manager = SessionManager::new(store);
        let calls = AtomicUsize::new(0);

        let result = manager.refresh_persisted_owner(USER_B, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(tokens("unexpected", "unexpected"))
        });

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(manager.access().is_none());
        assert!(manager.scope_for_user(USER_A).is_none());
    }

    #[test]
    fn failed_persisted_owner_refresh_does_not_publish_access_or_replace_record() {
        let store = Arc::new(MemoryRefreshTokenStore::with_session(USER_A, "refresh-a"));
        let manager = SessionManager::new(store.clone());

        let result = manager.refresh_persisted_owner(USER_A, |refresh_token| {
            assert!(refresh_token == "refresh-a");
            Err(ApiError::Network {
                message: "offline".to_string(),
                timeout: false,
            })
        });

        assert!(matches!(result, Err(ApiError::Network { .. })));
        assert!(manager.access().is_none());
        assert!(manager.scope_for_user(USER_A).is_none());
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-a"));
    }

    #[test]
    fn persisted_owner_refresh_installs_only_the_matching_owner() {
        let store = Arc::new(MemoryRefreshTokenStore::with_session(USER_A, "refresh-a"));
        let manager = SessionManager::new(store.clone());

        let scope = manager
            .refresh_persisted_owner(USER_A, |refresh_token| {
                assert!(refresh_token == "refresh-a");
                Ok(tokens("access-a", "refresh-a-rotated"))
            })
            .unwrap();

        assert_eq!(scope.owner_user_id, USER_A);
        assert_eq!(manager.scope_for_user(USER_A), Some(scope.clone()));
        assert_eq!(manager.access_token_for_scope(&scope).unwrap(), "access-a");
        assert!(
            stored_refresh(store.as_ref()).as_deref() == Some("refresh-a-rotated")
        );
    }

    #[test]
    fn stale_refresh_for_user_a_cannot_replace_user_b_record() {
        let manager = session_manager_with_owner(USER_A, "refresh-a");
        let scope_a = manager.scope_for_user(USER_A).unwrap();
        manager
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), USER_B)
            .unwrap();

        let result = manager.persist_refreshed_tokens(
            &scope_a,
            &tokens("access-a-rotated", "rotated-a"),
        );

        assert!(result.is_err());
        assert_eq!(manager.persisted_owner_user_id().as_deref(), Some(USER_B));
        assert!(manager.persisted_refresh_token_for_test() == "refresh-b");
    }

    #[test]
    fn stale_refresh_cannot_replace_a_newer_record_for_the_same_owner() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager_a = SessionManager::new(store.clone());
        let scope_a = manager_a
            .install_tokens_for_user(&tokens("access-old", "refresh-old"), USER_A)
            .unwrap();
        let manager_b = SessionManager::new(store.clone());
        manager_b
            .install_tokens_for_user(&tokens("access-new", "refresh-new"), USER_A)
            .unwrap();

        let result = manager_a.persist_refreshed_tokens(
            &scope_a,
            &tokens("access-stale", "refresh-stale"),
        );

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        let current = store.load().unwrap().unwrap();
        assert!(current.refresh_token == "refresh-new");
        assert!(manager_a.access().is_none());
    }

    #[test]
    fn stored_record_owner_mismatch_is_rejected_before_scoped_refresh_network_call() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = SessionManager::new(store.clone());
        let scope_a = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();
        store
            .save(&PersistedRefreshSession {
                owner_user_id: USER_B.to_string(),
                refresh_token: "refresh-b".to_string(),
            })
            .unwrap();
        let calls = AtomicUsize::new(0);

        let result = manager.refresh_scope(&scope_a, Some("access-a"), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(tokens("unexpected", "unexpected"))
        });

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-b"));
    }

    #[test]
    fn stored_record_owner_mismatch_is_rejected_before_epoch_refresh_network_call() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = SessionManager::new(store.clone());
        let access = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .map(|_| manager.access().unwrap())
            .unwrap();
        store
            .save(&PersistedRefreshSession {
                owner_user_id: USER_B.to_string(),
                refresh_token: "refresh-b".to_string(),
            })
            .unwrap();
        let calls = AtomicUsize::new(0);

        let result = manager.refresh_epoch(
            access.auth_epoch,
            Some(&access.access_token),
            |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(tokens("unexpected", "unexpected"))
            },
        );

        assert!(matches!(result, Err(ApiError::AuthenticationRequired)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-b"));
    }

    #[test]
    fn same_owner_rotation_keeps_owner_bound_to_the_new_refresh_token() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = SessionManager::new(store.clone());
        let scope = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();

        manager
            .persist_refreshed_tokens(
                &scope,
                &tokens("access-a-rotated", "refresh-a-rotated"),
            )
            .unwrap();

        let record = store.load().unwrap().unwrap();
        assert_eq!(record.owner_user_id, USER_A);
        assert!(record.refresh_token == "refresh-a-rotated");
        assert_eq!(
            manager.access_token_for_scope(&scope).unwrap(),
            "access-a-rotated"
        );
    }

    #[derive(Default)]
    struct FailingClearStore {
        value: Mutex<Option<PersistedRefreshSession>>,
        clear_attempts: AtomicUsize,
    }

    #[derive(Default)]
    struct FailingReplaceStore {
        value: Mutex<Option<PersistedRefreshSession>>,
    }

    impl RefreshTokenStore for FailingReplaceStore {
        fn load(&self) -> Result<Option<PersistedRefreshSession>, ApiError> {
            Ok(self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone())
        }

        fn save(&self, session: &PersistedRefreshSession) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session.clone());
            Ok(())
        }

        fn clear(&self) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            Ok(())
        }

        fn replace_if_current(
            &self,
            _expected: &PersistedRefreshSession,
            _replacement: &PersistedRefreshSession,
        ) -> Result<StoreMutation, ApiError> {
            Err(local_state_message("simulated atomic replace failure"))
        }

        fn clear_if_current(
            &self,
            expected: &PersistedRefreshSession,
        ) -> Result<StoreMutation, ApiError> {
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if value.as_ref() != Some(expected) {
                return Ok(StoreMutation::Conflict(value.clone()));
            }
            *value = None;
            Ok(StoreMutation::Applied)
        }
    }

    impl RefreshTokenStore for FailingClearStore {
        fn load(&self) -> Result<Option<PersistedRefreshSession>, ApiError> {
            Ok(self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone())
        }

        fn save(&self, session: &PersistedRefreshSession) -> Result<(), ApiError> {
            *self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session.clone());
            Ok(())
        }

        fn clear(&self) -> Result<(), ApiError> {
            Err(ApiError::Credential {
                message: "simulated keychain failure".to_string(),
            })
        }

        fn replace_if_current(
            &self,
            expected: &PersistedRefreshSession,
            replacement: &PersistedRefreshSession,
        ) -> Result<StoreMutation, ApiError> {
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if value.as_ref() != Some(expected) {
                return Ok(StoreMutation::Conflict(value.clone()));
            }
            *value = Some(replacement.clone());
            Ok(StoreMutation::Applied)
        }

        fn clear_if_current(
            &self,
            expected: &PersistedRefreshSession,
        ) -> Result<StoreMutation, ApiError> {
            if self.clear_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(ApiError::Credential {
                    message: "simulated keychain failure".to_string(),
                });
            }
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if value.as_ref() != Some(expected) {
                return Ok(StoreMutation::Conflict(value.clone()));
            }
            *value = None;
            Ok(StoreMutation::Applied)
        }
    }

    #[test]
    fn atomic_replace_io_failure_publishes_no_new_access_and_tracks_the_old_record() {
        let store = Arc::new(FailingReplaceStore::default());
        let manager = SessionManager::new(store.clone());
        let scope = manager
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), USER_A)
            .unwrap();

        let result = manager.refresh_scope(&scope, Some("access-a"), |_| {
            Ok(tokens("access-new", "refresh-new"))
        });

        let error = result.err().unwrap();
        assert!(matches!(error, ApiError::LocalState { .. }));
        assert!(!error.to_string().contains("refresh-a"));
        assert!(!error.to_string().contains("refresh-new"));
        assert!(manager.access().is_none());
        assert!(manager.is_scope_current(&scope));
        assert_eq!(manager.persisted_owner_user_id().as_deref(), Some(USER_A));
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-a"));
    }

    #[test]
    fn installing_tokens_persists_refresh_and_keeps_access_in_memory() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = SessionManager::new(store.clone());
        manager
            .install_tokens_for_user(&tokens("access-1", "refresh-1"), "user-a")
            .unwrap();

        assert_eq!(manager.access_token().as_deref(), Some("access-1"));
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-1"));
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
        assert_eq!(manager.persisted_owner_user_id().as_deref(), Some("user-a"));
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-a"));

        manager.clear().unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(manager.persisted_owner_user_id().is_none());
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
                assert!(refresh_token == "refresh-a");
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
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-b"));
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
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-a"));

        let recovered = manager
            .refresh_scope(&scope, None, |refresh_token| {
                assert!(refresh_token == "refresh-a");
                Ok(tokens("access-a-recovered", "refresh-a-rotated"))
            })
            .unwrap();

        assert_eq!(recovered, "access-a-recovered");
        assert_eq!(
            manager.access_token_for_scope(&scope).unwrap(),
            "access-a-recovered"
        );
        assert!(
            stored_refresh(store.as_ref()).as_deref() == Some("refresh-a-rotated")
        );
    }

    #[test]
    fn persisted_refresh_token_is_available_to_a_fresh_session_manager() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let first = SessionManager::new(store.clone());
        first
            .install_tokens_for_user(&tokens("access-1", "refresh-1"), USER_A)
            .unwrap();
        drop(first);

        let second = SessionManager::new(store.clone());
        assert!(second.has_refresh_token().unwrap());
        assert_eq!(second.persisted_owner_user_id().as_deref(), Some(USER_A));
        assert!(second.scope_for_user(USER_A).is_none());
        let scope = second
            .refresh_persisted_owner(USER_A, |refresh| {
                assert!(refresh == "refresh-1");
                Ok(tokens("access-2", "refresh-2"))
            })
            .unwrap();

        assert_eq!(second.access_token_for_scope(&scope).unwrap(), "access-2");
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-2"));
    }

    #[test]
    fn file_store_persists_rotated_token_and_clears_it() {
        let dir = temporary_directory();
        let first = FileRefreshTokenStore::new(dir.path());
        first
            .save(&PersistedRefreshSession {
                owner_user_id: USER_A.to_string(),
                refresh_token: "refresh-1".to_string(),
            })
            .unwrap();
        assert!(stored_refresh(&first).as_deref() == Some("refresh-1"));

        let second = FileRefreshTokenStore::new(dir.path());
        second
            .save(&PersistedRefreshSession {
                owner_user_id: USER_A.to_string(),
                refresh_token: "refresh-2".to_string(),
            })
            .unwrap();
        assert!(stored_refresh(&second).as_deref() == Some("refresh-2"));
        second.clear().unwrap();
        assert!(second.load().unwrap().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn file_store_traverses_an_ancestor_without_list_permission() {
        let directory = temporary_directory();
        let ancestor = directory.path().join("traverse-only");
        let data_root = ancestor.join("data-root");
        fs::create_dir_all(&data_root).unwrap();
        let mut acl = TemporaryTraverseOnlyAcl::install(&ancestor).unwrap();

        assert!(fs::read_dir(&ancestor).is_err());
        assert!(fs::metadata(&data_root).is_ok());

        let store = FileRefreshTokenStore::new(&data_root);
        store
            .save(&PersistedRefreshSession {
                owner_user_id: USER_A.to_string(),
                refresh_token: "refresh-secret".to_string(),
            })
            .unwrap();
        let restored = store.load().unwrap().unwrap();
        assert_eq!(restored.owner_user_id, USER_A);
        assert!(restored.refresh_token == "refresh-secret");

        acl.restore().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_store_restricts_directory_and_file_permissions() {
        let dir = temporary_directory();
        let store = FileRefreshTokenStore::new(dir.path());
        store
            .save(&PersistedRefreshSession {
                owner_user_id: USER_A.to_string(),
                refresh_token: "refresh-secret".to_string(),
            })
            .unwrap();

        let file_mode = fs::metadata(&store.path).unwrap().permissions().mode() & 0o777;
        let directory_mode = fs::metadata(store.path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(directory_mode, 0o700);
    }

    #[test]
    fn concurrent_refresh_is_single_flight() {
        let store = Arc::new(MemoryRefreshTokenStore::default());
        let manager = Arc::new(SessionManager::new(store.clone()));
        let scope = manager
            .install_tokens_for_user(&tokens("access-old", "refresh-old"), "user-a")
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        for _ in 0..6 {
            let manager = manager.clone();
            let calls = calls.clone();
            let scope = scope.clone();
            handles.push(thread::spawn(move || {
                manager
                    .refresh_scope(&scope, Some("access-old"), |refresh| {
                        assert!(refresh == "refresh-old");
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
        assert!(stored_refresh(store.as_ref()).as_deref() == Some("refresh-new"));
    }

    #[test]
    fn rotated_access_token_prevents_a_second_refresh() {
        let store = Arc::new(MemoryRefreshTokenStore::with_session("user-a", "refresh-new"));
        let manager = SessionManager::new(store);
        manager
            .install_tokens_for_user(&tokens("access-new", "refresh-new"), "user-a")
            .unwrap();
        let value = manager
            .refresh(Some("access-old"), |_| panic!("refresh must not run"))
            .unwrap();
        assert_eq!(value, "access-new");
    }
}
