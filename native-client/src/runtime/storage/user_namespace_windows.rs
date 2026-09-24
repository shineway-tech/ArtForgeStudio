//! Windows retained-handle namespace implementation.

use super::{
    reject_reserved_account_path, reject_mapped_private_identity, validate_export_leaf, validate_export_volume_root_name, validate_windows_relative_name,
    ManagedFileCheck, ManagedFileKey, ManagedFileMetadata, ManagedPublication,
    ManagedPublicationConflict, ManagedReadSeek, ManagedRelativeName, ManagedUserArea, ManagedWriteState,
    StableFileIdentity, UserNamespace, MANAGED_USER_AREAS,
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

impl DataRootCapability {
    pub(super) fn read_external_source(&self, path: &Path, limit: u64) -> Result<Vec<u8>> {
        reject_reserved_account_path(path)?;
        use std::io::Read;
        ensure!(path.is_absolute(), "image source must be absolute");
        let mut components = path.components();
        let prefix = match components.next() {
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => prefix,
            _ => anyhow::bail!("external image requires a provable local volume root"),
        };
        ensure!(matches!(components.next(), Some(Component::RootDir)), "image volume root missing");
        let text = path.to_str().ok_or_else(|| anyhow!("invalid image Unicode"))?;
        let prefix_text = prefix.as_os_str().to_str().ok_or_else(|| anyhow!("invalid prefix"))?;
        validate_windows_relative_name(&text[prefix_text.len() + 1..])?;
        let names = components.map(|part| match part {
            Component::Normal(name) => { leaf_wide(name)?; Ok(name.to_owned()) },
            _ => Err(anyhow!("unclean image source")),
        }).collect::<Result<Vec<_>>>()?;
        let (leaf, parents) = names.split_last().ok_or_else(|| anyhow!("image filename missing"))?;
        let mut anchor = PathBuf::from(prefix.as_os_str()); anchor.push("\\");
        let root = open_absolute_external_anchor(&anchor, TRAVERSE_ACCESS, SHARE_LOCK)?;
        prove_external_volume_root(&root)?;
        let mut chain = vec![root];
        for name in parents {
            chain.push(nt_open_external(chain.last().unwrap(), name, TRAVERSE_ACCESS, SHARE_LOCK, NT_OPEN, true)?);
        }
        ensure!(identify(&self.handle, true)? == self.identity, "private root changed");
        reject_external_source_chain(&chain, self.identity)?;
        let handle = nt_open_external(chain.last().unwrap(), leaf,
            FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_SHARE_READ, NT_OPEN, false)?;
        identify_external(&handle, false)?;
        ensure!(file_link_count(&handle)? == 1, "linked image source");
        // Read-only sharing pins this exact regular object against write/delete; all
        // ancestor handles deny deletion. No pathname is reopened for the bytes.
        let mut file = std::fs::File::from(handle);
        let before = file.metadata()?;
        ensure!(before.len() <= limit, "image source too large");
        let mut bytes = Vec::new();
        (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
        reject_external_source_chain(&chain, self.identity)?;
        ensure!(bytes.len() as u64 == before.len() && bytes.len() as u64 <= limit, "image source size changed");
        Ok(bytes)
    }
}
const LOCK_NAME: &str = ".namespace.lock";
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const SHARE_LOCK: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
// Do not require write or directory-listing rights on C:\ or its ancestors.
const TRAVERSE_ACCESS: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const MANAGED_ACCESS: u32 = TRAVERSE_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;
// Enumeration rights are requested only for private managed traversal.
const CHECKED_MANAGED_ACCESS: u32 = MANAGED_ACCESS | FILE_LIST_DIRECTORY;
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
const FILE_RENAME_INFORMATION_EX: u32 = 65;
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
    fn NtSetInformationFile(
        handle: HANDLE,
        status: *mut IoStatusBlock,
        information: *const c_void,
        length: u32,
        information_class: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Identity {
    volume: u64,
    file: [u8; 16],
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExternalIdentity {
    Extended(Identity),
    Legacy { volume: u32, file: u64 },
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
    write_state: ManagedWriteState,
}
pub(crate) struct NamespaceFs {
    root: OwnedHandle,
    root_identity: Identity,
    lock: OwnedHandle,
    lock_identity: Identity,
    user: String,
    binding: u64,
    namespace: UserNamespace,
    _mapped_chains: Vec<Vec<OwnedHandle>>,
}

/// A pinned chain from a proven volume root to the external directory. Keeping
/// every parent open without delete sharing preserves the ancestry proof;
/// native NT relative names have no supported general-purpose `..` traversal.
pub(crate) struct ExternalExportDestination {
    chain: Vec<OwnedHandle>,
    private_root: OwnedHandle,
    display_path: PathBuf,
}

impl ExternalExportDestination {
    pub(crate) fn open(data_root: &DataRootCapability, candidate: &Path) -> Result<Self> {
        reject_reserved_account_path(candidate)?;
        ensure!(
            candidate.is_absolute(),
            "export destination must be absolute"
        );
        let mut components = candidate.components();
        let prefix = match components.next() {
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) =>
            {
                prefix
            }
            _ => {
                return Err(anyhow!(
                    "export requires a provable local drive root; UNC is unsupported"
                ))
            }
        };
        ensure!(
            matches!(components.next(), Some(Component::RootDir)),
            "missing export drive root"
        );
        let mut anchor = PathBuf::from(prefix.as_os_str());
        anchor.push("\\");
        let names = components
            .map(|part| match part {
                Component::Normal(name) => {
                    leaf_wide(name)?;
                    Ok(name.to_os_string())
                }
                _ => Err(anyhow!("unclean export path")),
            })
            .collect::<Result<Vec<_>>>()?;
        // components() normalizes dots and repeated separators in DOS paths;
        // inspect the original suffix as well so no unclean input is accepted.
        let text = candidate
            .to_str()
            .ok_or_else(|| anyhow!("invalid export Unicode"))?;
        let prefix_text = prefix
            .as_os_str()
            .to_str()
            .ok_or_else(|| anyhow!("invalid export prefix"))?;
        let suffix = &text[prefix_text.len() + 1..];
        if !suffix.is_empty() {
            validate_windows_relative_name(suffix)?;
        }
        ensure!(
            identify(&data_root.handle, true)? == data_root.identity,
            "private root identity changed"
        );
        let root = open_absolute_anchor(&anchor, TRAVERSE_ACCESS, SHARE_LOCK)?;
        prove_external_volume_root(&root)?;
        let mut chain = vec![root];
        let mut missing = names.len();
        for (index, name) in names.iter().enumerate() {
            match nt_open(
                chain.last().unwrap(),
                name,
                TRAVERSE_ACCESS,
                SHARE_LOCK,
                NT_OPEN,
                true,
            ) {
                Ok(next) => chain.push(next),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    missing = index;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        reject_private_chain(&chain, data_root.identity)?;
        // Upgrade only the closest existing parent. Ancestors need traversal
        // rights, not write/list access. Relative children use the pinned parent.
        // For a volume root, retain the original handle while opening the anchor
        // with write access, then prove identity before any directory creation.
        let current = chain.last().unwrap();
        let writable = if chain.len() == 1 {
            let root = open_absolute_anchor(&anchor, MANAGED_ACCESS, SHARE_LOCK)?;
            prove_external_volume_root(&root)?;
            root
        } else {
            nt_open(
                &chain[chain.len() - 2],
                &names[chain.len() - 2],
                MANAGED_ACCESS,
                SHARE_LOCK,
                NT_OPEN,
                true,
            )?
        };
        ensure!(
            identify(current, true)? == identify(&writable, true)?,
            "external parent identity changed"
        );
        *chain.last_mut().unwrap() = writable;
        for name in &names[missing..] {
            reject_private_chain(&chain, data_root.identity)?;
            let next = nt_open(
                chain.last().unwrap(),
                name,
                MANAGED_ACCESS,
                SHARE_LOCK,
                NT_CREATE,
                true,
            )?;
            chain.push(next);
        }
        reject_private_chain(&chain, data_root.identity)?;
        let display_path = names.iter().fold(anchor, |path, name| path.join(name));
        Ok(Self {
            chain,
            private_root: data_root.handle.try_clone()?,
            display_path,
        })
    }

    /// Display/serialization metadata only; all I/O uses the retained chain.
    pub(crate) fn normalized_display_path(&self) -> &Path {
        &self.display_path
    }

    pub(crate) fn create_new_directory(&self, name: &str) -> Result<Self> {
        reject_reserved_account_path(&self.display_path.join(name))?;
        validate_export_leaf(name)?;
        self.validate()?;
        let mut chain = self
            .chain
            .iter()
            .map(OwnedHandle::try_clone)
            .collect::<std::io::Result<Vec<_>>>()?;
        let next = nt_open(
            chain.last().unwrap(),
            OsStr::new(name),
            MANAGED_ACCESS,
            SHARE_LOCK,
            NT_CREATE,
            true,
        )?;
        chain.push(next);
        Ok(Self {
            chain,
            private_root: self.private_root.try_clone()?,
            display_path: self.display_path.join(name),
        })
    }

    /// Sync the completed stream before a handle-based no-replace rename.
    /// Success returns bytes, not a pathname granting further I/O authority.
    pub(crate) fn write_new_file(
        &self,
        name: &str,
        stream: &mut impl std::io::Read,
    ) -> Result<u64> {
        self.write_new_file_with_temp(
            name,
            stream,
            &format!(".export-{}.tmp", uuid::Uuid::new_v4()),
        )
    }

    fn write_new_file_with_temp(
        &self,
        name: &str,
        stream: &mut impl std::io::Read,
        temporary: &str,
    ) -> Result<u64> {
        validate_export_leaf(name)?;
        validate_export_leaf(temporary)?;
        ensure!(
            !name.eq_ignore_ascii_case(temporary),
            "temporary name equals destination"
        );
        self.validate()?;
        let parent = self.chain.last().unwrap();
        // NT_CREATE failure owns no object and triggers no deletion. Denying
        // write/delete sharing protects our new temporary until publication.
        let handle = nt_open(
            parent,
            OsStr::new(temporary),
            REGULAR_ACCESS,
            FILE_SHARE_READ,
            NT_CREATE,
            false,
        )?;
        let result = (|| {
            let mut file = std::fs::File::from(handle.try_clone()?);
            let bytes = std::io::copy(stream, &mut file)?;
            file.sync_all()?;
            self.validate()?;
            identify(&handle, false)?;
            rename_handle(&handle, parent, OsStr::new(name), false)?;
            Ok(bytes)
        })();
        if result.is_err() {
            // Delete only the object we opened, never a name-based substitute.
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            check_bool(unsafe {
                SetFileInformationByHandle(
                    handle.as_raw_handle(),
                    FileDispositionInfo,
                    &disposition as *const _ as *const c_void,
                    size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            })
            .context("failed export temporary could not be removed")?;
        }
        result
    }

    fn validate(&self) -> Result<()> {
        reject_private_chain(&self.chain, identify(&self.private_root, true)?)
    }
}

fn prove_external_volume_root(handle: &OwnedHandle) -> Result<()> {
    let mut buffer = vec![0u16; 128];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            handle.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_GUID,
        )
    };
    ensure!(
        length != 0 && (length as usize) < buffer.len(),
        "external anchor ancestry cannot be proven"
    );
    let name = String::from_utf16(&buffer[..length as usize])?;
    // SUBST to a private descendant reports a suffix after the volume GUID;
    // shares/unsupported providers cannot supply the required local root proof.
    validate_export_volume_root_name(&name)
}

fn reject_private_chain(chain: &[OwnedHandle], private: Identity) -> Result<()> {
    let volume = identify(
        chain
            .first()
            .ok_or_else(|| anyhow!("missing external anchor"))?,
        true,
    )?
    .volume;
    for handle in chain.iter().rev() {
        let identity = identify(handle, true)?;
        reject_mapped_private_identity(StableFileIdentity::Windows { volume: identity.volume, file_id: identity.file })?;
        ensure!(
            identity != private,
            "export destination is inside private app storage"
        );
        ensure!(
            identity.volume == volume,
            "external export cannot cross a mount boundary"
        );
    }
    Ok(())
}

fn reject_external_source_chain(chain: &[OwnedHandle], private: Identity) -> Result<()> {
    let volume = identify_external(
        chain
            .first()
            .ok_or_else(|| anyhow!("missing external anchor"))?,
        true,
    )?;
    for handle in chain.iter().rev() {
        let identity = identify_external(handle, true)?;
        if let ExternalIdentity::Extended(identity) = identity {
            reject_mapped_private_identity(StableFileIdentity::Windows {
                volume: identity.volume,
                file_id: identity.file,
            })?;
            ensure!(identity != private, "external source is inside private app storage");
        }
        ensure!(identity.same_volume(volume), "external source cannot cross a mount boundary");
    }
    Ok(())
}

impl ExternalIdentity {
    fn same_volume(self, other: Self) -> bool {
        match (self, other) {
            (Self::Extended(left), Self::Extended(right)) => left.volume == right.volume,
            (Self::Legacy { volume: left, .. }, Self::Legacy { volume: right, .. }) => left == right,
            _ => false,
        }
    }
}

impl NamespaceFs {
    pub(super) fn verify_no_legacy_import_state(&self, directory: &ManagedDirectoryCapability) -> Result<()> {
        let _lock = self.lock_mutations()?;
        let chain = self.checked_directory_chain(directory)?;
        for entry in managed_entries(chain.last().unwrap())? {
            let name = entry.name.to_str().ok_or_else(|| anyhow!("unsupported staging entry requires repair"))?;
            ensure!(!name.starts_with("legacy-import-") && name != "import-plan.json",
                "unsupported legacy import journal requires repair");
        }
        self.checked_directory_chain(directory)?;
        Ok(())
    }
    pub(crate) fn open_optional_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
    ) -> Result<Option<ManagedFileCapability>> {
        let _guard = self.lock_mutations()?;
        let mut chain = self.checked_directory_chain(directory)?;
        let leaf = checked_relative_chain(&mut chain, directory.area, name)?;
        let parent = chain.last().unwrap();
        let Some(handle) = optional_checked_regular(parent, &leaf)? else {
            let mut current = self.checked_directory_chain(directory)?;
            checked_relative_chain(&mut current, directory.area, name)?;
            ensure!(
                identify(current.last().unwrap(), true)? == identify(parent, true)?,
                "missing leaf parent detached"
            );
            return Ok(None);
        };
        let identity = identify(&handle, false)?;
        let file = ManagedFileCapability {
            handle,
            identity,
            binding: self.binding,
            area: directory.area,
            relative_name: name.0.clone(),
            parent_identity: identify(parent, true)?,
            write_state: ManagedWriteState::Existing,
        };
        self.checked_file_chain(directory, &file)?;
        Ok(Some(file))
    }

    /// Streams must not reenter this namespace or perform network/UI work.
    /// Failed sinks retain partial effects and must be discarded by callers.
    pub(crate) fn read_regular_to(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &mut ManagedFileCapability,
        sink: &mut dyn std::io::Write,
    ) -> Result<u64> {
        self.with_regular_reader(directory, file, |reader| Ok(std::io::copy(reader, sink)?))
    }

    /// Worker-only bounded local reads/seeks and decode; no network/UI/reentry.
    /// A result is accepted only after the retained chain is post-validated.
    pub(crate) fn with_regular_reader<T>(
        &self, directory: &ManagedDirectoryCapability, file: &mut ManagedFileCapability,
        operation: impl FnOnce(&mut dyn ManagedReadSeek) -> Result<T>,
    ) -> Result<T> {
        use std::io::{Seek, SeekFrom};
        let _guard = self.lock_mutations()?;
        let _chain = self.checked_file_chain(directory, file)?;
        let mut stream = std::fs::File::from(file.handle.try_clone()?);
        stream.seek(SeekFrom::Start(0))?;
        let result = operation(&mut stream)?;
        self.checked_file_chain(directory, file)?;
        Ok(result)
    }

    /// Source reads run outside the mutation lock. Each bounded write and EOF
    /// revalidate retained authority; failure or unwind poisons the one attempt.
    pub(crate) fn write_new_regular_from(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &mut ManagedFileCapability,
        source: &mut dyn std::io::Read,
    ) -> Result<u64> {
        use std::io::{Seek, SeekFrom, Write};
        let mut stream = {
            let _guard = self.lock_mutations()?;
            let _chain = self.checked_file_chain(directory, file)?;
            ensure!(
                file.write_state == ManagedWriteState::New,
                "only an owned unwritten temporary can be written"
            );
            file.write_state = ManagedWriteState::Poisoned;
            let mut stream = std::fs::File::from(file.handle.try_clone()?);
            stream.seek(SeekFrom::Start(0))?;
            stream
        };
        let copied = super::copy_managed_chunks(source, |chunk| {
            let _guard = self.lock_mutations()?;
            let _chain = self.checked_file_chain(directory, file)?;
            stream.write_all(chunk)?;
            self.checked_file_chain(directory, file)?;
            Ok(())
        })?;
        let _guard = self.lock_mutations()?;
        let _chain = self.checked_file_chain(directory, file)?;
        file.write_state = ManagedWriteState::Written;
        Ok(copied)
    }

    pub(crate) fn sync_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &mut ManagedFileCapability,
    ) -> Result<()> {
        self.sync_regular_with(directory, file, |handle| {
            Ok(std::fs::File::from(handle.try_clone()?).sync_all()?)
        })
    }

    fn sync_regular_with(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &mut ManagedFileCapability,
        sync: impl FnOnce(&OwnedHandle) -> Result<()>,
    ) -> Result<()> {
        let _guard = self.lock_mutations()?;
        let _chain = self.checked_file_chain(directory, file)?;
        ensure!(
            file.write_state != ManagedWriteState::Poisoned,
            "failed write cannot be synced for publication"
        );
        let owned = matches!(
            file.write_state,
            ManagedWriteState::Written | ManagedWriteState::Synced
        );
        if owned {
            file.write_state = ManagedWriteState::Poisoned;
        }
        sync(&file.handle)?;
        self.checked_file_chain(directory, file)?;
        if owned {
            file.write_state = ManagedWriteState::Synced;
        }
        Ok(())
    }

    pub(crate) fn inspect_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &ManagedFileCapability,
    ) -> Result<ManagedFileMetadata> {
        let _guard = self.lock_mutations()?;
        let _chain = self.checked_file_chain(directory, file)?;
        managed_metadata(&file.handle)
    }

    pub(crate) fn enumerate_regular_names(
        &self,
        directory: &ManagedDirectoryCapability,
    ) -> Result<Vec<ManagedRelativeName>> {
        let _guard = self.lock_mutations()?;
        let chain = self.checked_directory_chain(directory)?;
        let mut result = Vec::new();
        enumerate_managed(chain.last().unwrap(), directory.area, "", &mut result)?;
        self.checked_directory_chain(directory)?;
        result.sort_by(|a, b| a.0.cmp(&b.0));
        ensure!(
            result.windows(2).all(|pair| pair[0] != pair[1]),
            "duplicate managed entries"
        );
        Ok(result)
    }

    /// Acquire before SQLite; callback is scalar/SQL-only and commits before
    /// returning. No namespace/recovery reentry, streams, network or UI work.
    pub(crate) fn with_current_regular_files<T>(
        &self,
        files: &[ManagedFileCheck<'_>],
        operation: impl FnOnce(&[ManagedFileMetadata]) -> Result<T>,
    ) -> Result<T> {
        ensure!(!files.is_empty(), "empty managed file validation set");
        let _guard = self.lock_mutations()?;
        let mut chains = Vec::with_capacity(files.len());
        let mut metadata = Vec::with_capacity(files.len());
        for check in files {
            chains.push(self.checked_file_chain(check.directory, check.file)?);
            let info = managed_metadata(&check.file.handle)?;
            ensure!(
                check
                    .expected
                    .is_none_or(|expected| expected == info.identity),
                "persisted file identity mismatch"
            );
            metadata.push(info);
        }
        operation(&metadata)
    }

    pub(crate) fn publish_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        source: &mut ManagedFileCapability,
        destination: ManagedPublication<'_>,
    ) -> Result<()> {
        self.publish_regular_with_current_identity(directory, source, destination, |handle| {
            identify(handle, false)
        })
    }

    fn publish_regular_with_current_identity(
        &self,
        directory: &ManagedDirectoryCapability,
        source: &mut ManagedFileCapability,
        destination: ManagedPublication<'_>,
        inspect_current: impl FnOnce(&OwnedHandle) -> Result<Identity>,
    ) -> Result<()> {
        let _guard = self.lock_mutations()?;
        let _source_chain = self.checked_file_chain(directory, source)?;
        ensure!(
            source.write_state == ManagedWriteState::Synced,
            "publication requires a written and synced owned temporary"
        );
        let mut target_chain = self.checked_directory_chain(directory)?;
        let target_name = match destination {
            ManagedPublication::Absent(name) => name.clone(),
            ManagedPublication::Replace(file) => {
                ensure!(
                    file.binding == self.binding && file.area == directory.area,
                    "replacement belongs to another authority"
                );
                ensure!(
                    identify_with_links(&file.handle, false, false)? == file.identity
                        && file.identity != source.identity,
                    "invalid retained replacement identity"
                );
                ensure!(
                    file_link_count(&file.handle)? <= 1,
                    "hardlinked replacement"
                );
                ManagedRelativeName::try_from(file.relative_name.as_str())?
            }
        };
        let leaf = checked_relative_chain(&mut target_chain, directory.area, &target_name)?;
        let parent = target_chain.last().unwrap();
        let parent_identity = identify(parent, true)?;
        if let ManagedPublication::Replace(file) = destination {
            ensure!(
                parent_identity == file.parent_identity,
                "replacement parent changed"
            );
        }
        let current = optional_checked_regular(parent, &leaf)?;
        match destination {
            ManagedPublication::Absent(_) => {
                if current.is_some() {
                    return Err(ManagedPublicationConflict::DestinationAppeared.into());
                }
                if let Err(error) = rename_handle(&source.handle, parent, &leaf, false) {
                    if is_win_error(&error, &[80, 183]) {
                        self.checked_file_chain(directory, source)?;
                        self.checked_directory_chain(directory)?;
                        if optional_checked_regular(parent, &leaf)?.is_some() {
                            return Err(ManagedPublicationConflict::DestinationAppeared.into());
                        }
                    }
                    return Err(error);
                }
            }
            ManagedPublication::Replace(file) => {
                let current_identity = current.as_ref().map(inspect_current).transpose()?;
                if current_identity != Some(file.identity) {
                    return Err(ManagedPublicationConflict::DestinationChanged.into());
                }
                rename_handle(&source.handle, parent, &leaf, true)?;
            }
        }
        // The retained source changes state at kernel commit; no fallible reopen.
        source.relative_name = target_name.0;
        source.parent_identity = parent_identity;
        source.write_state = ManagedWriteState::Published;
        Ok(())
    }

    fn checked_directory_chain(
        &self,
        directory: &ManagedDirectoryCapability,
    ) -> Result<Vec<OwnedHandle>> {
        self.validate_root()?;
        ensure!(
            directory.binding == self.binding,
            "foreign managed directory authority"
        );
        ensure!(
            identify(&directory.handle, true)? == directory.identity,
            "retained directory changed"
        );
        let mut chain = vec![self.root.try_clone()?];
        for (name, expected) in [
            ("accounts", directory.accounts_identity),
            (self.user.as_str(), directory.namespace_identity),
        ] {
            let child = checked_directory_at(chain.last().unwrap(), OsStr::new(name))?;
            ensure!(
                identify(&child, true)? == expected,
                "namespace ancestor detached"
            );
            chain.push(child);
        }
        if self.namespace.mappings.iter().any(|m| m.area == directory.area.storage_name()) {
            chain.push(self.area_directory(chain.last().unwrap(), directory.area, false)?);
        } else {
            for name in directory.area.relative_path().split('/') {
                chain.push(checked_directory_at(chain.last().unwrap(), OsStr::new(name))?);
            }
        }
        ensure!(
            identify(chain.last().unwrap(), true)? == directory.identity,
            "managed area detached"
        );
        Ok(chain)
    }

    fn checked_file_chain(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &ManagedFileCapability,
    ) -> Result<Vec<OwnedHandle>> {
        ensure!(
            file.binding == self.binding && file.area == directory.area,
            "foreign managed file authority"
        );
        ensure!(
            identify(&file.handle, false)? == file.identity,
            "retained identity changed"
        );
        let mut chain = self.checked_directory_chain(directory)?;
        let leaf = checked_relative_chain(
            &mut chain,
            directory.area,
            &ManagedRelativeName::try_from(file.relative_name.as_str())?,
        )?;
        let parent = chain.last().unwrap();
        ensure!(
            identify(parent, true)? == file.parent_identity,
            "managed parent changed"
        );
        let current = optional_checked_regular(parent, &leaf)?
            .ok_or_else(|| anyhow!("managed file disappeared"))?;
        ensure!(
            identify(&current, false)? == file.identity,
            "managed current identity changed"
        );
        Ok(chain)
    }

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
        namespace.validate_active_material_owners()?;
        Ok(Self {
            root: root.handle.try_clone()?,
            root_identity: root.identity,
            lock: root.lock.try_clone()?,
            lock_identity: root.lock_identity,
            user: namespace.user_public_id().to_owned(),
            binding: BINDING_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            namespace: namespace.clone(),
            _mapped_chains: namespace.mappings.iter().filter(|m| namespace.path(ManagedUserArea::from_storage_name(&m.area).unwrap()) == m.target).map(|m| open_absolute_directory_chain(&m.target)).collect::<Result<_>>()?,
        })
    }

    fn area_directory(&self, namespace: &OwnedHandle, area: ManagedUserArea, create: bool) -> Result<OwnedHandle> {
        if let Some(mapping) = self.namespace.mappings.iter().rev().find(|m| m.area == area.storage_name()) {
            let chain = open_absolute_directory_chain(&mapping.target)?;
            let handle = chain.last().unwrap();
            let actual = identify(handle, true)?;
            ensure!(mapping.identity == StableFileIdentity::Windows { volume: actual.volume, file_id: actual.file }, "mapped directory unavailable or replaced");
            Ok(handle.try_clone()?)
        } else { walk_directories(namespace, area.relative_path(), create) }
    }
    pub(crate) fn directory_identity_at(path: &Path) -> Result<StableFileIdentity> {
        let chain = open_absolute_directory_chain(path)?;
        let id = identify(chain.last().unwrap(), true)?;
        Ok(StableFileIdentity::Windows { volume: id.volume, file_id: id.file })
    }

    pub(crate) fn read_material_owner_file(path: &Path) -> Result<(StableFileIdentity, StableFileIdentity, Vec<u8>)> {
        use std::io::Read;
        let chain = open_absolute_directory_chain(path)?;
        let root = chain.last().unwrap();
        let root_id = identify(root, true)?;
        let handle = nt_open(root, OsStr::new(super::MATERIAL_OWNER_FILE),
            FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_SHARE_READ, NT_OPEN, false)?;
        ensure!(file_link_count(&handle)? == 1, "linked material ownership file");
        let file_id = identify(&handle, false)?;
        let mut file = std::fs::File::from(handle);
        ensure!(file.metadata()?.len() <= 2048, "material ownership file is too large");
        let mut bytes = Vec::new();
        (&mut file).take(2049).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 2048, "material ownership file is too large");
        ensure!(identify(open_absolute_directory_chain(path)?.last().unwrap(), true)? == root_id, "material root changed");
        Ok((StableFileIdentity::Windows { volume: root_id.volume, file_id: root_id.file },
            StableFileIdentity::Windows { volume: file_id.volume, file_id: file_id.file }, bytes))
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
            let handle = self.area_directory(&namespace, area, true)?;
            let identity = identify(&handle, true)?;
            managed.push(RetainedDirectory {
                area,
                handle,
                identity,
            });
        }
        let current = self.reopen_namespace(accounts_identity, namespace_identity)?;
        for retained in &managed {
            let attached = self.area_directory(&current, retained.area, false)?;
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
        let mut chain = self.checked_directory_chain(directory)?;
        let leaf = checked_relative_chain(&mut chain, directory.area, name)?;
        let parent = chain.last().unwrap().try_clone()?;
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
        ManagedFileKey::new(directory.area, name.as_str())?;
        let parent_identity = identify(&parent, true)?;
        let handle = open_regular(&parent, leaf, if create { NT_CREATE } else { NT_OPEN })?;
        let identity = identify(&handle, false)?;
        if let Err(error) = prove_entry_spelling(&parent, leaf, identity) {
            if create {
                // Delete only this exclusively created handle, never its name.
                if let Err(cleanup) = unlink_handle(&handle) {
                    return Err(error.context(format!("owned temporary cleanup failed: {cleanup}")));
                }
            }
            return Err(error);
        }
        Ok(ManagedFileCapability {
            handle,
            identity,
            binding: self.binding,
            area: directory.area,
            relative_name: name.0.clone(),
            parent_identity,
            write_state: if create {
                ManagedWriteState::New
            } else {
                ManagedWriteState::Existing
            },
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
        source.write_state = ManagedWriteState::Published;
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
        source.write_state = ManagedWriteState::Published;
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
        let current = self.area_directory(&namespace, directory.area, false)?;
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

fn is_win_error(error: &anyhow::Error, codes: &[i32]) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::raw_os_error)
        .is_some_and(|code| codes.contains(&code))
}

fn file_link_count(handle: &OwnedHandle) -> Result<u64> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    check_bool(unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) })?;
    Ok(u64::from(info.nNumberOfLinks))
}

