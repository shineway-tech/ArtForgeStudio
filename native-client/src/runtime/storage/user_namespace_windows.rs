//! Windows retained-handle namespace implementation.

use super::{
    validate_windows_relative_name, ManagedRelativeName, ManagedUserArea, UserNamespace,
    MANAGED_USER_AREAS,
};
use anyhow::{anyhow, ensure, Context, Result};
use std::ffi::{c_void, OsStr, OsString};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::OVERLAPPED;

const LOCK_NAME: &str = ".namespace.lock";
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const SHARE_LOCK: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
// Do not require write or directory-listing rights on C:\ or its ancestors.
const TRAVERSE_ACCESS: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const MANAGED_ACCESS: u32 = TRAVERSE_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;
const REGULAR_ACCESS: u32 =
    FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE;
const NT_OPEN: u32 = 1;
const NT_CREATE: u32 = 2;
const NT_OPEN_IF: u32 = 3;
const NT_DIRECTORY: u32 = 1;
const NT_NON_DIRECTORY: u32 = 0x40;
const NT_SYNCHRONOUS: u32 = 0x20;
const NT_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const RENAME_REPLACE: u32 = 1;
const RENAME_POSIX: u32 = 2;
static BINDING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
        handle: *mut HANDLE,
        access: u32,
        attributes: *mut ObjectAttributes,
        status: *mut IoStatusBlock,
        allocation_size: *mut i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *mut c_void,
        ea_length: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Identity {
    volume: u64,
    file: [u8; 16],
}

pub(crate) struct DataRootCapability {
    handle: OwnedHandle,
    identity: Identity,
    display_root: PathBuf,
    // This non-delete-sharing handle pins the common lock name for our lifetime.
    lock: OwnedHandle,
    lock_identity: Identity,
}
struct RetainedDirectory {
    area: ManagedUserArea,
    handle: OwnedHandle,
    identity: Identity,
}
pub(crate) struct ManagedNamespaceDirectories {
    binding: u64,
    accounts: OwnedHandle,
    accounts_identity: Identity,
    namespace: OwnedHandle,
    namespace_identity: Identity,
    managed: Vec<RetainedDirectory>,
}
pub(crate) struct ManagedDirectoryCapability {
    handle: OwnedHandle,
    identity: Identity,
    binding: u64,
    area: ManagedUserArea,
    accounts_identity: Identity,
    namespace_identity: Identity,
}
pub(crate) struct ManagedFileCapability {
    handle: OwnedHandle,
    identity: Identity,
    binding: u64,
    area: ManagedUserArea,
    relative_name: String,
    parent_identity: Identity,
}
pub(crate) struct NamespaceFs {
    root: OwnedHandle,
    root_identity: Identity,
    lock: OwnedHandle,
    lock_identity: Identity,
    user: String,
    binding: u64,
}

impl NamespaceFs {
    pub(crate) fn open_data_root(path: &Path) -> Result<DataRootCapability> {
        let handle = open_absolute_directory(path)?;
        let identity = identify(&handle, true)?;
        let lock = open_lock(&handle)?;
        let lock_identity = identify(&lock, false)?;
        Ok(DataRootCapability {
            handle,
            identity,
            display_root: path.to_owned(),
            lock,
            lock_identity,
        })
    }

