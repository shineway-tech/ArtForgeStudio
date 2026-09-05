//! Owner-bound user namespaces and their capability-only filesystem boundary.
#![allow(dead_code)]

use anyhow::{anyhow, ensure, Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

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
pub(crate) struct DataRootCapability {
    descriptor: OwnedFd,
    identity: ObjectIdentity,
    display_root: PathBuf,
}

#[cfg(not(unix))]
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

#[cfg(not(unix))]
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

#[cfg(not(unix))]
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

#[cfg(not(unix))]
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

#[cfg(not(unix))]
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

#[cfg(not(unix))]
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

#[cfg(not(unix))]
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

    #[cfg(not(unix))]
    #[test]
    fn namespace_filesystem_fails_closed_without_audited_handle_relative_support() {
        let root = temporary_directory();
        assert!(NamespaceFs::open_data_root(root.path()).is_err());
    }
}