fn managed_metadata(handle: &OwnedHandle) -> Result<ManagedFileMetadata> {
    let identity = identify(handle, false)?;
    let metadata = std::fs::File::from(handle.try_clone()?).metadata()?;
    Ok(ManagedFileMetadata {
        identity: StableFileIdentity::Windows {
            volume: identity.volume,
            file_id: identity.file,
        },
        byte_size: metadata.len(),
        modified_at: metadata.modified()?,
        link_count: file_link_count(handle)?,
    })
}

struct ManagedEntry {
    name: OsString,
    file_id: [u8; 16],
    attributes: u32,
    reparse_tag: u32,
}

// This API supplies the actual long entry name and all 128 file-ID bits. A
// provider without extended directory IDs fails closed; 8.3 names are not used.
fn managed_entries(parent: &OwnedHandle) -> Result<Vec<ManagedEntry>> {
    let mut result = Vec::new();
    let mut buffer = vec![0u64; 8192];
    let capacity = buffer.len() * size_of::<u64>();
    let mut class = FileIdExtdDirectoryRestartInfo;
    loop {
        buffer.fill(0);
        let success = unsafe {
            GetFileInformationByHandleEx(
                parent.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast(),
                capacity as u32,
            )
        };
        if success == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(18) {
                break;
            } // ERROR_NO_MORE_FILES
            return Err(error).context("enumerate retained managed directory with full file IDs");
        }
        class = FileIdExtdDirectoryInfo;
        let mut offset = 0usize;
        loop {
            let name_offset = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
            ensure!(
                offset + size_of::<FILE_ID_EXTD_DIR_INFO>() <= capacity,
                "malformed directory enumeration header"
            );
            let entry = unsafe {
                buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_EXTD_DIR_INFO>()
                    .read_unaligned()
            };
            let name_bytes = entry.FileNameLength as usize;
            ensure!(
                name_bytes > 0
                    && name_bytes % 2 == 0
                    && offset + name_offset + name_bytes <= capacity,
                "malformed directory entry name"
            );
            let mut wide = Vec::with_capacity(name_bytes / 2);
            for index in 0..name_bytes / 2 {
                wide.push(unsafe {
                    buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset + name_offset + index * 2)
                        .cast::<u16>()
                        .read_unaligned()
                });
            }
            let name = String::from_utf16(&wide).context("undecodable managed directory entry")?;
            if name != "." && name != ".." {
                result.push(ManagedEntry {
                    name: OsString::from(name),
                    file_id: entry.FileId.Identifier,
                    attributes: entry.FileAttributes,
                    reparse_tag: entry.ReparsePointTag,
                });
            }
            if entry.NextEntryOffset == 0 {
                break;
            }
            let next = entry.NextEntryOffset as usize;
            ensure!(
                next >= name_offset + name_bytes && offset + next < capacity,
                "malformed directory entry offset"
            );
            offset += next;
        }
    }
    Ok(result)
}