    pub(crate) fn for_namespace(
        root: &DataRootCapability,
        namespace: &UserNamespace,
    ) -> Result<Self> {
        ensure!(
            namespace.root()
                == root
                    .display_root
                    .join("accounts")
                    .join(namespace.user_public_id()),
            "namespace was not resolved from this data-root capability"
        );
        ensure!(
            identify(&root.handle, true)? == root.identity,
            "retained data-root identity changed"
        );
        let current = open_absolute_directory(&root.display_root)?;
        ensure!(
            identify(&current, true)? == root.identity,
            "configured data root has been replaced"
        );
        Ok(Self {
            root: root.handle.try_clone()?,
            root_identity: root.identity,
            lock: root.lock.try_clone()?,
            lock_identity: root.lock_identity,
            user: namespace.user_public_id().to_owned(),
            binding: BINDING_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub(crate) fn ensure_managed_dirs(&self) -> Result<ManagedNamespaceDirectories> {
        let _guard = self.lock_mutations()?;
        self.validate_root()?;
        let accounts = open_directory(&self.root, OsStr::new("accounts"), true)?;
        let accounts_identity = identify(&accounts, true)?;
        let namespace = open_directory(&accounts, OsStr::new(&self.user), true)?;
        let namespace_identity = identify(&namespace, true)?;
        let mut managed = Vec::with_capacity(MANAGED_USER_AREAS.len());
        for area in MANAGED_USER_AREAS {
            let handle = walk_directories(&namespace, area.relative_path(), true)?;
            let identity = identify(&handle, true)?;
            managed.push(RetainedDirectory {
                area,
                handle,
                identity,
            });
        }
        let current = self.reopen_namespace(accounts_identity, namespace_identity)?;
        for retained in &managed {
            let attached = walk_directories(&current, retained.area.relative_path(), false)?;
            ensure!(
                identify(&attached, true)? == retained.identity,
                "managed directory moved while opening capabilities"
            );
        }
        Ok(ManagedNamespaceDirectories {
            binding: self.binding,
            accounts,
            accounts_identity,
            namespace,
            namespace_identity,
            managed,
        })
    }

    pub(crate) fn open_managed_dir(
        &self,
        directories: &ManagedNamespaceDirectories,
        area: ManagedUserArea,
    ) -> Result<ManagedDirectoryCapability> {
        let _guard = self.lock_mutations()?;
        ensure!(
            directories.binding == self.binding,
            "directory set belongs to another authority"
        );
        ensure!(identify(&directories.accounts, true)? == directories.accounts_identity);
        ensure!(identify(&directories.namespace, true)? == directories.namespace_identity);
        let retained = directories
            .managed
            .iter()
            .find(|dir| dir.area == area)
            .ok_or_else(|| anyhow!("missing managed directory"))?;
        let directory = ManagedDirectoryCapability {
            handle: retained.handle.try_clone()?,
            identity: retained.identity,
            binding: self.binding,
            area,
            accounts_identity: directories.accounts_identity,
            namespace_identity: directories.namespace_identity,
        };
        self.validate_directory(&directory)?;
        Ok(directory)
    }

    pub(crate) fn open_existing_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        self.open_file(directory, name, false)
    }

    pub(crate) fn create_new_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        self.open_file(directory, name, true)
    }

    fn open_file(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
        create: bool,
    ) -> Result<ManagedFileCapability> {
        let _guard = self.lock_mutations()?;
        self.validate_directory(directory)?;
        let (parent, leaf) = relative_parent(&directory.handle, name)?;
        self.open_file_at(directory, name, parent, &leaf, create)
    }

    fn open_file_at(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
        parent: OwnedHandle,
        leaf: &OsStr,
        create: bool,
    ) -> Result<ManagedFileCapability> {
        let parent_identity = identify(&parent, true)?;
        let handle = open_regular(&parent, leaf, if create { NT_CREATE } else { NT_OPEN })?;
        let identity = identify(&handle, false)?;
        Ok(ManagedFileCapability {
            handle,
            identity,
            binding: self.binding,
            area: directory.area,
            relative_name: name.0.clone(),
            parent_identity,
        })
    }

    pub(crate) fn rename_within(
        &self,
        directory: &ManagedDirectoryCapability,
        mut source: ManagedFileCapability,
        destination: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        let _guard = self.lock_mutations()?;
        self.validate_directory(directory)?;
        let _source_location = self.validate_file(directory, &source)?;
        let (parent, leaf) = relative_parent(&directory.handle, destination)?;
        let parent_identity = identify(&parent, true)?;
        let relative_name = destination.0.clone();
        rename_handle(&source.handle, &parent, &leaf, false)?;
        // Kernel rename is the commit point: no fallible reopen or validation.
        source.relative_name = relative_name;
        source.parent_identity = parent_identity;
        Ok(source)
    }

    pub(crate) fn replace_within(
        &self,
        directory: &ManagedDirectoryCapability,
        mut source: ManagedFileCapability,
        destination: ManagedFileCapability,
    ) -> Result<ManagedFileCapability> {
        let _guard = self.lock_mutations()?;
        self.validate_directory(directory)?;
        let _source_location = self.validate_file(directory, &source)?;
        let (parent, leaf) = self.validate_file(directory, &destination)?;
        ensure!(
            source.identity != destination.identity,
            "cannot replace a file with itself"
        );
        // POSIX replacement keeps the expected destination handle valid through
        // commit. Classic FileRenameInfo cannot replace an open destination.
        rename_handle(&source.handle, &parent, &leaf, true)?;
        source.relative_name = destination.relative_name;
        source.parent_identity = destination.parent_identity;
        Ok(source)
    }

    pub(crate) fn unlink_within(
        &self,
        directory: &ManagedDirectoryCapability,
        file: ManagedFileCapability,
    ) -> Result<()> {
        let _guard = self.lock_mutations()?;
        self.validate_directory(directory)?;
        let _location = self.validate_file(directory, &file)?;
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
        };
        check_bool(unsafe {
            SetFileInformationByHandle(
                file.handle.as_raw_handle(),
                FileDispositionInfoEx,
                &disposition as *const _ as *const c_void,
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        })
        .context("unlink the retained managed file handle")
    }

    fn validate_root(&self) -> Result<()> {
        ensure!(
            identify(&self.root, true)? == self.root_identity,
            "retained data root changed"
        );
        Ok(())
    }

    fn reopen_namespace(
        &self,
        accounts_identity: Identity,
        namespace_identity: Identity,
    ) -> Result<OwnedHandle> {
        self.validate_root()?;
        let accounts = open_directory(&self.root, OsStr::new("accounts"), false)?;
        ensure!(
            identify(&accounts, true)? == accounts_identity,
            "accounts directory changed"
        );
        let namespace = open_directory(&accounts, OsStr::new(&self.user), false)?;
        ensure!(
            identify(&namespace, true)? == namespace_identity,
            "namespace directory changed"
        );
        Ok(namespace)
    }

    fn validate_directory(&self, directory: &ManagedDirectoryCapability) -> Result<()> {
        ensure!(
            directory.binding == self.binding,
            "directory belongs to another authority"
        );
        ensure!(
            identify(&directory.handle, true)? == directory.identity,
            "retained directory changed"
        );
        let namespace =
            self.reopen_namespace(directory.accounts_identity, directory.namespace_identity)?;
        let current = walk_directories(&namespace, directory.area.relative_path(), false)?;
        ensure!(
            identify(&current, true)? == directory.identity,
            "managed directory moved"
        );
        Ok(())
    }

    fn validate_file(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &ManagedFileCapability,
    ) -> Result<(OwnedHandle, OsString)> {
        ensure!(
            file.binding == self.binding && file.area == directory.area,
            "file belongs to another directory authority"
        );
        ensure!(
            identify(&file.handle, false)? == file.identity,
            "retained file identity changed"
        );
        let name = ManagedRelativeName::try_from(file.relative_name.as_str())?;
        let (parent, leaf) = relative_parent(&directory.handle, &name)?;
        ensure!(
            identify(&parent, true)? == file.parent_identity,
            "file parent changed"
        );
        let current = open_regular(&parent, &leaf, NT_OPEN)?;
        ensure!(
            identify(&current, false)? == file.identity,
            "stale managed file capability"
        );
        Ok((parent, leaf))
    }

    fn lock_mutations(&self) -> Result<MutationLock> {
        self.validate_root()?;
        ensure!(
            identify(&self.lock, false)? == self.lock_identity,
            "retained lock changed"
        );
        let fresh = open_lock(&self.root)?;
        ensure!(
            identify(&fresh, false)? == self.lock_identity,
            "namespace lock replaced"
        );
        MutationLock::acquire(fresh, false)
    }
}

struct MutationLock {
    handle: OwnedHandle,
}
impl MutationLock {
    fn acquire(handle: OwnedHandle, nonblocking: bool) -> Result<Self> {
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        let flags = LOCKFILE_EXCLUSIVE_LOCK
            | if nonblocking {
                LOCKFILE_FAIL_IMMEDIATELY
            } else {
                0
            };
        check_bool(unsafe {
            LockFileEx(
                handle.as_raw_handle(),
                flags,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        })
        .context("lock managed namespace mutations")?;
        Ok(Self { handle })
    }
}
impl Drop for MutationLock {
    fn drop(&mut self) {
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        // Closing also releases locks if the explicit unlock itself fails.
        unsafe {
            UnlockFileEx(
                self.handle.as_raw_handle(),
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            );
        }
    }
}

fn check_bool(value: i32) -> Result<()> {
    if value == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

fn identify(handle: &OwnedHandle, directory: bool) -> Result<Identity> {
    let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
    check_bool(unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            &mut tag as *mut _ as *mut c_void,
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    })?;
    ensure!(
        tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 && tag.ReparseTag == 0,
        "reparse points are not namespace authority"
    );
    ensure!(
        (tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) == directory,
        "wrong managed object type"
    );
    ensure!(
        unsafe { GetFileType(handle.as_raw_handle()) } == FILE_TYPE_DISK,
        "managed object is not disk storage"
    );
    if !directory {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        check_bool(unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) })?;
        ensure!(
            info.nNumberOfLinks == 1,
            "hardlinked or unlinked files are not managed regular files"
        );
    }
    let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    check_bool(unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileIdInfo,
            &mut id as *mut _ as *mut c_void,
            size_of::<FILE_ID_INFO>() as u32,
        )
    })?;
    Ok(Identity {
        volume: id.VolumeSerialNumber,
        file: id.FileId.Identifier,
    })
}

