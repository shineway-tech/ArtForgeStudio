//! Owner-bound user namespaces and their capability-only filesystem boundary.
#![allow(dead_code)]

#[cfg(not(windows))]
use anyhow::anyhow;
#[cfg(unix)]
use anyhow::Context;
use anyhow::{ensure, Result};
#[cfg(unix)]
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

#[cfg(windows)]
#[path = "user_namespace_windows.rs"]
mod windows;
#[cfg(windows)]
#[allow(unused_imports)] // Public handoff types need not all be used by each exact-module harness.
pub(crate) use windows::{
    DataRootCapability, ExternalExportDestination, ManagedDirectoryCapability, ManagedFileCapability,
    ManagedNamespaceDirectories, NamespaceFs,
};

// Kept platform independent so Windows name-policy tests execute on every host.
#[cfg(any(windows, test))]
fn validate_windows_relative_name(value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "empty Windows relative name");
    for component in value.split(['/', '\\']) {
        ensure!(
            !component.is_empty() && component != "." && component != "..",
            "invalid Windows path component"
        );
        ensure!(
            !component.ends_with(['.', ' ']),
            "ambiguous Windows path component"
        );
        ensure!(
            !component.chars().any(|c| c <= '\u{1f}' || "<>:\"|?*".contains(c)),
            "invalid Windows filename character"
        );
        let stem = component.split('.').next().unwrap().trim_end_matches(' ').to_uppercase();
        let numbered_device = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| matches!(suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"));
        ensure!(
            !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
                && !numbered_device,
            "reserved Windows device name"
        );
    }
    Ok(())
}

#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ManagedUserArea {
    Input,
    Output,
    Prompt,
    Canvas,
    CanvasUploads,
    CanvasExports,
    References,
    ReferencesLibrary,
    ReferencesImports,
    Previews,
    Recovery,
    DeliveryStaging,
    Videos,
    ToolboxCompressionInputs,
    ToolboxCompressionResults,
    ToolboxConversionInputs,
    ToolboxConversionResults,
    ToolboxCropInputs,
}