fn prove_entry_spelling(parent: &OwnedHandle, name: &OsStr, identity: Identity) -> Result<()> {
    let entries = managed_entries(parent)?;
    let matching = entries
        .iter()
        .filter(|entry| entry.name == name)
        .collect::<Vec<_>>();
    ensure!(
        matching.len() == 1,
        "exact stored long-name spelling is unproven"
    );
    ensure!(
        matching[0].file_id == identity.file
            && identity.file != [0; 16]
            && identify(parent, true)?.volume == identity.volume,
        "stored entry identity changed or unsupported"
    );
    ensure!(
        matching[0].attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 && matching[0].reparse_tag == 0,
        "managed entry is a reparse point"
    );
    Ok(())
}

fn checked_directory_at(parent: &OwnedHandle, name: &OsStr) -> Result<OwnedHandle> {
    let child = open_directory(parent, name, false)?;
    prove_entry_spelling(parent, name, identify(&child, true)?)?;
    Ok(child)
}

fn checked_relative_chain(
    chain: &mut Vec<OwnedHandle>,
    area: ManagedUserArea,
    name: &ManagedRelativeName,
) -> Result<OsString> {
    ManagedFileKey::new(area, name.as_str())?;
    let mut parts = name.as_str().split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return Ok(OsString::from(part));
        }
        chain.push(checked_directory_at(
            chain.last().unwrap(),
            OsStr::new(part),
        )?);
    }
    anyhow::bail!("missing managed leaf")
}