fn leaf_wide(name: &OsStr) -> Result<Vec<u16>> {
    // Reject ill-formed UTF-16 conservatively instead of lossy normalization.
    let text = name
        .to_str()
        .ok_or_else(|| anyhow!("invalid Unicode filename"))?;
    validate_windows_relative_name(text)?;
    ensure!(
        !text.contains(['/', '\\']),
        "relative open requires one component"
    );
    let wide: Vec<u16> = name.encode_wide().collect();
    ensure!(wide.len() <= u16::MAX as usize / 2, "filename is too long");
    Ok(wide)
}

fn nt_open(
    parent: &OwnedHandle,
    name: &OsStr,
    access: u32,
    share: u32,
    disposition: u32,
    directory: bool,
) -> Result<OwnedHandle> {
    let mut wide = leaf_wide(name)?;
    let mut unicode = UnicodeString {
        length: (wide.len() * 2) as u16,
        maximum_length: (wide.len() * 2) as u16,
        buffer: wide.as_mut_ptr(),
    };
    let mut attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: parent.as_raw_handle(),
        object_name: &mut unicode,
        attributes: 0x40,
        security_descriptor: null_mut(),
        security_quality_of_service: null_mut(),
    };
    let mut status = IoStatusBlock {
        status_or_pointer: 0,
        information: 0,
    };
    let mut raw = null_mut();
    let result = unsafe {
        NtCreateFile(
            &mut raw,
            access,
            &mut attributes,
            &mut status,
            null_mut(),
            if directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                FILE_ATTRIBUTE_NORMAL
            },
            share,
            disposition,
            NT_OPEN_REPARSE_POINT
                | NT_SYNCHRONOUS
                | if directory {
                    NT_DIRECTORY
                } else {
                    NT_NON_DIRECTORY
                },
            null_mut(),
            0,
        )
    };
    if result < 0 {
        return Err(std::io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32
        )
        .into());
    }
    ensure!(
        !raw.is_null() && raw != INVALID_HANDLE_VALUE,
        "invalid native namespace handle"
    );
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    identify(&handle, directory)?;
    Ok(handle)
}