impl ManagedUserArea {
    fn relative_path(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "out",
            Self::Prompt => "prompt",
            Self::Canvas => "canvas",
            Self::CanvasUploads => "canvas/uploads",
            Self::CanvasExports => "canvas/exports",
            Self::References => "references",
            Self::ReferencesLibrary => "references/library",
            Self::ReferencesImports => "references/imports",
            Self::Previews => "previews",
            Self::Recovery => "recovery",
            Self::DeliveryStaging => "delivery-staging",
            Self::Videos => "videos",
            Self::ToolboxCompressionInputs => "toolbox/compression-inputs",
            Self::ToolboxCompressionResults => "toolbox/compression-results",
            Self::ToolboxConversionInputs => "toolbox/conversion-inputs",
            Self::ToolboxConversionResults => "toolbox/conversion-results",
            Self::ToolboxCropInputs => "toolbox/crop-inputs",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct UserNamespace {
    user_public_id: String,
    root: PathBuf,
}

impl UserNamespace {
    pub(crate) fn new(data_root: &Path, user_public_id: &str) -> Result<Self> {
        let canonical_user_public_id = Uuid::parse_str(user_public_id)?.to_string();
        ensure!(
            canonical_user_public_id == user_public_id,
            "user namespace id must use canonical UUID spelling"
        );
        Ok(Self {
            root: data_root
                .join("accounts")
                .join(&canonical_user_public_id),
            user_public_id: canonical_user_public_id,
        })
    }

    pub(crate) fn user_public_id(&self) -> &str {
        &self.user_public_id
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn path(&self, area: ManagedUserArea) -> PathBuf {
        self.root.join(area.relative_path())
    }

    pub(crate) fn output_dir(&self) -> PathBuf {
        self.path(ManagedUserArea::Output)
    }

    pub(crate) fn canvas_dir(&self) -> PathBuf {
        self.path(ManagedUserArea::Canvas)
    }

    pub(crate) fn reference_dir(&self) -> PathBuf {
        self.path(ManagedUserArea::References)
    }

    pub(crate) fn preview_dir(&self) -> PathBuf {
        self.path(ManagedUserArea::Previews)
    }

    pub(crate) fn recovery_dir(&self) -> PathBuf {
        self.path(ManagedUserArea::Recovery)
    }

    pub(crate) fn delivery_staging_dir(&self) -> PathBuf {
        self.path(ManagedUserArea::DeliveryStaging)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NamespaceLease {
    pub(crate) namespace: UserNamespace,
    pub(crate) auth_epoch: u64,
    pub(crate) namespace_epoch: u64,
}

pub(crate) struct ManagedRelativeName(String);

impl TryFrom<&str> for ManagedRelativeName {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        #[cfg(windows)]
        validate_windows_relative_name(value)?;
        let path = Path::new(value);
        ensure!(!value.is_empty() && !path.is_absolute());
        ensure!(!value.as_bytes().contains(&0));
        ensure!(
            path.components()
                .all(|component| matches!(component, Component::Normal(_)))
        );
        Ok(Self(value.to_owned()))
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Eq, PartialEq)]
struct ObjectIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
pub(crate) struct ExternalExportDestination {
    descriptor: OwnedFd,
    display_path: PathBuf,
    private_root: OwnedFd,
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct ExternalExportDestination {
    display_path: PathBuf,
}

#[cfg(not(any(unix, windows)))]
impl ExternalExportDestination {
    pub(crate) fn open(_: &DataRootCapability, _: &Path) -> Result<Self> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn normalized_display_path(&self) -> &Path {
        &self.display_path
    }
    pub(crate) fn create_new_directory(&self, _: &str) -> Result<Self> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn write_new_file(&self, _: &str, _: &mut impl std::io::Read) -> Result<u64> {
        unsupported_namespace_capabilities()
    }
}

#[cfg(unix)]
impl ExternalExportDestination {
    pub(crate) fn open(data_root: &DataRootCapability, candidate: &Path) -> Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        ensure!(
            candidate.is_absolute(),
            "export destination must be absolute"
        );
        let raw = candidate.as_os_str().as_bytes();
        ensure!(!raw.contains(&0), "NUL in export path");
        ensure!(
            raw == b"/"
                || raw[1..]
                    .split(|b| *b == b'/')
                    .all(|part| !part.is_empty() && part != b"." && part != b".."),
            "export path must have clean components"
        );
        ensure!(
            directory_identity(&data_root.descriptor)? == data_root.identity,
            "private root identity changed"
        );
        let names: Vec<_> = candidate
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        let mut descriptor = open_absolute_directory(Path::new("/"))?;
        let mut missing = names.len();
        for (index, name) in names.iter().enumerate() {
            match open_external_directory_at(&descriptor, name) {
                Ok(next) => descriptor = next,
                Err(error)
                    if error.downcast_ref::<rustix::io::Errno>()
                        == Some(&rustix::io::Errno::NOENT) =>
                {
                    missing = index;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        // Check the closest existing parent before the first mkdir. Identity,
        // not the display spelling, determines whether it is private.
        reject_private_ancestry(&descriptor, data_root.identity)?;
        for name in &names[missing..] {
            reject_private_ancestry(&descriptor, data_root.identity)?;
            rustix::fs::mkdirat(&descriptor, *name, rustix::fs::Mode::RWXU)
                .context("create a missing external directory without adopting a collision")?;
            descriptor = open_external_directory_at(&descriptor, name)?;
            reject_private_ancestry(&descriptor, data_root.identity)?;
        }
        Ok(Self {
            descriptor,
            display_path: candidate.to_owned(),
            private_root: duplicate_descriptor(&data_root.descriptor)?,
        })
    }

    pub(crate) fn normalized_display_path(&self) -> &Path {
        &self.display_path
    }

    pub(crate) fn create_new_directory(&self, name: &str) -> Result<Self> {
        validate_export_leaf(name)?;
        let _lock = self.lock_directory()?;
        let private_identity = directory_identity(&self.private_root)?;
        reject_private_ancestry(&self.descriptor, private_identity)?;
        rustix::fs::mkdirat(&self.descriptor, name, rustix::fs::Mode::RWXU)?;
        let descriptor = open_external_directory_at(&self.descriptor, OsStr::new(name))?;
        reject_private_ancestry(&descriptor, private_identity)?;
        Ok(Self {
            descriptor,
            display_path: self.display_path.join(name),
            private_root: duplicate_descriptor(&self.private_root)?,
        })
    }

    /// Writes and syncs a private temporary file, then publishes without replace.
    /// The successful rename is the commit point; the result grants no path I/O.
    pub(crate) fn write_new_file(
        &self,
        name: &str,
        stream: &mut impl std::io::Read,
    ) -> Result<u64> {
        self.write_new_file_with_temp(name, stream, &format!(".export-{}.tmp", Uuid::new_v4()))
    }

    fn write_new_file_with_temp(
        &self,
        name: &str,
        stream: &mut impl std::io::Read,
        temporary: &str,
    ) -> Result<u64> {
        validate_export_leaf(name)?;
        validate_export_leaf(temporary)?;
        ensure!(name != temporary, "temporary name equals destination");
        let _lock = self.lock_directory()?;
        reject_private_ancestry(&self.descriptor, directory_identity(&self.private_root)?)?;
        // A failed create owns nothing. In particular, never clean up a name
        // that already existed when O_EXCL failed.
        let descriptor = rustix::fs::openat(
            &self.descriptor,
            temporary,
            rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )?;
        let identity = regular_file_identity(&descriptor)?;
        let mut file = std::fs::File::from(descriptor);
        let result = (|| {
            let bytes = std::io::copy(stream, &mut file)?;
            file.sync_all()?;
            reject_private_ancestry(&self.descriptor, directory_identity(&self.private_root)?)?;
            let current = open_regular_at(&self.descriptor, OsStr::new(temporary))?;
            ensure!(
                regular_file_identity(&current)? == identity,
                "export temporary identity changed"
            );
            rename_without_replacement(
                &self.descriptor,
                OsStr::new(temporary),
                &self.descriptor,
                OsStr::new(name),
            )?;
            Ok(bytes)
        })();
        if result.is_err() {
            // Advisory locking serializes cooperating app writers. This check
            // is not inode-CAS against noncooperating same-UID renames.
            if let Ok(current) = open_regular_at(&self.descriptor, OsStr::new(temporary)) {
                if regular_file_identity(&current).is_ok_and(|current| current == identity) {
                    let _ = rustix::fs::unlinkat(
                        &self.descriptor,
                        temporary,
                        rustix::fs::AtFlags::empty(),
                    );
                }
            }
        }
        result
    }

    fn lock_directory(&self) -> Result<OwnedFd> {
        let lock = open_directory_at(&self.descriptor, OsStr::new("."))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
        Ok(lock)
    }
}

fn validate_export_leaf(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']),
        "export operation requires one clean filename"
    );
    #[cfg(windows)]
    validate_windows_relative_name(name)?;
    Ok(())
}

#[cfg(any(windows, test))]
fn validate_export_volume_root_name(name: &str) -> Result<()> {
    let guid = name
        .strip_prefix(r"\\?\Volume{")
        .and_then(|value| value.strip_suffix("}\\"))
        .ok_or_else(|| anyhow::anyhow!("external anchor is not a proven local volume root"))?;
    Uuid::parse_str(guid)?;
    Ok(())
}

#[cfg(unix)]
fn open_external_directory_at(parent: &OwnedFd, name: &OsStr) -> Result<OwnedFd> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let child = rustix::fs::openat2(
        parent,
        name,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::NO_SYMLINKS | rustix::fs::ResolveFlags::NO_XDEV,
    )?;
    #[cfg(target_vendor = "apple")]
    let child = open_directory_at(parent, name)?;
    #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
    let child: OwnedFd = {
        let _ = (parent, name);
        anyhow::bail!("unsupported external mount semantics")
    };
    ensure!(
        directory_identity(parent)?.device == directory_identity(&child)?.device,
        "external export cannot cross a mount boundary"
    );
    Ok(child)
}

#[cfg(unix)]
fn reject_private_ancestry(directory: &OwnedFd, private: ObjectIdentity) -> Result<()> {
    let mut current = duplicate_descriptor(directory)?;
    // A bounded walk also fails closed on abnormal ancestry/cycles. Traversal
    // follows retained handles, never a resolved display pathname.
    for _ in 0..1024 {
        let identity = directory_identity(&current)?;
        ensure!(
            identity != private,
            "export destination is inside private app storage"
        );
        ensure!(
            rustix::fs::fstat(&current)?.st_nlink != 0,
            "export ancestor was removed"
        );
        let parent = open_directory_at(&current, OsStr::new(".."))?;
        let parent_identity = directory_identity(&parent)?;
        if parent_identity == identity {
            return Ok(());
        }
        ensure!(
            identity.device == parent_identity.device,
            "unproven external mount ancestry"
        );
        current = parent;
    }
    anyhow::bail!("external ancestry exceeds the supported depth")
}

#[cfg(unix)]
pub(crate) struct DataRootCapability {
    descriptor: OwnedFd,
    identity: ObjectIdentity,
    display_root: PathBuf,
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct DataRootCapability {
    _unsupported: (),
}

#[cfg(unix)]
struct RetainedManagedDirectory {
    area: ManagedUserArea,
    descriptor: OwnedFd,
    identity: ObjectIdentity,
}

#[cfg(unix)]
pub(crate) struct ManagedNamespaceDirectories {
    binding_id: u64,
    accounts_descriptor: OwnedFd,
    accounts_identity: ObjectIdentity,
    namespace_descriptor: OwnedFd,
    namespace_identity: ObjectIdentity,
    managed: Vec<RetainedManagedDirectory>,
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct ManagedNamespaceDirectories {
    _unsupported: (),
}

#[cfg(unix)]
pub(crate) struct ManagedDirectoryCapability {
    descriptor: OwnedFd,
    binding_id: u64,
    accounts_identity: ObjectIdentity,
    namespace_identity: ObjectIdentity,
    area: ManagedUserArea,
    identity: ObjectIdentity,
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct ManagedDirectoryCapability {
    _unsupported: (),
}

#[cfg(unix)]
pub(crate) struct ManagedFileCapability {
    descriptor: OwnedFd,
    binding_id: u64,
    area: ManagedUserArea,
    relative_name: String,
    parent_identity: ObjectIdentity,
    identity: ObjectIdentity,
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct ManagedFileCapability {
    _unsupported: (),
}

#[cfg(unix)]
pub(crate) struct NamespaceFs {
    root_descriptor: OwnedFd,
    root_identity: ObjectIdentity,
    user_public_id: String,
    binding_id: u64,
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct NamespaceFs {
    _unsupported: (),
}

#[cfg(unix)]
static NAMESPACE_BINDING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

const MANAGED_USER_AREAS: [ManagedUserArea; 18] = [
    ManagedUserArea::Input,
    ManagedUserArea::Output,
    ManagedUserArea::Prompt,
    ManagedUserArea::Canvas,
    ManagedUserArea::CanvasUploads,
    ManagedUserArea::CanvasExports,
    ManagedUserArea::References,
    ManagedUserArea::ReferencesLibrary,
    ManagedUserArea::ReferencesImports,
    ManagedUserArea::Previews,
    ManagedUserArea::Recovery,
    ManagedUserArea::DeliveryStaging,
    ManagedUserArea::Videos,
    ManagedUserArea::ToolboxCompressionInputs,
    ManagedUserArea::ToolboxCompressionResults,
    ManagedUserArea::ToolboxConversionInputs,
    ManagedUserArea::ToolboxConversionResults,
    ManagedUserArea::ToolboxCropInputs,
];

#[cfg(unix)]
impl NamespaceFs {
    pub(crate) fn open_data_root(data_root: &Path) -> Result<DataRootCapability> {
        let descriptor = open_absolute_directory(data_root)?;
        let identity = directory_identity(&descriptor)?;
        Ok(DataRootCapability {
            descriptor,
            identity,
            display_root: data_root.to_path_buf(),
        })
    }

    pub(crate) fn for_namespace(
        data_root: &DataRootCapability,
        namespace: &UserNamespace,
    ) -> Result<Self> {
        ensure!(
            namespace.root()
                == data_root
                    .display_root
                    .join("accounts")
                    .join(namespace.user_public_id()),
            "namespace was not resolved from this data-root capability"
        );
        ensure!(
            directory_identity(&data_root.descriptor)? == data_root.identity,
            "data-root capability identity changed"
        );
        let current_root = open_absolute_directory(&data_root.display_root)?;
        ensure!(
            directory_identity(&current_root)? == data_root.identity,
            "configured data root no longer names its retained directory"
        );
        Ok(Self {
            root_descriptor: duplicate_descriptor(&data_root.descriptor)?,
            root_identity: data_root.identity,
            user_public_id: namespace.user_public_id().to_string(),
            binding_id: NAMESPACE_BINDING_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub(crate) fn ensure_managed_dirs(&self) -> Result<ManagedNamespaceDirectories> {
        let _mutation_lock = self.lock_mutations()?;
        self.validate_root_descriptor()?;
        let accounts_descriptor =
            ensure_directory_at(&self.root_descriptor, OsStr::new("accounts"))?;
        let accounts_identity = directory_identity(&accounts_descriptor)?;
        let namespace_descriptor = ensure_directory_at(
            &accounts_descriptor,
            OsStr::new(self.user_public_id.as_str()),
        )?;
        let namespace_identity = directory_identity(&namespace_descriptor)?;
        let mut managed = Vec::with_capacity(MANAGED_USER_AREAS.len());
        for area in MANAGED_USER_AREAS {
            let descriptor = walk_fixed_directories(
                &namespace_descriptor,
                area.relative_path(),
                DirectoryWalk::CreateMissing,
            )?;
            let identity = directory_identity(&descriptor)?;
            managed.push(RetainedManagedDirectory {
                area,
                descriptor,
                identity,
            });
        }

        let current_namespace =
            self.reopen_namespace(accounts_identity, namespace_identity)?;
        for retained in &managed {
            let current = walk_fixed_directories(
                &current_namespace,
                retained.area.relative_path(),
                DirectoryWalk::ExistingOnly,
            )?;
            ensure!(
                directory_identity(&current)? == retained.identity,
                "managed directory changed while its capabilities were opened"
            );
        }

        Ok(ManagedNamespaceDirectories {
            binding_id: self.binding_id,
            accounts_descriptor,
            accounts_identity,
            namespace_descriptor,
            namespace_identity,
            managed,
        })
    }

    pub(crate) fn open_managed_dir(
        &self,
        directories: &ManagedNamespaceDirectories,
        area: ManagedUserArea,
    ) -> Result<ManagedDirectoryCapability> {
        let _mutation_lock = self.lock_mutations()?;
        ensure!(
            directories.binding_id == self.binding_id,
            "directory set belongs to another namespace authority"
        );
        ensure!(
            directory_identity(&directories.accounts_descriptor)?
                == directories.accounts_identity,
            "retained accounts directory identity changed"
        );
        ensure!(
            directory_identity(&directories.namespace_descriptor)?
                == directories.namespace_identity,
            "retained namespace directory identity changed"
        );
        let retained = directories
            .managed
            .iter()
            .find(|directory| directory.area == area)
            .ok_or_else(|| anyhow!("managed directory capability is missing"))?;
        ensure!(
            directory_identity(&retained.descriptor)? == retained.identity,
            "retained managed directory identity changed"
        );
        let current_namespace = self.reopen_namespace(
            directories.accounts_identity,
            directories.namespace_identity,
        )?;
        let current = walk_fixed_directories(
            &current_namespace,
            area.relative_path(),
            DirectoryWalk::ExistingOnly,
        )?;
        ensure!(
            directory_identity(&current)? == retained.identity,
            "managed directory is no longer attached to this namespace"
        );
        Ok(ManagedDirectoryCapability {
            descriptor: duplicate_descriptor(&retained.descriptor)?,
            binding_id: self.binding_id,
            accounts_identity: directories.accounts_identity,
            namespace_identity: directories.namespace_identity,
            area,
            identity: retained.identity,
        })
    }

    pub(crate) fn open_existing_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        let _mutation_lock = self.lock_mutations()?;
        self.validate_managed_directory(directory)?;
        let (parent, leaf) = open_relative_parent(&directory.descriptor, name)?;
        let parent_identity = directory_identity(&parent)?;
        let descriptor = open_regular_at(&parent, &leaf)?;
        let identity = regular_file_identity(&descriptor)?;
        Ok(ManagedFileCapability {
            descriptor,
            binding_id: self.binding_id,
            area: directory.area,
            relative_name: name.0.clone(),
            parent_identity,
            identity,
        })
    }

    pub(crate) fn create_new_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        self.create_new_regular_after_validation(directory, name, || {})
    }

    fn create_new_regular_after_validation(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
        after_validation: impl FnOnce(),
    ) -> Result<ManagedFileCapability> {
        let _mutation_lock = self.lock_mutations()?;
        self.validate_managed_directory(directory)?;
        let (parent, leaf) = open_relative_parent(&directory.descriptor, name)?;
        let parent_identity = directory_identity(&parent)?;
        after_validation();
        let descriptor = rustix::fs::openat(
            &parent,
            &leaf,
            rustix::fs::OFlags::RDWR
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .context("create a new regular file relative to a retained directory")?;
        let identity = regular_file_identity(&descriptor)?;
        rustix::fs::fchmod(
            &descriptor,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .context("restrict managed file permissions")?;
        Ok(ManagedFileCapability {
            descriptor,
            binding_id: self.binding_id,
            area: directory.area,
            relative_name: name.0.clone(),
            parent_identity,
            identity,
        })
    }

    pub(crate) fn rename_within(
        &self,
        directory: &ManagedDirectoryCapability,
        source: ManagedFileCapability,
        destination: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        self.rename_within_after_commit(directory, source, destination, || {})
    }

    fn rename_within_after_commit(
        &self,
        directory: &ManagedDirectoryCapability,
        source: ManagedFileCapability,
        destination: &ManagedRelativeName,
        after_commit: impl FnOnce(),
    ) -> Result<ManagedFileCapability> {
        let _mutation_lock = self.lock_mutations()?;
        self.validate_managed_directory(directory)?;
        let (source_parent, source_leaf) =
            self.validate_managed_file(directory, &source)?;
        let (destination_parent, destination_leaf) =
            open_relative_parent(&directory.descriptor, destination)?;
        let destination_parent_identity = directory_identity(&destination_parent)?;
        rename_without_replacement(
            &source_parent,
            &source_leaf,
            &destination_parent,
            &destination_leaf,
        )
        .context("rename a managed file without replacing an existing destination")?;
        // The rename syscall is the commit point. Retain the already-open file;
        // a subsequent name lookup could fail after a successful operation.
        after_commit();
        Ok(ManagedFileCapability {
            descriptor: source.descriptor,
            binding_id: self.binding_id,
            area: directory.area,
            relative_name: destination.0.clone(),
            parent_identity: destination_parent_identity,
            identity: source.identity,
        })
    }

    pub(crate) fn replace_within(
        &self,
        directory: &ManagedDirectoryCapability,
        source: ManagedFileCapability,
        destination: ManagedFileCapability,
    ) -> Result<ManagedFileCapability> {
        let _mutation_lock = self.lock_mutations()?;
        self.validate_managed_directory(directory)?;
        let (source_parent, source_leaf) = self.validate_managed_file(directory, &source)?;
        let (destination_parent, destination_leaf) =
            self.validate_managed_file(directory, &destination)?;
        ensure!(source.identity != destination.identity, "cannot replace a file with itself");
        rustix::fs::renameat(&source_parent, &source_leaf, &destination_parent, &destination_leaf)
            .context("replace the expected managed file under the mutation lock")?;
        Ok(ManagedFileCapability {
            descriptor: source.descriptor,
            binding_id: self.binding_id,
            area: directory.area,
            relative_name: destination.relative_name,
            parent_identity: destination.parent_identity,
            identity: source.identity,
        })
    }

    pub(crate) fn unlink_within(
        &self,
        directory: &ManagedDirectoryCapability,
        file: ManagedFileCapability,
    ) -> Result<()> {
        let _mutation_lock = self.lock_mutations()?;
        self.validate_managed_directory(directory)?;
        let (parent, leaf) = self.validate_managed_file(directory, &file)?;
        rustix::fs::unlinkat(&parent, &leaf, rustix::fs::AtFlags::empty())
            .context("unlink a managed file through its retained parent capability")
    }

    fn lock_mutations(&self) -> Result<OwnedFd> {
        // A fresh open file description is necessary: dup() would share a flock
        // owner and would not serialize two threads using the same root handle.
        // Every NamespaceFs instance follows this protocol. Like other advisory
        // locks, it does not exclude unrelated same-UID processes ignoring it.
        let descriptor = open_directory_at(&self.root_descriptor, OsStr::new("."))?;
        ensure!(directory_identity(&descriptor)? == self.root_identity);
        rustix::fs::flock(&descriptor, rustix::fs::FlockOperation::LockExclusive)
            .context("lock managed filesystem operations")?;
        Ok(descriptor)
    }

    fn validate_root_descriptor(&self) -> Result<()> {
        ensure!(
            directory_identity(&self.root_descriptor)? == self.root_identity,
            "data-root capability identity changed"
        );
        Ok(())
    }

    fn reopen_namespace(
        &self,
        accounts_identity: ObjectIdentity,
        namespace_identity: ObjectIdentity,
    ) -> Result<OwnedFd> {
        self.validate_root_descriptor()?;
        let accounts = open_directory_at(&self.root_descriptor, OsStr::new("accounts"))?;
        ensure!(
            directory_identity(&accounts)? == accounts_identity,
            "accounts ancestor is no longer attached to this data root"
        );
        let namespace =
            open_directory_at(&accounts, OsStr::new(self.user_public_id.as_str()))?;
        ensure!(
            directory_identity(&namespace)? == namespace_identity,
            "user namespace is no longer attached to its accounts ancestor"
        );
        Ok(namespace)
    }

    fn validate_managed_directory(
        &self,
        directory: &ManagedDirectoryCapability,
    ) -> Result<()> {
        ensure!(
            directory.binding_id == self.binding_id,
            "managed directory belongs to another namespace authority"
        );
        ensure!(
            directory_identity(&directory.descriptor)? == directory.identity,
            "retained managed directory identity changed"
        );
        let namespace = self.reopen_namespace(
            directory.accounts_identity,
            directory.namespace_identity,
        )?;
        let current = walk_fixed_directories(
            &namespace,
            directory.area.relative_path(),
            DirectoryWalk::ExistingOnly,
        )?;
        ensure!(
            directory_identity(&current)? == directory.identity,
            "managed directory is no longer attached to this namespace"
        );
        Ok(())
    }

    fn validate_managed_file(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &ManagedFileCapability,
    ) -> Result<(OwnedFd, OsString)> {
        ensure!(
            file.binding_id == self.binding_id && file.area == directory.area,
            "managed file belongs to another directory capability"
        );
        ensure!(
            regular_file_identity(&file.descriptor)? == file.identity,
            "retained managed file identity changed"
        );
        let relative_name = ManagedRelativeName::try_from(file.relative_name.as_str())?;
        let (parent, leaf) = open_relative_parent(&directory.descriptor, &relative_name)?;
        ensure!(
            directory_identity(&parent)? == file.parent_identity,
            "managed file parent identity changed"
        );
        let current = open_regular_at(&parent, &leaf)?;
        ensure!(
            regular_file_identity(&current)? == file.identity,
            "managed file path no longer names its retained identity"
        );
        Ok((parent, leaf))
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum DirectoryWalk {
    ExistingOnly,
    CreateMissing,
}

#[cfg(unix)]
fn duplicate_descriptor(descriptor: &OwnedFd) -> Result<OwnedFd> {
    rustix::io::fcntl_dupfd_cloexec(descriptor, 0)
        .context("duplicate a retained filesystem capability")
}

#[cfg(unix)]
fn rename_without_replacement(
    source_parent: &OwnedFd,
    source_leaf: &OsStr,
    destination_parent: &OwnedFd,
    destination_leaf: &OsStr,
) -> rustix::io::Result<()> {
    #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "redox"))]
    {
        rustix::fs::renameat_with(
            source_parent,
            source_leaf,
            destination_parent,
            destination_leaf,
            rustix::fs::RenameFlags::NOREPLACE,
        )
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "redox")))]
    {
        let _ = (source_parent, source_leaf, destination_parent, destination_leaf);
        Err(rustix::io::Errno::NOTSUP)
    }
}

#[cfg(unix)]
fn object_identity(descriptor: &OwnedFd) -> Result<(ObjectIdentity, rustix::fs::FileType)> {
    let stat = rustix::fs::fstat(descriptor).context("inspect a retained filesystem object")?;
    Ok((
        ObjectIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
        },
        rustix::fs::FileType::from_raw_mode(stat.st_mode),
    ))
}

#[cfg(unix)]
fn directory_identity(descriptor: &OwnedFd) -> Result<ObjectIdentity> {
    let (identity, file_type) = object_identity(descriptor)?;
    ensure!(file_type.is_dir(), "filesystem capability is not a directory");
    Ok(identity)
}

#[cfg(unix)]
fn regular_file_identity(descriptor: &OwnedFd) -> Result<ObjectIdentity> {
    let (identity, file_type) = object_identity(descriptor)?;
    ensure!(file_type.is_file(), "filesystem capability is not a regular file");
    Ok(identity)
}

#[cfg(unix)]
fn open_directory_at(parent: &OwnedFd, component: &OsStr) -> Result<OwnedFd> {
    let descriptor = rustix::fs::openat(
        parent,
        component,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .context("open a directory component relative to its retained parent")?;
    directory_identity(&descriptor)?;
    Ok(descriptor)
}

#[cfg(unix)]
fn open_absolute_directory(path: &Path) -> Result<OwnedFd> {
    ensure!(path.is_absolute(), "application data root must be absolute");
    let mut descriptor = rustix::fs::open(
        "/",
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .context("open the absolute filesystem anchor")?;
    directory_identity(&descriptor)?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => descriptor = open_directory_at(&descriptor, name)?,
            _ => return Err(anyhow!("application data root contains a non-normal component")),
        }
    }
    Ok(descriptor)
}

#[cfg(unix)]
fn ensure_directory_at(parent: &OwnedFd, component: &OsStr) -> Result<OwnedFd> {
    let descriptor = match open_directory_at(parent, component) {
        Ok(descriptor) => descriptor,
        Err(open_error) => {
            match rustix::fs::mkdirat(parent, component, rustix::fs::Mode::RWXU) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                Err(create_error) => {
                    return Err(create_error)
                        .context("create a directory component through its retained parent");
                }
            }
            open_directory_at(parent, component).map_err(|retry_error| {
                retry_error.context(format!(
                    "open managed directory after create attempt ({open_error})"
                ))
            })?
        }
    };
    rustix::fs::fchmod(&descriptor, rustix::fs::Mode::RWXU)
        .context("restrict managed directory permissions")?;
    Ok(descriptor)
}

#[cfg(unix)]
fn walk_fixed_directories(
    base: &OwnedFd,
    relative_path: &str,
    walk: DirectoryWalk,
) -> Result<OwnedFd> {
    let mut current = duplicate_descriptor(base)?;
    for component in relative_path.split('/') {
        ensure!(!component.is_empty(), "fixed managed path is invalid");
        current = match walk {
            DirectoryWalk::ExistingOnly =>
                open_directory_at(&current, OsStr::new(component))?,
            DirectoryWalk::CreateMissing =>
                ensure_directory_at(&current, OsStr::new(component))?,
        };
    }
    Ok(current)
}

#[cfg(unix)]
fn normal_name_components(name: &ManagedRelativeName) -> Result<Vec<OsString>> {
    Path::new(&name.0)
        .components()
        .map(|component| match component {
            Component::Normal(component) => Ok(component.to_os_string()),
            _ => Err(anyhow!("managed relative name invariant was violated")),
        })
        .collect()
}

#[cfg(unix)]
fn open_relative_parent(
    base: &OwnedFd,
    name: &ManagedRelativeName,
) -> Result<(OwnedFd, OsString)> {
    let components = normal_name_components(name)?;
    let (leaf, parents) = components
        .split_last()
        .ok_or_else(|| anyhow!("managed relative name has no leaf"))?;
    let mut current = duplicate_descriptor(base)?;
    for component in parents {
        current = open_directory_at(&current, component)?;
    }
    Ok((current, leaf.clone()))
}

#[cfg(unix)]
fn open_regular_at(parent: &OwnedFd, leaf: &OsStr) -> Result<OwnedFd> {
    let descriptor = rustix::fs::openat(
        parent,
        leaf,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .context("open a regular file relative to its retained parent")?;
    regular_file_identity(&descriptor)?;
    Ok(descriptor)
}

#[cfg(not(any(unix, windows)))]
impl NamespaceFs {
    pub(crate) fn open_data_root(_data_root: &Path) -> Result<DataRootCapability> {
        unsupported_namespace_capabilities()
    }

    pub(crate) fn for_namespace(
        _data_root: &DataRootCapability,
        _namespace: &UserNamespace,
    ) -> Result<Self> {
        unsupported_namespace_capabilities()
    }

    pub(crate) fn ensure_managed_dirs(&self) -> Result<ManagedNamespaceDirectories> {
        unsupported_namespace_capabilities()
    }

    pub(crate) fn open_managed_dir(
        &self,
        _directories: &ManagedNamespaceDirectories,
        _area: ManagedUserArea,
    ) -> Result<ManagedDirectoryCapability> {
        unsupported_namespace_capabilities()
    }

    pub(crate) fn open_existing_regular(
        &self,
        _directory: &ManagedDirectoryCapability,
        _name: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        unsupported_namespace_capabilities()
    }

    pub(crate) fn create_new_regular(
        &self,
        _directory: &ManagedDirectoryCapability,
        _name: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        unsupported_namespace_capabilities()
    }

    pub(crate) fn rename_within(
        &self,
        _directory: &ManagedDirectoryCapability,
        _source: ManagedFileCapability,
        _destination: &ManagedRelativeName,
    ) -> Result<ManagedFileCapability> {
        unsupported_namespace_capabilities()
    }

    pub(crate) fn unlink_within(
        &self,
        _directory: &ManagedDirectoryCapability,
        _file: ManagedFileCapability,
    ) -> Result<()> {
        unsupported_namespace_capabilities()
    }

    pub(crate) fn replace_within(
        &self,
        _directory: &ManagedDirectoryCapability,
        _source: ManagedFileCapability,
        _destination: ManagedFileCapability,
    ) -> Result<ManagedFileCapability> {
        unsupported_namespace_capabilities()
    }
}

#[cfg(not(any(unix, windows)))]
fn unsupported_namespace_capabilities<T>() -> Result<T> {
    Err(anyhow!(
        "managed namespace capabilities are unavailable without audited handle-relative support"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[cfg(unix)]
    use std::os::unix::net::UnixListener;

    fn temporary_directory() -> tempfile::TempDir {
        let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
        tempfile::tempdir_in(parent).unwrap()
    }

    const USER_A: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn external_export_volume_anchor_proof_rejects_hidden_subtrees_and_shares() {
        assert!(validate_export_volume_root_name(
            r"\\?\Volume{11111111-1111-4111-8111-111111111111}\"
        )
        .is_ok());
        for rejected in [
            r"\\?\Volume{11111111-1111-4111-8111-111111111111}\private\accounts\",
            r"\\?\UNC\server\share\",
            r"C:\",
            r"\\?\Volume{invalid}\",
        ] {
            assert!(validate_export_volume_root_name(rejected).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn external_export_temporary_collision_never_removes_existing_file() {
        let root = temporary_directory();
        let external = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let destination = ExternalExportDestination::open(&data_root, external.path()).unwrap();
        fs::write(external.path().join("collision.tmp"), b"keep temporary").unwrap();
        assert!(destination
            .write_new_file_with_temp("result.bin", &mut &b"new"[..], "collision.tmp")
            .is_err());
        assert_eq!(
            fs::read(external.path().join("collision.tmp")).unwrap(),
            b"keep temporary"
        );
        assert!(!external.path().join("result.bin").exists());
    }

    #[cfg(unix)]
    #[test]
    fn external_export_swapped_temporary_is_neither_published_nor_removed() {
        struct SwapTemporary(PathBuf);
        impl std::io::Read for SwapTemporary {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                fs::remove_file(&self.0)?;
                fs::write(&self.0, b"replacement")?;
                Ok(0)
            }
        }
        let root = temporary_directory();
        let external = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let destination = ExternalExportDestination::open(&data_root, external.path()).unwrap();
        let temporary = external.path().join("owned.tmp");
        assert!(destination
            .write_new_file_with_temp(
                "result.bin",
                &mut SwapTemporary(temporary.clone()),
                "owned.tmp"
            )
            .is_err());
        assert_eq!(fs::read(temporary).unwrap(), b"replacement");
        assert!(!external.path().join("result.bin").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn external_export_macos_data_volume_alias_keeps_the_private_boundary() {
        // Only alias paths to our own temporary fixtures are inspected. No
        // fixed mount point or user directory is created by this test.
        let root = temporary_directory();
        let external = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let root_alias =
            Path::new("/System/Volumes/Data").join(root.path().strip_prefix("/").unwrap());
        let external_alias =
            Path::new("/System/Volumes/Data").join(external.path().strip_prefix("/").unwrap());
        if !root_alias.is_dir() || !external_alias.is_dir() {
            return;
        }
        assert!(ExternalExportDestination::open(&data_root, &root_alias.join("absent")).is_err());
        assert!(!root.path().join("absent").exists());
        match ExternalExportDestination::open(&data_root, &external_alias) {
            Ok(destination) => {
                destination
                    .write_new_file("alias.bin", &mut &b"fixture"[..])
                    .unwrap();
                assert_eq!(
                    fs::read(external.path().join("alias.bin")).unwrap(),
                    b"fixture"
                );
                eprintln!(
                    "macOS fixture data-volume alias admitted through retained identity checks"
                );
            }
            Err(_) => {
                assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
                eprintln!("macOS fixture data-volume alias fails closed on this filesystem");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn external_export_removed_or_relocated_private_parent_fails_closed() {
        let root = temporary_directory();
        let external = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        fs::create_dir(external.path().join("removed")).unwrap();
        let removed =
            ExternalExportDestination::open(&data_root, &external.path().join("removed")).unwrap();
        fs::remove_dir(external.path().join("removed")).unwrap();
        assert!(removed.create_new_directory("child").is_err());
        assert!(removed
            .write_new_file("result.bin", &mut &b"bad"[..])
            .is_err());
        fs::create_dir(external.path().join("moved")).unwrap();
        let moved =
            ExternalExportDestination::open(&data_root, &external.path().join("moved")).unwrap();
        fs::rename(external.path().join("moved"), root.path().join("moved")).unwrap();
        assert!(moved.create_new_directory("child").is_err());
        assert!(moved
            .write_new_file("result.bin", &mut &b"bad"[..])
            .is_err());
        assert_eq!(fs::read_dir(root.path().join("moved")).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn external_export_preserves_existing_permissions_and_rejects_wrong_types() {
        let root = temporary_directory();
        let external = temporary_directory();
        fs::set_permissions(external.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let destination = ExternalExportDestination::open(&data_root, external.path()).unwrap();
        assert_eq!(
            fs::metadata(external.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::write(external.path().join("file"), b"keep").unwrap();
        assert!(ExternalExportDestination::open(
            &data_root,
            &external.path().join("file").join("child")
        )
        .is_err());
        assert!(destination.create_new_directory("file").is_err());
        destination.create_new_directory("directory").unwrap();
        assert!(destination
            .write_new_file("directory", &mut &b"bad"[..])
            .is_err());
        symlink(root.path(), external.path().join("linked")).unwrap();
        assert!(destination.create_new_directory("linked").is_err());
        assert!(destination
            .write_new_file("linked", &mut &b"bad"[..])
            .is_err());
        assert!(fs::symlink_metadata(external.path().join("linked"))
            .unwrap()
            .is_symlink());
        assert_eq!(fs::read(external.path().join("file")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(external.path()).unwrap().count(), 3);
    }

    // Removing the identity boundary must cause these tests to create private
    // directories; removing retained-relative I/O must redirect the swap test.
    #[cfg(any(unix, windows))]
    #[test]
    fn external_export_rejects_private_roots_before_creating_missing_suffixes() {
        let root = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        assert!(ExternalExportDestination::open(&data_root, root.path()).is_err());
        for area in [
            "accounts",
            "legacy_unassigned",
            "session",
            "cache",
            "updater",
        ] {
            let path = root.path().join(area).join("absent").join("export");
            assert!(ExternalExportDestination::open(&data_root, &path).is_err());
            assert!(!root.path().join(area).exists());
        }
        let namespace = root.path().join("accounts").join(USER_A);
        fs::create_dir_all(&namespace).unwrap();
        for area in MANAGED_USER_AREAS {
            let path = namespace.join(area.relative_path()).join("absent");
            assert!(ExternalExportDestination::open(&data_root, &path).is_err());
            assert!(!namespace.join(area.relative_path()).exists());
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn external_export_rejects_unclean_paths_without_creating_entries() {
        let root = temporary_directory();
        let external = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        for path in [
            PathBuf::from("relative"),
            external.path().join("../escape"),
            external.path().join("./escape"),
            external.path().join("missing//escape"),
        ] {
            assert!(
                ExternalExportDestination::open(&data_root, &path).is_err(),
                "{path:?}"
            );
        }
        assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn external_export_creates_suffix_and_publishes_stream_without_overwrite() {
        let root = temporary_directory();
        let external = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let path = external.path().join("missing").join("exports");
        let destination = ExternalExportDestination::open(&data_root, &path).unwrap();
        assert_eq!(destination.normalized_display_path(), path);
        let archive = destination.create_new_directory("archive").unwrap();
        let bytes = b"\0export bytes\xff\n";
        assert_eq!(
            archive
                .write_new_file("result.bin", &mut &bytes[..])
                .unwrap(),
            15
        );
        assert_eq!(
            fs::read(path.join("archive").join("result.bin")).unwrap(),
            bytes
        );
        assert!(archive
            .write_new_file("result.bin", &mut &b"overwrite"[..])
            .is_err());
        assert_eq!(
            fs::read(path.join("archive").join("result.bin")).unwrap(),
            bytes
        );
        assert!(destination.create_new_directory("archive").is_err());
        assert_eq!(fs::read_dir(path.join("archive")).unwrap().count(), 1);
        for invalid in ["", ".", "..", "nested/file", "./file", "file/"] {
            assert!(archive.create_new_directory(invalid).is_err());
            assert!(archive.write_new_file(invalid, &mut &b"bad"[..]).is_err());
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn external_export_failed_stream_cleans_only_its_own_temporary_file() {
        struct Broken(bool);
        impl std::io::Read for Broken {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if !self.0 && !buffer.is_empty() {
                    self.0 = true;
                    buffer[0] = b'X';
                    return Ok(1);
                }
                Err(std::io::Error::other("fixture stream failed"))
            }
        }
        let root = temporary_directory();
        let external = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let destination = ExternalExportDestination::open(&data_root, external.path()).unwrap();
        fs::write(external.path().join("existing.bin"), b"keep").unwrap();
        assert!(destination
            .write_new_file("failed.bin", &mut Broken(false))
            .is_err());
        assert!(!external.path().join("failed.bin").exists());
        assert_eq!(
            fs::read(external.path().join("existing.bin")).unwrap(),
            b"keep"
        );
        assert_eq!(fs::read_dir(external.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn external_export_rejects_symlink_aliases_and_retains_acquired_directory() {
        let root = temporary_directory();
        let external = temporary_directory();
        let replacement = temporary_directory();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        symlink(root.path(), external.path().join("private-alias")).unwrap();
        assert!(ExternalExportDestination::open(
            &data_root,
            &external.path().join("private-alias/absent")
        )
        .is_err());
        assert!(!root.path().join("absent").exists());
        fs::create_dir(external.path().join("selected")).unwrap();
        let destination =
            ExternalExportDestination::open(&data_root, &external.path().join("selected")).unwrap();
        fs::rename(
            external.path().join("selected"),
            external.path().join("retained"),
        )
        .unwrap();
        symlink(replacement.path(), external.path().join("selected")).unwrap();
        destination
            .write_new_file("result.bin", &mut &b"retained"[..])
            .unwrap();
        destination.create_new_directory("archive").unwrap();
        assert_eq!(
            fs::read(external.path().join("retained/result.bin")).unwrap(),
            b"retained"
        );
        assert!(external.path().join("retained/archive").is_dir());
        assert_eq!(fs::read_dir(replacement.path()).unwrap().count(), 0);
        assert!(
            ExternalExportDestination::open(&data_root, &external.path().join("selected/new"))
                .is_err()
        );
    }

    #[test]
    fn windows_names_reject_streams_devices_and_ambiguous_components() {
        for rejected in [
            "", ".", "..", "a/../b", "a/./b", "a//b", "a/", "a\\", "\\a", "C:a",
            "C:\\a", "a:stream", "NUL", "con.txt", "COM1", "LPT9.log", "COM¹.txt",
            "aux ", "a.", "a ", "a\0b", "a<b", "a?b", "a|b", "a\u{1}b",
        ] {
            assert!(
                validate_windows_relative_name(rejected).is_err(),
                "{rejected:?}"
            );
        }
        for accepted in [
            "file.bin", "nested/file.bin", "nested\\file.bin", "LPT10.txt", "控制/结果.png",
        ] {
            assert!(
                validate_windows_relative_name(accepted).is_ok(),
                "{accepted:?}"
            );
        }
    }

    #[test]
    fn namespace_accepts_only_a_canonical_server_uuid() {
        let root = temporary_directory();
        assert!(UserNamespace::new(root.path(), "../../other-user").is_err());
        assert!(UserNamespace::new(root.path(), "11111111111141118111111111111111").is_err());
        assert!(UserNamespace::new(root.path(), "11111111-1111-4111-8111-11111111111A").is_err());

        let namespace = UserNamespace::new(root.path(), USER_A).unwrap();
        assert_eq!(namespace.user_public_id(), USER_A);
        assert_eq!(
            namespace.root(),
            root.path()
                .join("accounts/11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(namespace.output_dir(), namespace.root().join("out"));
        assert_eq!(
            namespace.path(ManagedUserArea::Videos),
            namespace.root().join("videos")
        );
        assert_eq!(
            namespace.path(ManagedUserArea::ToolboxCompressionResults),
            namespace.root().join("toolbox/compression-results")
        );
    }

    #[test]
    fn managed_relative_name_rejects_empty_absolute_and_traversal_components() {
        assert!(ManagedRelativeName::try_from("result.bin").is_ok());
        assert!(ManagedRelativeName::try_from("nested/result.bin").is_ok());
        for rejected in ["", ".", "..", "../result.bin", "nested/../result.bin"] {
            assert!(ManagedRelativeName::try_from(rejected).is_err(), "{rejected}");
        }
        assert!(ManagedRelativeName::try_from("/outside/result.bin").is_err());
    }

    #[cfg(unix)]
    fn create_directory_symlink(target: &Path, link: &Path) {
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(target, link).unwrap();
    }

    #[cfg(unix)]
    fn prepared_namespace(
        root: &Path,
    ) -> (
        UserNamespace,
        DataRootCapability,
        NamespaceFs,
        ManagedNamespaceDirectories,
    ) {
        let namespace = UserNamespace::new(root, USER_A).unwrap();
        let data_root = NamespaceFs::open_data_root(root).unwrap();
        let namespace_fs = NamespaceFs::for_namespace(&data_root, &namespace).unwrap();
        let directories = namespace_fs.ensure_managed_dirs().unwrap();
        (namespace, data_root, namespace_fs, directories)
    }

    #[cfg(unix)]
    #[test]
    fn data_root_capability_rejects_a_symlink() {
        let parent = temporary_directory();
        let real = temporary_directory();
        let linked_root = parent.path().join("linked-root");
        symlink(real.path(), &linked_root).unwrap();

        assert!(NamespaceFs::open_data_root(&linked_root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn data_root_capability_rejects_symlinked_ancestor() {
        let parent = temporary_directory();
        let external = temporary_directory();
        fs::create_dir(external.path().join("data-root")).unwrap();
        let alias = parent.path().join("alias-parent");
        symlink(external.path(), &alias).unwrap();

        assert!(NamespaceFs::open_data_root(&alias.join("data-root")).is_err());
        assert!(!external.path().join("data-root/accounts").exists());
    }

    #[cfg(unix)]
    #[test]
    fn namespace_rejects_replaced_data_root_before_binding() {
        let parent = temporary_directory();
        let root = parent.path().join("data-root");
        let retained = parent.path().join("retained-root");
        fs::create_dir(&root).unwrap();
        let capability = NamespaceFs::open_data_root(&root).unwrap();
        fs::rename(&root, &retained).unwrap();
        fs::create_dir(&root).unwrap();
        let namespace = UserNamespace::new(&root, USER_A).unwrap();

        assert!(NamespaceFs::for_namespace(&capability, &namespace).is_err());
        assert!(!retained.join("accounts").exists());
        assert!(!root.join("accounts").exists());
    }

    #[cfg(unix)]
    #[test]
    fn namespace_rejects_symlinked_account_or_managed_directory() {
        let root = temporary_directory();
        let external = temporary_directory();
        create_directory_symlink(
            external.path(),
            &root.path().join("accounts").join(USER_A).join("out"),
        );
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let namespace = UserNamespace::new(root.path(), USER_A).unwrap();
        let result = NamespaceFs::for_namespace(&data_root, &namespace)
            .and_then(|namespace_fs| namespace_fs.ensure_managed_dirs().map(|_| ()));

        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn namespace_rejects_symlinked_accounts_ancestor() {
        let root = temporary_directory();
        let external = temporary_directory();
        create_directory_symlink(external.path(), &root.path().join("accounts"));
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let namespace = UserNamespace::new(root.path(), USER_A).unwrap();
        let result = NamespaceFs::for_namespace(&data_root, &namespace)
            .and_then(|namespace_fs| namespace_fs.ensure_managed_dirs().map(|_| ()));

        assert!(result.is_err());
        assert!(!external.path().join(USER_A).exists());
    }

    #[cfg(unix)]
    #[test]
    fn namespace_rejects_ancestor_swapped_before_operation() {
        let root = temporary_directory();
        let external = temporary_directory();
        let (_namespace, _data_root, namespace_fs, directories) =
            prepared_namespace(root.path());
        let output = namespace_fs
            .open_managed_dir(&directories, ManagedUserArea::Output)
            .unwrap();
        let accounts = root.path().join("accounts");
        fs::rename(&accounts, root.path().join("accounts-held")).unwrap();
        symlink(external.path(), &accounts).unwrap();

        let name = ManagedRelativeName::try_from("result.bin").unwrap();
        assert!(namespace_fs.create_new_regular(&output, &name).is_err());
        assert!(!external
            .path()
            .join(USER_A)
            .join("out/result.bin")
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn managed_file_opens_and_creates_only_regular_files() {
        let root = tempfile::Builder::new()
            .prefix("n")
            .tempdir_in(fs::canonicalize("/tmp").unwrap())
            .unwrap();
        let (namespace, _data_root, namespace_fs, directories) =
            prepared_namespace(root.path());
        let output = namespace_fs
            .open_managed_dir(&directories, ManagedUserArea::Output)
            .unwrap();

        let regular_name = ManagedRelativeName::try_from("regular.bin").unwrap();
        let _created = namespace_fs
            .create_new_regular(&output, &regular_name)
            .unwrap();
        let _opened = namespace_fs
            .open_existing_regular(&output, &regular_name)
            .unwrap();
        assert!(namespace_fs
            .create_new_regular(&output, &regular_name)
            .is_err());
        assert_eq!(
            fs::metadata(namespace.output_dir().join("regular.bin"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        fs::create_dir(namespace.output_dir().join("directory")).unwrap();
        let directory_name = ManagedRelativeName::try_from("directory").unwrap();
        assert!(namespace_fs
            .open_existing_regular(&output, &directory_name)
            .is_err());

        let external = tempfile::NamedTempFile::new().unwrap();
        symlink(
            external.path(),
            namespace.output_dir().join("linked-file"),
        )
        .unwrap();
        let linked_name = ManagedRelativeName::try_from("linked-file").unwrap();
        assert!(namespace_fs
            .open_existing_regular(&output, &linked_name)
            .is_err());

        let _socket = UnixListener::bind(namespace.output_dir().join("local.sock")).unwrap();
        let socket_name = ManagedRelativeName::try_from("local.sock").unwrap();
        assert!(namespace_fs
            .open_existing_regular(&output, &socket_name)
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn namespace_instances_share_a_mutation_lock() {
        let root = temporary_directory();
        let (_, _, first, _) = prepared_namespace(root.path());
        let (_, _, second, _) = prepared_namespace(root.path());
        let held = first.lock_mutations().unwrap();
        let contender = open_directory_at(&second.root_descriptor, OsStr::new(".")).unwrap();
        assert!(rustix::fs::flock(
            &contender,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        ).is_err());
        drop(held);
        assert!(rustix::fs::flock(
            &contender,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        ).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn create_after_ancestor_move_stays_in_the_retained_directory() {
        let root = temporary_directory();
        let external = temporary_directory();
        let (namespace, _data_root, namespace_fs, directories) =
            prepared_namespace(root.path());
        let output = namespace_fs
            .open_managed_dir(&directories, ManagedUserArea::Output)
            .unwrap();
        let name = ManagedRelativeName::try_from("result.bin").unwrap();
        let retained = root.path().join("retained-accounts");
        let file = namespace_fs.create_new_regular_after_validation(&output, &name, || {
            fs::rename(root.path().join("accounts"), &retained).unwrap();
            symlink(external.path(), root.path().join("accounts")).unwrap();
        }).unwrap();
        assert!(regular_file_identity(&file.descriptor).unwrap() == file.identity);
        assert!(retained.join(USER_A).join("out/result.bin").is_file());
        assert!(!namespace.output_dir().join("result.bin").exists());
        assert!(fs::read_dir(external.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn committed_rename_keeps_its_open_file_if_name_changes_after_commit() {
        use std::io::Read;
        let root = temporary_directory();
        let external = tempfile::NamedTempFile::new().unwrap();
        let (namespace, _data_root, namespace_fs, directories) = prepared_namespace(root.path());
        let output = namespace_fs.open_managed_dir(&directories, ManagedUserArea::Output).unwrap();
        let name = ManagedRelativeName::try_from("source.bin").unwrap();
        let source = namespace_fs.create_new_regular(&output, &name).unwrap();
        fs::write(namespace.output_dir().join("source.bin"), b"private").unwrap();
        let destination = ManagedRelativeName::try_from("destination.bin").unwrap();
        let result = namespace_fs.rename_within_after_commit(&output, source, &destination, || {
            fs::remove_file(namespace.output_dir().join("destination.bin")).unwrap();
            symlink(external.path(), namespace.output_dir().join("destination.bin")).unwrap();
        }).unwrap();
        let mut retained = fs::File::from(duplicate_descriptor(&result.descriptor).unwrap());
        let mut bytes = Vec::new();
        retained.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"private");
        assert!(namespace_fs.unlink_within(&output, result).is_err());
        assert!(fs::symlink_metadata(namespace.output_dir().join("destination.bin")).unwrap().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn replacement_requires_the_current_destination_capability() {
        let root = temporary_directory();
        let (namespace, _data_root, namespace_fs, directories) = prepared_namespace(root.path());
        let output = namespace_fs.open_managed_dir(&directories, ManagedUserArea::Output).unwrap();
        let source_name = ManagedRelativeName::try_from("source.bin").unwrap();
        let destination_name = ManagedRelativeName::try_from("destination.bin").unwrap();
        let source = namespace_fs.create_new_regular(&output, &source_name).unwrap();
        let destination = namespace_fs.create_new_regular(&output, &destination_name).unwrap();
        fs::write(namespace.output_dir().join("source.bin"), b"new").unwrap();
        fs::write(namespace.output_dir().join("destination.bin"), b"old").unwrap();
        let replaced = namespace_fs.replace_within(&output, source, destination).unwrap();
        assert_eq!(fs::read(namespace.output_dir().join("destination.bin")).unwrap(), b"new");
        assert!(!namespace.output_dir().join("source.bin").exists());
        let next = namespace_fs.create_new_regular(&output, &source_name).unwrap();
        fs::remove_file(namespace.output_dir().join("destination.bin")).unwrap();
        fs::write(namespace.output_dir().join("destination.bin"), b"changed").unwrap();
        assert!(namespace_fs.replace_within(&output, next, replaced).is_err());
        assert_eq!(fs::read(namespace.output_dir().join("destination.bin")).unwrap(), b"changed");
        assert!(namespace.output_dir().join("source.bin").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rename_collision_preserves_both_existing_files() {
        let root = temporary_directory();
        let (namespace, _data_root, namespace_fs, directories) =
            prepared_namespace(root.path());
        let output = namespace_fs
            .open_managed_dir(&directories, ManagedUserArea::Output)
            .unwrap();
        let source_name = ManagedRelativeName::try_from("source.bin").unwrap();
        let source = namespace_fs.create_new_regular(&output, &source_name).unwrap();
        fs::write(namespace.output_dir().join("source.bin"), b"source").unwrap();
        fs::write(namespace.output_dir().join("occupied.bin"), b"existing").unwrap();
        let destination = ManagedRelativeName::try_from("occupied.bin").unwrap();

        assert!(namespace_fs.rename_within(&output, source, &destination).is_err());
        assert_eq!(fs::read(namespace.output_dir().join("source.bin")).unwrap(), b"source");
        assert_eq!(fs::read(namespace.output_dir().join("occupied.bin")).unwrap(), b"existing");
    }

    #[cfg(unix)]
    #[test]
    fn rename_and_unlink_require_matching_retained_file_capabilities() {
        let root = temporary_directory();
        let (namespace, _data_root, namespace_fs, directories) =
            prepared_namespace(root.path());
        let output = namespace_fs
            .open_managed_dir(&directories, ManagedUserArea::Output)
            .unwrap();
        let recovery = namespace_fs
            .open_managed_dir(&directories, ManagedUserArea::Recovery)
            .unwrap();

        let source_name = ManagedRelativeName::try_from("source.bin").unwrap();
        let source = namespace_fs
            .create_new_regular(&output, &source_name)
            .unwrap();
        fs::write(namespace.output_dir().join("source.bin"), b"private").unwrap();
        let destination_name = ManagedRelativeName::try_from("renamed.bin").unwrap();
        let renamed = namespace_fs
            .rename_within(&output, source, &destination_name)
            .unwrap();
        assert!(!namespace.output_dir().join("source.bin").exists());
        assert_eq!(
            fs::read(namespace.output_dir().join("renamed.bin")).unwrap(),
            b"private"
        );
        namespace_fs.unlink_within(&output, renamed).unwrap();
        assert!(!namespace.output_dir().join("renamed.bin").exists());

        let wrong_area_name = ManagedRelativeName::try_from("wrong-area.bin").unwrap();
        let wrong_area_file = namespace_fs
            .create_new_regular(&output, &wrong_area_name)
            .unwrap();
        assert!(namespace_fs
            .rename_within(&recovery, wrong_area_file, &destination_name)
            .is_err());
        assert!(namespace.output_dir().join("wrong-area.bin").exists());

        let other_root = temporary_directory();
        let (_other_namespace, _other_data_root, other_fs, other_directories) =
            prepared_namespace(other_root.path());
        let other_output = other_fs
            .open_managed_dir(&other_directories, ManagedUserArea::Output)
            .unwrap();
        let foreign_name = ManagedRelativeName::try_from("foreign.bin").unwrap();
        let foreign = namespace_fs
            .create_new_regular(&output, &foreign_name)
            .unwrap();
        assert!(other_fs.unlink_within(&other_output, foreign).is_err());
        assert!(namespace.output_dir().join("foreign.bin").exists());

        let stale_name = ManagedRelativeName::try_from("stale.bin").unwrap();
        let stale = namespace_fs
            .create_new_regular(&output, &stale_name)
            .unwrap();
        fs::remove_file(namespace.output_dir().join("stale.bin")).unwrap();
        fs::write(namespace.output_dir().join("stale.bin"), b"replacement").unwrap();
        assert!(namespace_fs.unlink_within(&output, stale).is_err());
        assert_eq!(
            fs::read(namespace.output_dir().join("stale.bin")).unwrap(),
            b"replacement"
        );
    }

    #[cfg(not(any(unix, windows)))]
    #[test]
    fn namespace_filesystem_fails_closed_without_audited_handle_relative_support() {
        let root = temporary_directory();
        assert!(NamespaceFs::open_data_root(root.path()).is_err());
    }
}