fn optional_checked_regular(parent: &OwnedHandle, leaf: &OsStr) -> Result<Option<OwnedHandle>> {
    let handle = match open_regular(parent, leaf, NT_OPEN) {
        Ok(handle) => handle,
        Err(error) if is_win_error(&error, &[2]) => return Ok(None), // only FILE_NOT_FOUND, never PATH_NOT_FOUND
        Err(error) => return Err(error),
    };
    prove_entry_spelling(parent, leaf, identify(&handle, false)?)?;
    Ok(Some(handle))
}

fn enumerate_managed(
    parent: &OwnedHandle,
    area: ManagedUserArea,
    prefix: &str,
    output: &mut Vec<ManagedRelativeName>,
) -> Result<()> {
    let parent_identity = identify(parent, true)?;
    for entry in managed_entries(parent)? {
        if entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || entry.reparse_tag != 0 {
            continue;
        }
        if entry.attributes & FILE_ATTRIBUTE_DEVICE != 0 {
            continue;
        }
        let leaf = entry
            .name
            .to_str()
            .ok_or_else(|| anyhow!("undecodable managed entry"))?;
        let relative = if prefix.is_empty() {
            leaf.to_owned()
        } else {
            format!("{prefix}/{leaf}")
        };
        let canonical = ManagedRelativeName::try_from(relative.as_str())?;
        let is_directory = entry.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let identity = Identity {
            volume: parent_identity.volume,
            file: entry.file_id,
        };
        if ManagedFileKey::new(area, &relative).is_err() {
            ensure!(is_directory, "reserved subarea is not a directory");
            checked_directory_at(parent, &entry.name)?;
            continue;
        }
        if is_directory {
            let child = checked_directory_at(parent, &entry.name)?;
            ensure!(
                identify(&child, true)? == identity,
                "enumerated directory changed"
            );
            enumerate_managed(&child, area, &relative, output)?;
            let current = checked_directory_at(parent, &entry.name)?;
            ensure!(
                identify(&current, true)? == identity,
                "enumerated directory detached"
            );
        } else {
            let file = optional_checked_regular(parent, &entry.name)?
                .ok_or_else(|| anyhow!("enumerated file disappeared"))?;
            ensure!(
                identify(&file, false)? == identity,
                "enumerated file changed"
            );
            output.push(canonical);
        }
    }
    ensure!(
        identify(parent, true)? == parent_identity,
        "enumerated parent changed"
    );
    Ok(())
}