fn open_directory(parent: &OwnedHandle, name: &OsStr, create: bool) -> Result<OwnedHandle> {
    nt_open(
        parent,
        name,
        MANAGED_ACCESS,
        SHARE_ALL,
        if create { NT_OPEN_IF } else { NT_OPEN },
        true,
    )
}
fn open_regular(parent: &OwnedHandle, name: &OsStr, disposition: u32) -> Result<OwnedHandle> {
    nt_open(parent, name, REGULAR_ACCESS, SHARE_ALL, disposition, false)
}
fn open_lock(root: &OwnedHandle) -> Result<OwnedHandle> {
    nt_open(
        root,
        OsStr::new(LOCK_NAME),
        FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARE_LOCK,
        NT_OPEN_IF,
        false,
    )
}

fn open_absolute_directory(path: &Path) -> Result<OwnedHandle> {
    ensure!(path.is_absolute(), "data root must be absolute");
    let mut components = path.components();
    let prefix = match components.next() {
        Some(Component::Prefix(prefix)) => prefix,
        _ => {
            return Err(anyhow!(
                "data root requires an absolute drive or UNC anchor"
            ))
        }
    };
    ensure!(
        matches!(
            prefix.kind(),
            Prefix::Disk(_)
                | Prefix::VerbatimDisk(_)
                | Prefix::UNC(_, _)
                | Prefix::VerbatimUNC(_, _)
        ),
        "unsupported Windows namespace prefix"
    );
    ensure!(
        matches!(components.next(), Some(Component::RootDir)),
        "missing absolute Windows root"
    );
    let names = components
        .map(|part| match part {
            Component::Normal(name) => {
                leaf_wide(name)?;
                Ok(name.to_os_string())
            }
            _ => Err(anyhow!("non-normal data-root component")),
        })
        .collect::<Result<Vec<_>>>()?;
    let mut anchor = PathBuf::from(prefix.as_os_str());
    anchor.push("\\");
    let wide: Vec<u16> = anchor.as_os_str().encode_wide().chain(Some(0)).collect();
    ensure!(
        !wide[..wide.len() - 1].contains(&0),
        "NUL in Windows anchor"
    );
    // The sole path-based open targets only the drive/share anchor. All remaining
    // names are opened one component at a time relative to a retained handle.
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            if names.is_empty() {
                MANAGED_ACCESS
            } else {
                TRAVERSE_ACCESS
            },
            SHARE_ALL,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    ensure!(
        raw != INVALID_HANDLE_VALUE,
        "open Windows anchor: {}",
        std::io::Error::last_os_error()
    );
    let mut handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    identify(&handle, true)?;
    for (index, name) in names.iter().enumerate() {
        handle = nt_open(
            &handle,
            name,
            if index + 1 == names.len() {
                MANAGED_ACCESS
            } else {
                TRAVERSE_ACCESS
            },
            SHARE_ALL,
            NT_OPEN,
            true,
        )?;
    }
    Ok(handle)
}