fn unlink_handle(handle: &OwnedHandle) -> Result<()> {
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    check_bool(unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfoEx,
            &disposition as *const _ as *const c_void,
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    })
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
    identify_with_links(handle, directory, true)
}

fn identify_with_links(
    handle: &OwnedHandle,
    directory: bool,
    require_one_link: bool,
) -> Result<Identity> {
    validate_identity_handle(handle, directory, require_one_link)?;
    query_extended_identity(handle)
}

fn validate_identity_handle(
    handle: &OwnedHandle,
    directory: bool,
    require_one_link: bool,
) -> Result<()> {
    let tag = query_attribute_tag(handle)?;
    validate_handle_attributes(handle, directory, require_one_link, tag.FileAttributes, tag.ReparseTag)
}

fn query_attribute_tag(handle: &OwnedHandle) -> Result<FILE_ATTRIBUTE_TAG_INFO> {
    let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
    check_bool(unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            &mut tag as *mut _ as *mut c_void,
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    })?;
    Ok(tag)
}

fn validate_handle_attributes(
    handle: &OwnedHandle,
    directory: bool,
    require_one_link: bool,
    attributes: u32,
    reparse_tag: u32,
) -> Result<()> {
    ensure!(
        attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 && reparse_tag == 0,
        "reparse points are not namespace authority"
    );
    ensure!(
        (attributes & FILE_ATTRIBUTE_DIRECTORY != 0) == directory,
        "wrong managed object type"
    );
    ensure!(
        unsafe { GetFileType(handle.as_raw_handle()) } == FILE_TYPE_DISK,
        "managed object is not disk storage"
    );
    if !directory && require_one_link {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        check_bool(unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) })?;
        ensure!(
            info.nNumberOfLinks == 1,
            "hardlinked or unlinked files are not managed regular files"
        );
    }
    Ok(())
}