fn walk_directories(root: &OwnedHandle, relative: &str, create: bool) -> Result<OwnedHandle> {
    validate_windows_relative_name(relative)?;
    let mut handle = root.try_clone()?;
    for component in relative.split(['/', '\\']) {
        handle = open_directory(&handle, OsStr::new(component), create)?;
    }
    Ok(handle)
}

fn relative_parent(
    root: &OwnedHandle,
    name: &ManagedRelativeName,
) -> Result<(OwnedHandle, OsString)> {
    validate_windows_relative_name(&name.0)?;
    let mut parts: Vec<_> = name.0.split(['/', '\\']).collect();
    let leaf = OsString::from(parts.pop().ok_or_else(|| anyhow!("missing managed leaf"))?);
    let mut parent = root.try_clone()?;
    for part in parts {
        parent = open_directory(&parent, OsStr::new(part), false)?;
    }
    Ok((parent, leaf))
}

fn rename_handle(
    source: &OwnedHandle,
    parent: &OwnedHandle,
    leaf: &OsStr,
    replace: bool,
) -> Result<()> {
    let wide = leaf_wide(leaf)?;
    let bytes = (offset_of!(FILE_RENAME_INFO, FileName) + wide.len() * 2)
        .max(size_of::<FILE_RENAME_INFO>());
    // usize backing provides HANDLE/structure alignment, including short names.
    let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.Flags = if replace {
            RENAME_REPLACE | RENAME_POSIX
        } else {
            0
        };
        (*info).RootDirectory = parent.as_raw_handle();
        (*info).FileNameLength = (wide.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            wide.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            wide.len(),
        );
        check_bool(SetFileInformationByHandle(
            source.as_raw_handle(),
            FileRenameInfoEx,
            info.cast(),
            bytes as u32,
        ))
    }
    .context("rename retained managed file (native extended semantics required)")
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::{identify, open_lock, relative_parent, MutationLock, LOCK_NAME};
    use std::fs;
    use std::os::windows::fs::{symlink_dir, symlink_file};

    const USER: &str = "11111111-1111-4111-8111-111111111111";

    fn fixture() -> (
        tempfile::TempDir,
        UserNamespace,
        NamespaceFs,
        ManagedDirectoryCapability,
    ) {
        let root = tempfile::tempdir().unwrap();
        let namespace = UserNamespace::new(root.path(), USER).unwrap();
        let capability = NamespaceFs::open_data_root(root.path()).unwrap();
        let fs = NamespaceFs::for_namespace(&capability, &namespace).unwrap();
        let dirs = fs.ensure_managed_dirs().unwrap();
        let out = fs.open_managed_dir(&dirs, ManagedUserArea::Output).unwrap();
        (root, namespace, fs, out)
    }

    fn name(value: &str) -> ManagedRelativeName {
        ManagedRelativeName::try_from(value).unwrap()
    }

    #[test]
    fn windows_create_rename_replace_unlink_lifecycle() {
        let (_root, ns, fs, out) = fixture();
        let source = fs.create_new_regular(&out, &name("source")).unwrap();
        fs::write(ns.output_dir().join("source"), b"private").unwrap();
        let renamed = fs.rename_within(&out, source, &name("renamed")).unwrap();
        assert!(!ns.output_dir().join("source").exists());
        assert_eq!(
            fs::read(ns.output_dir().join("renamed")).unwrap(),
            b"private"
        );
        let destination = fs.create_new_regular(&out, &name("destination")).unwrap();
        fs::write(ns.output_dir().join("destination"), b"old").unwrap();
        let replaced = fs.replace_within(&out, renamed, destination).unwrap();
        assert!(!ns.output_dir().join("renamed").exists());
        assert_eq!(
            fs::read(ns.output_dir().join("destination")).unwrap(),
            b"private"
        );
        fs.unlink_within(&out, replaced).unwrap();
        assert!(!ns.output_dir().join("destination").exists());
    }

    #[test]
    fn windows_no_replace_collision_preserves_both_files() {
        let (_root, ns, fs, out) = fixture();
        let source = fs.create_new_regular(&out, &name("source")).unwrap();
        fs::write(ns.output_dir().join("source"), b"source").unwrap();
        fs::write(ns.output_dir().join("occupied"), b"occupied").unwrap();
        assert!(fs.rename_within(&out, source, &name("occupied")).is_err());
        assert_eq!(fs::read(ns.output_dir().join("source")).unwrap(), b"source");
        assert_eq!(
            fs::read(ns.output_dir().join("occupied")).unwrap(),
            b"occupied"
        );
    }

    #[test]
    fn windows_stale_capability_cannot_delete_or_replace_new_file() {
        let (_root, ns, fs, out) = fixture();
        for replace in [false, true] {
            let stale = fs.create_new_regular(&out, &name("stale")).unwrap();
            fs::rename(ns.output_dir().join("stale"), ns.output_dir().join("held")).unwrap();
            fs::write(ns.output_dir().join("stale"), b"replacement").unwrap();
            if replace {
                let source = fs.create_new_regular(&out, &name("source")).unwrap();
                assert!(fs.replace_within(&out, source, stale).is_err());
                assert!(ns.output_dir().join("source").is_file());
            } else {
                assert!(fs.unlink_within(&out, stale).is_err());
            }
            assert_eq!(
                fs::read(ns.output_dir().join("stale")).unwrap(),
                b"replacement"
            );
            fs::remove_file(ns.output_dir().join("stale")).unwrap();
            fs::remove_file(ns.output_dir().join("held")).unwrap();
        }
    }

    #[test]
    fn windows_reparse_accounts_account_and_managed_directories_are_rejected() {
        for relative in [
            "accounts".to_owned(),
            format!("accounts/{USER}"),
            format!("accounts/{USER}/out"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let external = tempfile::tempdir().unwrap();
            let link = root.path().join(relative);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink_dir(external.path(), &link)
                .expect("Windows test runner requires Developer Mode or symlink privilege");
            let cap = NamespaceFs::open_data_root(root.path()).unwrap();
            let ns = UserNamespace::new(root.path(), USER).unwrap();
            let result =
                NamespaceFs::for_namespace(&cap, &ns).and_then(|fs| fs.ensure_managed_dirs());
            assert!(result.is_err());
            assert!(fs::read_dir(external.path()).unwrap().next().is_none());
        }
    }

    #[test]
    fn windows_reparse_leaf_nested_parent_and_wrong_type_are_rejected() {
        let (_root, ns, fs, out) = fixture();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("secret"), b"outside").unwrap();
        symlink_file(
            external.path().join("secret"),
            ns.output_dir().join("linked"),
        )
        .unwrap();
        symlink_dir(external.path(), ns.output_dir().join("nested")).unwrap();
        fs::create_dir(ns.output_dir().join("directory")).unwrap();
        for leaf in ["linked", "nested/secret", "directory"] {
            assert!(fs.open_existing_regular(&out, &name(leaf)).is_err());
            assert!(fs.create_new_regular(&out, &name(leaf)).is_err());
        }
        assert!(fs
            .create_new_regular(&out, &name("nested/created"))
            .is_err());
        assert!(!external.path().join("created").exists());
        assert_eq!(
            fs::read(external.path().join("secret")).unwrap(),
            b"outside"
        );
    }

    #[test]
    fn windows_root_reparse_ancestor_and_rebound_root_are_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::create_dir(external.path().join("root")).unwrap();
        symlink_dir(external.path(), parent.path().join("alias")).unwrap();
        assert!(NamespaceFs::open_data_root(&parent.path().join("alias/root")).is_err());
        assert!(NamespaceFs::open_data_root(&parent.path().join("alias")).is_err());
        let root = parent.path().join("root");
        fs::create_dir(&root).unwrap();
        let cap = NamespaceFs::open_data_root(&root).unwrap();
        if let Err(error) = fs::rename(&root, parent.path().join("held")) {
            // Windows may pin an ancestor when the non-delete-sharing lock is
            // open. That is also a safe outcome: no replacement tree exists.
            assert!(matches!(error.raw_os_error(), Some(5 | 32)), "{error}");
            assert!(!parent.path().join("held").exists());
            assert!(
                NamespaceFs::for_namespace(&cap, &UserNamespace::new(&root, USER).unwrap()).is_ok()
            );
            return;
        }
        fs::create_dir(&root).unwrap();
        let ns = UserNamespace::new(&root, USER).unwrap();
        assert!(NamespaceFs::for_namespace(&cap, &ns).is_err());
        assert!(!root.join("accounts").exists());
    }

    #[test]
    fn windows_swapped_accounts_cannot_redirect_create() {
        let (root, _ns, fs, out) = fixture();
        let external = tempfile::tempdir().unwrap();
        fs::rename(root.path().join("accounts"), root.path().join("held")).unwrap();
        symlink_dir(external.path(), root.path().join("accounts")).unwrap();
        assert!(fs.create_new_regular(&out, &name("result")).is_err());
        assert!(fs::read_dir(external.path()).unwrap().next().is_none());
    }

    #[test]
    fn windows_independent_instances_serialize_and_pin_the_same_lock_file() {
        let (root, ns, first, _out) = fixture();
        let cap = NamespaceFs::open_data_root(root.path()).unwrap();
        let second = NamespaceFs::for_namespace(&cap, &ns).unwrap();
        let held = first.lock_mutations().unwrap();
        let contender = open_lock(&second.root).unwrap();
        assert!(MutationLock::acquire(contender, true).is_err());
        assert!(fs::remove_file(root.path().join(LOCK_NAME)).is_err());
        assert!(fs::rename(root.path().join(LOCK_NAME), root.path().join("old-lock")).is_err());
        drop(held);
        let acquired = MutationLock::acquire(open_lock(&second.root).unwrap(), true).unwrap();
        drop(acquired);
        // Lock identity stays pinned between operations, too.
        assert!(fs::remove_file(root.path().join(LOCK_NAME)).is_err());
        assert!(!root.path().join("old-lock").exists());
    }

    #[test]
    fn windows_reparse_and_hardlinked_lock_files_are_rejected() {
        let external = tempfile::NamedTempFile::new().unwrap();
        for hardlink in [false, true] {
            let root = tempfile::tempdir().unwrap();
            if hardlink {
                fs::hard_link(external.path(), root.path().join(LOCK_NAME)).unwrap();
            } else {
                symlink_file(external.path(), root.path().join(LOCK_NAME)).unwrap();
            }
            assert!(NamespaceFs::open_data_root(root.path()).is_err());
            assert!(!root.path().join("accounts").exists());
        }
        assert_eq!(fs::metadata(external.path()).unwrap().len(), 0);
    }

    #[test]
    fn windows_create_after_parent_open_stays_in_retained_tree() {
        let (root, _ns, fs, out) = fixture();
        let external = tempfile::tempdir().unwrap();
        let leaf_name = name("result");
        let _guard = fs.lock_mutations().unwrap();
        fs.validate_directory(&out).unwrap();
        let (parent, leaf) = relative_parent(&out.handle, &leaf_name).unwrap();
        fs::rename(root.path().join("accounts"), root.path().join("held")).unwrap();
        symlink_dir(external.path(), root.path().join("accounts")).unwrap();
        // Invoke the same retained-parent operation used by the public create,
        // after deterministically swapping the already-validated ancestor.
        let created = fs
            .open_file_at(&out, &leaf_name, parent, &leaf, true)
            .unwrap();
        assert!(identify(&created.handle, false).unwrap() == created.identity);
        assert!(root
            .path()
            .join("held")
            .join(USER)
            .join("out/result")
            .is_file());
        assert!(fs::read_dir(external.path()).unwrap().next().is_none());
    }

    #[test]
    fn windows_nested_parent_swap_and_stale_source_cannot_redirect_rename() {
        let (_root, ns, fs, out) = fixture();
        let external = tempfile::tempdir().unwrap();
        fs::create_dir(ns.output_dir().join("nested")).unwrap();
        let source = fs.create_new_regular(&out, &name("nested/source")).unwrap();
        fs::write(ns.output_dir().join("nested/source"), b"private").unwrap();
        fs::rename(ns.output_dir().join("nested"), ns.output_dir().join("held")).unwrap();
        symlink_dir(external.path(), ns.output_dir().join("nested")).unwrap();
        assert!(fs
            .rename_within(&out, source, &name("destination"))
            .is_err());
        assert_eq!(
            fs::read(ns.output_dir().join("held/source")).unwrap(),
            b"private"
        );
        assert!(fs::read_dir(external.path()).unwrap().next().is_none());
        let stale = fs.create_new_regular(&out, &name("source")).unwrap();
        fs::rename(
            ns.output_dir().join("source"),
            ns.output_dir().join("old-source"),
        )
        .unwrap();
        fs::write(ns.output_dir().join("source"), b"replacement").unwrap();
        assert!(fs.rename_within(&out, stale, &name("destination")).is_err());
        assert_eq!(
            fs::read(ns.output_dir().join("source")).unwrap(),
            b"replacement"
        );
        assert!(!ns.output_dir().join("destination").exists());
    }

    #[test]
    fn windows_renamed_capability_retains_identity_after_external_name_change() {
        use std::io::Read;
        let (_root, ns, fs, out) = fixture();
        let source = fs.create_new_regular(&out, &name("a")).unwrap();
        let original = source.identity;
        fs::write(ns.output_dir().join("a"), b"private").unwrap();
        // One-letter destination also exercises the variable-length native
        // rename buffer's minimum structure allocation.
        let renamed = fs.rename_within(&out, source, &name("b")).unwrap();
        assert!(renamed.identity == original);
        fs::rename(ns.output_dir().join("b"), ns.output_dir().join("c")).unwrap();
        fs::write(ns.output_dir().join("b"), b"replacement").unwrap();
        let mut retained = fs::File::from(renamed.handle.try_clone().unwrap());
        let mut bytes = Vec::new();
        retained.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"private");
        assert!(fs.unlink_within(&out, renamed).is_err());
        assert_eq!(fs::read(ns.output_dir().join("b")).unwrap(), b"replacement");
    }

    #[test]
    fn windows_foreign_capabilities_fail_without_mutation() {
        let (_root, ns, fs, out) = fixture();
        let (_other_root, other_ns, other, other_out) = fixture();
        let source = fs.create_new_regular(&out, &name("source")).unwrap();
        assert!(other
            .rename_within(&other_out, source, &name("destination"))
            .is_err());
        assert!(ns.output_dir().join("source").is_file());
        assert!(!other_ns.output_dir().join("destination").exists());
        let foreign_name = name("foreign");
        assert!(other.create_new_regular(&out, &foreign_name).is_err());
        assert!(!ns.output_dir().join("foreign").exists());
    }
}