fn query_extended_identity(handle: &OwnedHandle) -> Result<Identity> {
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

fn identify_external(handle: &OwnedHandle, directory: bool) -> Result<ExternalIdentity> {
    let mut legacy: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    check_bool(unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut legacy) })?;
    let (attributes, reparse_tag) = match query_attribute_tag(handle) {
        Ok(tag) => (tag.FileAttributes, tag.ReparseTag),
        Err(error) if external_capability_unavailable(&error) => (legacy.dwFileAttributes, 0),
        Err(error) => return Err(error),
    };
    validate_handle_attributes(handle, directory, true, attributes, reparse_tag)?;
    external_identity_with_fallback(query_extended_identity(handle), || {
        Ok(ExternalIdentity::Legacy {
            volume: legacy.dwVolumeSerialNumber,
            file: (u64::from(legacy.nFileIndexHigh) << 32) | u64::from(legacy.nFileIndexLow),
        })
    })
}

fn external_capability_unavailable(error: &anyhow::Error) -> bool {
    is_win_error(error, &[1, 50, 87])
}

fn external_identity_with_fallback(
    extended: Result<Identity>,
    legacy: impl FnOnce() -> Result<ExternalIdentity>,
) -> Result<ExternalIdentity> {
    match extended {
        Ok(identity) => Ok(ExternalIdentity::Extended(identity)),
        Err(error) if external_capability_unavailable(&error) => legacy(),
        Err(error) => Err(error),
    }
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
    nt_open_checked(parent, name, access, share, disposition, directory, false)
}

fn nt_open_external(
    parent: &OwnedHandle,
    name: &OsStr,
    access: u32,
    share: u32,
    disposition: u32,
    directory: bool,
) -> Result<OwnedHandle> {
    nt_open_checked(parent, name, access, share, disposition, directory, true)
}

fn nt_open_checked(
    parent: &OwnedHandle,
    name: &OsStr,
    access: u32,
    share: u32,
    disposition: u32,
    directory: bool,
    external: bool,
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
    if external {
        identify_external(&handle, directory)?;
    } else {
        identify(&handle, directory)?;
    }
    Ok(handle)
}

fn open_directory(parent: &OwnedHandle, name: &OsStr, create: bool) -> Result<OwnedHandle> {
    let handle = nt_open(
        parent,
        name,
        CHECKED_MANAGED_ACCESS,
        SHARE_ALL,
        if create { NT_OPEN_IF } else { NT_OPEN },
        true,
    )?;
    prove_entry_spelling(parent, name, identify(&handle, true)?)?;
    Ok(handle)
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
    let mut handle = open_absolute_anchor(
        &anchor,
        if names.is_empty() {
            CHECKED_MANAGED_ACCESS
        } else {
            TRAVERSE_ACCESS
        },
        SHARE_ALL,
    )?;
    for (index, name) in names.iter().enumerate() {
        handle = nt_open(
            &handle,
            name,
            if index + 1 == names.len() {
                CHECKED_MANAGED_ACCESS
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

fn open_absolute_directory_chain(path: &Path) -> Result<Vec<OwnedHandle>> {
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
    let handle = open_absolute_anchor(
        &anchor,
        if names.is_empty() {
            CHECKED_MANAGED_ACCESS
        } else {
            TRAVERSE_ACCESS
        },
        SHARE_LOCK,
    )?;
    let mut chain = vec![handle];
    for (index, name) in names.iter().enumerate() {
        let handle = nt_open(
            chain.last().unwrap(),
            name,
            if index + 1 == names.len() {
                CHECKED_MANAGED_ACCESS
            } else {
                TRAVERSE_ACCESS
            },
            SHARE_LOCK,
            NT_OPEN,
            true,
        )?;
        chain.push(handle);
    }
    Ok(chain)
}

fn open_absolute_anchor(anchor: &Path, access: u32, share: u32) -> Result<OwnedHandle> {
    open_absolute_anchor_checked(anchor, access, share, false)
}

fn open_absolute_external_anchor(anchor: &Path, access: u32, share: u32) -> Result<OwnedHandle> {
    open_absolute_anchor_checked(anchor, access, share, true)
}

fn open_absolute_anchor_checked(
    anchor: &Path,
    access: u32,
    share: u32,
    external: bool,
) -> Result<OwnedHandle> {
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
            access,
            share,
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
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if external {
        identify_external(&handle, true)?;
    } else {
        identify(&handle, true)?;
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
        // Native extended rename supports a retained RootDirectory; the Win32
        // wrapper rejects that handle with ERROR_INVALID_PARAMETER.
        let mut io_status: IoStatusBlock = std::mem::zeroed();
        let status = NtSetInformationFile(
            source.as_raw_handle(),
            &mut io_status,
            info.cast(),
            bytes as u32,
            FILE_RENAME_INFORMATION_EX,
        );
        if status < 0 {
            Err(std::io::Error::from_raw_os_error(RtlNtStatusToDosError(status) as i32))
        } else {
            Ok(())
        }
    }
    .context("rename retained managed file (native extended semantics required)")
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::{external_identity_with_fallback, identify, open_lock, relative_parent, ExternalIdentity, Identity, MutationLock, LOCK_NAME};
    use std::fs;
    use std::os::windows::fs::{symlink_dir, symlink_file};

    const USER: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn external_identity_uses_legacy_ids_only_when_extended_ids_are_unsupported() {
        let fallback = ExternalIdentity::Legacy { volume: 42, file: 91 };
        let unavailable: Result<Identity> = Err(std::io::Error::from_raw_os_error(87).into());
        assert!(external_identity_with_fallback(unavailable, || Ok(fallback)).unwrap() == fallback);

        let denied: Result<Identity> = Err(std::io::Error::from_raw_os_error(5).into());
        assert!(external_identity_with_fallback(denied, || panic!("access failures must not downgrade identity checks")).is_err());
    }

    #[test]
    fn windows_external_source_reads_configured_removable_reference_without_mutating_it() {
        let Some(source) = std::env::var_os("ELUNVI_TEST_REMOVABLE_REFERENCE").map(PathBuf::from) else { return; };
        let expected = fs::read(&source).unwrap();
        let private = tempfile::tempdir().unwrap();
        let data_root = NamespaceFs::open_data_root(private.path()).unwrap();
        let copied = data_root.read_external_source(&source, expected.len() as u64 + 1).unwrap();
        assert_eq!(copied, expected);
        assert_eq!(fs::read(source).unwrap(), expected);
    }

    #[test]
    fn managed_metadata_preserves_platform_identity_and_rejects_hardlinks() {
        let (_root, ns, fs, out) = fixture();
        let mut file = fs.create_new_regular(&out, &name("metadata")).unwrap();
        fs.write_new_regular_from(&out, &mut file, &mut &b"content"[..])
            .unwrap();
        let metadata = fs.inspect_regular(&out, &file).unwrap();
        let identity = identify(&file.handle, false).unwrap();
        assert_eq!(
            metadata.identity,
            StableFileIdentity::Windows {
                volume: identity.volume,
                file_id: identity.file
            }
        );
        assert_eq!(metadata.byte_size, 7);
        assert_eq!(metadata.link_count, 1);
        assert_eq!(
            metadata.modified_at,
            fs::metadata(ns.output_dir().join("metadata"))
                .unwrap()
                .modified()
                .unwrap()
        );
        fs::hard_link(
            ns.output_dir().join("metadata"),
            ns.output_dir().join("hardlink"),
        )
        .unwrap();
        assert!(fs.inspect_regular(&out, &file).is_err());
        assert!(fs.open_optional_regular(&out, &name("hardlink")).is_err());
        assert!(fs.enumerate_regular_names(&out).is_err());
    }

    #[test]
    fn managed_entry_spelling_rejects_short_name_aliases_when_available() {
        use std::os::windows::ffi::OsStrExt;
        let (_root, ns, fs, out) = fixture();
        let long = "Long managed filename.txt";
        fs::write(ns.output_dir().join(long), b"bytes").unwrap();
        fs.open_existing_regular(&out, &name(long)).unwrap();
        let wide: Vec<u16> = ns
            .output_dir()
            .join(long)
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut output = vec![0u16; 32768];
        let size = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
                wide.as_ptr(),
                output.as_mut_ptr(),
                output.len() as u32,
            )
        } as usize;
        assert!(size > 0 && size < output.len());
        let short_path = String::from_utf16(&output[..size]).unwrap();
        let short = Path::new(&short_path)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        if short != long {
            assert!(fs.open_optional_regular(&out, &name(short)).is_err());
        } else {
            eprintln!("fixture volume does not expose a separate short filename");
        }
        assert_eq!(fs::read(ns.output_dir().join(long)).unwrap(), b"bytes");
    }

    #[test]
    fn managed_injected_sync_failure_poisons_publication_and_retains_cleanup() {
        let (_root, ns, fs, out) = fixture();
        let mut file = fs.create_new_regular(&out, &name("temp")).unwrap();
        fs.write_new_regular_from(&out, &mut file, &mut &b"bytes"[..])
            .unwrap();
        assert!(fs
            .sync_regular_with(&out, &mut file, |_| Err(std::io::Error::other(
                "injected sync failure"
            )
            .into()))
            .is_err());
        let error = fs
            .publish_regular(
                &out,
                &mut file,
                ManagedPublication::Absent(&name("document")),
            )
            .unwrap_err();
        assert!(error.downcast_ref::<ManagedPublicationConflict>().is_none());
        assert!(fs.sync_regular(&out, &mut file).is_err());
        fs.unlink_within(&out, file).unwrap();
        assert!(!ns.output_dir().join("temp").exists());
        assert!(!ns.output_dir().join("document").exists());
    }

    #[test]
    fn managed_replacement_identity_failure_is_not_a_conflict() {
        let (_root, ns, fs, out) = fixture();
        let mut source = fs.create_new_regular(&out, &name("temp")).unwrap();
        fs.write_new_regular_from(&out, &mut source, &mut &b"new"[..])
            .unwrap();
        fs.sync_regular(&out, &mut source).unwrap();
        fs::write(ns.output_dir().join("document"), b"old").unwrap();
        let destination = fs.open_existing_regular(&out, &name("document")).unwrap();
        let error = fs
            .publish_regular_with_current_identity(
                &out,
                &mut source,
                ManagedPublication::Replace(&destination),
                |_| Err(std::io::Error::from_raw_os_error(5).into()),
            )
            .unwrap_err();
        assert!(error.downcast_ref::<ManagedPublicationConflict>().is_none());
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
        assert_eq!(fs::read(ns.output_dir().join("document")).unwrap(), b"old");
        fs::rename(
            ns.output_dir().join("document"),
            ns.output_dir().join("held"),
        )
        .unwrap();
        let missing = fs
            .publish_regular(&out, &mut source, ManagedPublication::Replace(&destination))
            .unwrap_err();
        assert_eq!(
            missing.downcast_ref::<ManagedPublicationConflict>(),
            Some(&ManagedPublicationConflict::DestinationChanged)
        );
        fs.unlink_within(&out, source).unwrap();
        assert_eq!(fs::read(ns.output_dir().join("held")).unwrap(), b"old");
        assert!(!ns.output_dir().join("temp").exists());
    }

    #[test]
    fn windows_external_export_can_select_a_writable_drive_root() {
        use super::{open_absolute_anchor, MANAGED_ACCESS, SHARE_LOCK};
        let private = tempfile::tempdir().unwrap();
        let data_root = NamespaceFs::open_data_root(private.path()).unwrap();
        let working = std::env::current_dir().unwrap();
        let drive = working.ancestors().last().unwrap();
        // This test opens handles only: it never creates files at the drive root.
        let Ok(_writable) = open_absolute_anchor(drive, MANAGED_ACCESS, SHARE_LOCK) else {
            eprintln!("root selection test needs a writable checkout drive");
            return;
        };
        let destination = ExternalExportDestination::open(&data_root, drive)
            .expect("a writable drive root must be selectable for material migration");
        assert_eq!(destination.normalized_display_path(), drive);
    }

    #[test]
    fn windows_external_export_accepts_ordinary_and_verbatim_local_drive_paths() {
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root_path = fs::canonicalize(root.path()).unwrap();
        let external_path = fs::canonicalize(external.path()).unwrap();
        let data_root = NamespaceFs::open_data_root(&root_path).unwrap();
        let ordinary = Path::new(
            external_path
                .to_str()
                .unwrap()
                .strip_prefix(r"\\?\")
                .unwrap(),
        );
        let first = ExternalExportDestination::open(&data_root, ordinary).unwrap();
        first
            .write_new_file("ordinary.bin", &mut &b"ordinary"[..])
            .unwrap();
        let second = ExternalExportDestination::open(&data_root, &external_path).unwrap();
        second
            .write_new_file("verbatim.bin", &mut &b"verbatim"[..])
            .unwrap();
        assert_eq!(
            fs::read(external_path.join("ordinary.bin")).unwrap(),
            b"ordinary"
        );
        assert_eq!(
            fs::read(external_path.join("verbatim.bin")).unwrap(),
            b"verbatim"
        );
    }

    #[test]
    fn windows_external_export_reparse_aliases_fail_without_creation() {
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root_path = fs::canonicalize(root.path()).unwrap();
        let external_path = fs::canonicalize(external.path()).unwrap();
        let data_root = NamespaceFs::open_data_root(&root_path).unwrap();
        symlink_dir(&root_path, external_path.join("alias")).unwrap();
        assert!(ExternalExportDestination::open(
            &data_root,
            &external_path.join("alias").join("absent")
        )
        .is_err());
        assert!(!root_path.join("absent").exists());
        assert!(ExternalExportDestination::open(
            &data_root,
            Path::new(r"\\localhost\C$\absent-export")
        )
        .is_err());
    }

    #[test]
    fn windows_external_export_pins_ancestors_and_preserves_temporary_collision() {
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root_path = fs::canonicalize(root.path()).unwrap();
        let external_path = fs::canonicalize(external.path()).unwrap();
        let data_root = NamespaceFs::open_data_root(&root_path).unwrap();
        fs::create_dir(external_path.join("selected")).unwrap();
        let destination =
            ExternalExportDestination::open(&data_root, &external_path.join("selected")).unwrap();
        assert!(fs::rename(external_path.join("selected"), external_path.join("moved")).is_err());
        fs::write(
            external_path.join("selected").join("collision.tmp"),
            b"keep",
        )
        .unwrap();
        assert!(destination
            .write_new_file_with_temp("result.bin", &mut &b"bad"[..], "collision.tmp")
            .is_err());
        assert_eq!(
            fs::read(external_path.join("selected").join("collision.tmp")).unwrap(),
            b"keep"
        );
        destination
            .write_new_file("result.bin", &mut &b"good"[..])
            .unwrap();
        assert_eq!(
            fs::read(external_path.join("selected").join("result.bin")).unwrap(),
            b"good"
        );
    }

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
