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
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct NamespaceStorageAuthority {
    data_root: Arc<DataRootCapability>,
    lease: NamespaceLease,
    fs: NamespaceFs,
    directories: ManagedNamespaceDirectories,
}

pub(crate) struct NamespaceManagedFile {
    key: ManagedFileKey,
    capability: ManagedFileCapability,
}

pub(crate) trait ManagedReadSeek: std::io::Read + std::io::Seek {}
impl<T: std::io::Read + std::io::Seek> ManagedReadSeek for T {}

impl NamespaceManagedFile {
    pub(crate) fn key(&self) -> &ManagedFileKey {
        &self.key
    }
}

pub(crate) struct NamespaceManagedFileCheck<'a> {
    pub(crate) file: &'a NamespaceManagedFile,
    pub(crate) expected: Option<StableFileIdentity>,
}

pub(crate) enum NamespaceManagedPublication<'a> {
    Absent(&'a ManagedFileKey),
    Replace(&'a NamespaceManagedFile),
}

impl NamespaceStorageAuthority {
    /// Worker-only local reading/seeking and decode. No network/UI/reentry;
    /// the returned value is accepted only after retained post-validation.
    pub(crate) fn with_regular_reader<T>(
        &self, file: &mut NamespaceManagedFile,
        operation: impl FnOnce(&mut dyn ManagedReadSeek) -> Result<T>,
    ) -> Result<T> {
        let directory = self.fs.open_managed_dir(&self.directories, file.key.area())?;
        self.fs.with_regular_reader(&directory, &mut file.capability, operation)
    }
    pub(crate) fn open(data_root: Arc<DataRootCapability>, lease: &NamespaceLease) -> Result<Self> {
        let fs = NamespaceFs::for_namespace(data_root.as_ref(), &lease.namespace)?;
        let directories = fs.ensure_managed_dirs()?;
        Ok(Self {
            data_root,
            lease: lease.clone(),
            fs,
            directories,
        })
    }
    pub(crate) fn lease(&self) -> &NamespaceLease {
        &self.lease
    }
    pub(crate) fn user_public_id(&self) -> &str {
        self.lease.namespace.user_public_id()
    }
    pub(crate) fn create_new_regular(&self, key: &ManagedFileKey) -> Result<NamespaceManagedFile> {
        let directory = self.fs.open_managed_dir(&self.directories, key.area())?;
        let capability = self
            .fs
            .create_new_regular(&directory, key.relative_name())?;
        Ok(NamespaceManagedFile {
            key: key.clone(),
            capability,
        })
    }
    pub(crate) fn open_existing_regular(
        &self,
        key: &ManagedFileKey,
    ) -> Result<NamespaceManagedFile> {
        let directory = self.fs.open_managed_dir(&self.directories, key.area())?;
        let capability = self
            .fs
            .open_existing_regular(&directory, key.relative_name())?;
        Ok(NamespaceManagedFile {
            key: key.clone(),
            capability,
        })
    }
    pub(crate) fn open_optional_regular(
        &self,
        key: &ManagedFileKey,
    ) -> Result<Option<NamespaceManagedFile>> {
        let directory = self.fs.open_managed_dir(&self.directories, key.area())?;
        Ok(self
            .fs
            .open_optional_regular(&directory, key.relative_name())?
            .map(|capability| NamespaceManagedFile {
                key: key.clone(),
                capability,
            }))
    }
    pub(crate) fn inspect_regular(
        &self,
        file: &NamespaceManagedFile,
    ) -> Result<ManagedFileMetadata> {
        let directory = self
            .fs
            .open_managed_dir(&self.directories, file.key.area())?;
        self.fs.inspect_regular(&directory, &file.capability)
    }
    pub(crate) fn enumerate_regular_names(
        &self,
        area: ManagedUserArea,
    ) -> Result<Vec<ManagedRelativeName>> {
        let directory = self.fs.open_managed_dir(&self.directories, area)?;
        self.fs.enumerate_regular_names(&directory)
    }
    /// The callback may perform scalar validation and SQLite work only. Acquire
    /// SQLite inside it, never while acquiring directories or this file guard.
    pub(crate) fn with_current_regular_files<T>(
        &self,
        files: &[NamespaceManagedFileCheck<'_>],
        operation: impl FnOnce(&[ManagedFileMetadata]) -> Result<T>,
    ) -> Result<T> {
        let directories = files
            .iter()
            .map(|check| {
                self.fs
                    .open_managed_dir(&self.directories, check.file.key.area())
            })
            .collect::<Result<Vec<_>>>()?;
        let checks = files
            .iter()
            .zip(&directories)
            .map(|(check, directory)| ManagedFileCheck {
                directory,
                file: &check.file.capability,
                expected: check.expected,
            })
            .collect::<Vec<_>>();
        self.fs.with_current_regular_files(&checks, operation)
    }
    pub(crate) fn create_temporary_regular_for(
        &self,
        destination: &ManagedFileKey,
    ) -> Result<NamespaceManagedFile> {
        self.create_temporary_regular_for_uuid(destination, Uuid::new_v4())
    }
    fn create_temporary_regular_for_uuid(
        &self,
        destination: &ManagedFileKey,
        candidate: Uuid,
    ) -> Result<NamespaceManagedFile> {
        let temporary_leaf = format!(".af-managed-{}.tmp", candidate.simple());
        let relative_name = destination
            .relative_name()
            .as_str()
            .rsplit_once('/')
            .map_or_else(
                || temporary_leaf.clone(),
                |(parent, _)| format!("{parent}/{temporary_leaf}"),
            );
        let temporary_key = ManagedFileKey::new(destination.area(), &relative_name)?;
        self.create_new_regular(&temporary_key)
    }
    pub(crate) fn read_regular_to(
        &self,
        file: &mut NamespaceManagedFile,
        sink: &mut dyn std::io::Write,
    ) -> Result<u64> {
        let directory = self
            .fs
            .open_managed_dir(&self.directories, file.key.area())?;
        self.fs
            .read_regular_to(&directory, &mut file.capability, sink)
    }
    pub(crate) fn write_new_regular_from(
        &self,
        file: &mut NamespaceManagedFile,
        source: &mut dyn std::io::Read,
    ) -> Result<u64> {
        let directory = self
            .fs
            .open_managed_dir(&self.directories, file.key.area())?;
        self.fs
            .write_new_regular_from(&directory, &mut file.capability, source)
    }
    pub(crate) fn sync_regular(&self, file: &mut NamespaceManagedFile) -> Result<()> {
        let directory = self
            .fs
            .open_managed_dir(&self.directories, file.key.area())?;
        self.fs.sync_regular(&directory, &mut file.capability)
    }
    pub(crate) fn publish_regular(
        &self,
        source: &mut NamespaceManagedFile,
        destination: NamespaceManagedPublication<'_>,
    ) -> Result<()> {
        let destination_key = match &destination {
            NamespaceManagedPublication::Absent(key) => (*key).clone(),
            NamespaceManagedPublication::Replace(file) => file.key.clone(),
        };
        ensure!(
            source.key.area() == destination_key.area(),
            "publication must stay within one managed area"
        );
        let directory = self
            .fs
            .open_managed_dir(&self.directories, source.key.area())?;
        let publication = match destination {
            NamespaceManagedPublication::Absent(key) => {
                ManagedPublication::Absent(key.relative_name())
            }
            NamespaceManagedPublication::Replace(file) => {
                ManagedPublication::Replace(&file.capability)
            }
        };
        self.fs
            .publish_regular(&directory, &mut source.capability, publication)?;
        source.key = destination_key;
        Ok(())
    }
    pub(crate) fn unlink_regular(&self, file: NamespaceManagedFile) -> Result<()> {
        let directory = self
            .fs
            .open_managed_dir(&self.directories, file.key.area())?;
        self.fs.unlink_within(&directory, file.capability)
    }
}

#[cfg(test)]
mod namespace_authority_tests {
    use super::*;
    use std::fs;

    const AUTHORITY_USER_A: &str = "11111111-1111-4111-8111-111111111111";

    fn authority_fixture(
        user_public_id: &str,
    ) -> (tempfile::TempDir, NamespaceLease, NamespaceStorageAuthority) {
        let temporary_parent = fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = tempfile::tempdir_in(temporary_parent).unwrap();
        let lease = NamespaceLease {
            namespace: UserNamespace::new(root.path(), user_public_id).unwrap(),
            auth_epoch: 1,
            namespace_epoch: 2,
        };
        let data_root = Arc::new(NamespaceFs::open_data_root(root.path()).unwrap());
        let authority = NamespaceStorageAuthority::open(data_root, &lease).unwrap();
        (root, lease, authority)
    }

    fn candidate_uuid(value: &str) -> Uuid {
        Uuid::parse_str(value).unwrap()
    }

    #[test]
    fn namespace_delivery_reader_seeks_revalidates_and_preserves_typed_error() {
        let (_root, lease, authority) = authority_fixture(AUTHORITY_USER_A);
        let key = ManagedFileKey::new(ManagedUserArea::Output, "reader").unwrap();
        let mut file = authority.create_new_regular(&key).unwrap();
        authority.write_new_regular_from(&mut file, &mut &b"abcdef"[..]).unwrap();
        let bytes = authority.with_regular_reader(&mut file, |reader| {
            let mut bytes = [0; 3];
            reader.seek(std::io::SeekFrom::Start(2))?;
            reader.read_exact(&mut bytes)?;
            reader.rewind()?;
            let mut first = [0; 1];
            reader.read_exact(&mut first)?;
            Ok((bytes, first))
        }).unwrap();
        assert_eq!(bytes, (*b"cde", *b"a"));
        let error = authority.with_regular_reader::<()>(&mut file, |_| {
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into())
        }).unwrap_err();
        assert_eq!(error.downcast_ref::<std::io::Error>().unwrap().kind(), std::io::ErrorKind::Interrupted);
        let (_other_root, _, other) = authority_fixture(AUTHORITY_USER_A);
        let called = std::cell::Cell::new(false);
        assert!(other.with_regular_reader(&mut file, |_| { called.set(true); Ok(()) }).is_err());
        assert!(!called.get());
        let path = lease.namespace.path(ManagedUserArea::Output).join("reader");
        assert!(authority.with_regular_reader(&mut file, |_| {
            fs::rename(&path, path.with_file_name("retained-reader"))?;
            fs::write(&path, b"replacement")?;
            Ok(())
        }).is_err());
        assert_eq!(fs::read(path).unwrap(), b"replacement");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn task8c0_authority_forwards_absent_and_replace_lifecycles() {
        let (_root, lease, authority) = authority_fixture(AUTHORITY_USER_A);
        let absent_key = ManagedFileKey::new(ManagedUserArea::Recovery, "first.json").unwrap();
        let mut absent = authority.create_temporary_regular_for(&absent_key).unwrap();
        assert_eq!(
            authority
                .write_new_regular_from(&mut absent, &mut &b"first bytes"[..])
                .unwrap(),
            11
        );
        authority.sync_regular(&mut absent).unwrap();
        authority
            .publish_regular(
                &mut absent,
                NamespaceManagedPublication::Absent(&absent_key),
            )
            .unwrap();
        assert_eq!(absent.key(), &absent_key);
        assert_eq!(authority.inspect_regular(&absent).unwrap().byte_size, 11);
        assert_eq!(
            authority
                .with_current_regular_files(
                    &[NamespaceManagedFileCheck {
                        file: &absent,
                        expected: None,
                    }],
                    |metadata| Ok(metadata[0].byte_size),
                )
                .unwrap(),
            11
        );
        let mut bytes = Vec::new();
        assert_eq!(
            authority.read_regular_to(&mut absent, &mut bytes).unwrap(),
            11
        );
        assert_eq!(bytes, b"first bytes");
        authority.unlink_regular(absent).unwrap();
        assert!(!lease.namespace.recovery_dir().join("first.json").exists());

        let replace_key = ManagedFileKey::new(ManagedUserArea::Recovery, "second.json").unwrap();
        fs::write(
            lease.namespace.recovery_dir().join("second.json"),
            b"old bytes",
        )
        .unwrap();
        let destination = authority.open_existing_regular(&replace_key).unwrap();
        let mut replacement = authority
            .create_temporary_regular_for(&replace_key)
            .unwrap();
        assert_eq!(
            authority
                .write_new_regular_from(&mut replacement, &mut &b"replacement"[..])
                .unwrap(),
            11
        );
        authority.sync_regular(&mut replacement).unwrap();
        authority
            .publish_regular(
                &mut replacement,
                NamespaceManagedPublication::Replace(&destination),
            )
            .unwrap();
        assert_eq!(replacement.key(), &replace_key);
        let mut replaced_bytes = Vec::new();
        assert_eq!(
            authority
                .read_regular_to(&mut replacement, &mut replaced_bytes)
                .unwrap(),
            11
        );
        assert_eq!(replaced_bytes, b"replacement");
        assert_eq!(
            authority.inspect_regular(&replacement).unwrap().byte_size,
            11
        );
        authority.unlink_regular(replacement).unwrap();
        assert!(!lease.namespace.recovery_dir().join("second.json").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn task8c0_temporary_names_are_one_exclusive_same_parent_uuid_candidate() {
        let (_root, lease, authority) = authority_fixture(AUTHORITY_USER_A);
        let root_key = ManagedFileKey::new(ManagedUserArea::Recovery, "document.json").unwrap();
        let root_uuid = candidate_uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let root_file = authority
            .create_temporary_regular_for_uuid(&root_key, root_uuid)
            .unwrap();
        assert_eq!(
            root_file.key().relative_name().as_str(),
            ".af-managed-aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa.tmp"
        );
        authority.unlink_regular(root_file).unwrap();

        fs::create_dir(lease.namespace.recovery_dir().join("nested")).unwrap();
        let nested_key =
            ManagedFileKey::new(ManagedUserArea::Recovery, "nested/document.json").unwrap();
        let nested_uuid = candidate_uuid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
        let nested_file = authority
            .create_temporary_regular_for_uuid(&nested_key, nested_uuid)
            .unwrap();
        assert_eq!(
            nested_file.key().relative_name().as_str(),
            "nested/.af-managed-bbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb.tmp"
        );
        authority.unlink_regular(nested_file).unwrap();

        let collision_path = lease
            .namespace
            .recovery_dir()
            .join(".af-managed-cccccccccccc4ccc8ccccccccccccccc.tmp");
        fs::write(&collision_path, b"existing candidate").unwrap();
        let collision_uuid = candidate_uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
        let collision_error = authority
            .create_temporary_regular_for_uuid(&root_key, collision_uuid)
            .err()
            .expect("exclusive creation must retain the collision error");
        #[cfg(unix)]
        assert_eq!(
            collision_error.downcast_ref::<rustix::io::Errno>(),
            Some(&rustix::io::Errno::EXIST)
        );
        #[cfg(windows)]
        assert!(collision_error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists));
        assert_eq!(fs::read(&collision_path).unwrap(), b"existing candidate");
        assert_eq!(
            fs::read_dir(lease.namespace.recovery_dir())
                .unwrap()
                .count(),
            2
        );

        let missing_parent =
            ManagedFileKey::new(ManagedUserArea::Recovery, "absent/document.json").unwrap();
        assert!(authority
            .create_temporary_regular_for_uuid(
                &missing_parent,
                candidate_uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
            )
            .is_err());
        assert!(!lease.namespace.recovery_dir().join("absent").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn task8c0_publication_conflicts_preserve_owned_wrapper_and_typed_error() {
        let (_root, lease, authority) = authority_fixture(AUTHORITY_USER_A);
        let key = ManagedFileKey::new(ManagedUserArea::Recovery, "document.json").unwrap();
        let mut absent_source = authority
            .create_temporary_regular_for_uuid(
                &key,
                candidate_uuid("11111111-1111-4111-8111-111111111111"),
            )
            .unwrap();
        let absent_temp_key = absent_source.key().clone();
        authority
            .write_new_regular_from(&mut absent_source, &mut &b"owned absent"[..])
            .unwrap();
        authority.sync_regular(&mut absent_source).unwrap();
        fs::write(
            lease.namespace.recovery_dir().join("document.json"),
            b"winner",
        )
        .unwrap();
        let appeared = authority
            .publish_regular(
                &mut absent_source,
                NamespaceManagedPublication::Absent(&key),
            )
            .unwrap_err();
        assert_eq!(
            appeared.downcast_ref::<ManagedPublicationConflict>(),
            Some(&ManagedPublicationConflict::DestinationAppeared)
        );
        assert_eq!(absent_source.key(), &absent_temp_key);
        let mut owned_absent = Vec::new();
        authority
            .read_regular_to(&mut absent_source, &mut owned_absent)
            .unwrap();
        assert_eq!(owned_absent, b"owned absent");
        authority.unlink_regular(absent_source).unwrap();
        assert_eq!(
            fs::read(lease.namespace.recovery_dir().join("document.json")).unwrap(),
            b"winner"
        );

        let stale = authority.open_existing_regular(&key).unwrap();
        fs::rename(
            lease.namespace.recovery_dir().join("document.json"),
            lease.namespace.recovery_dir().join("old-winner.json"),
        )
        .unwrap();
        fs::write(
            lease.namespace.recovery_dir().join("document.json"),
            b"new winner",
        )
        .unwrap();
        let mut replace_source = authority
            .create_temporary_regular_for_uuid(
                &key,
                candidate_uuid("22222222-2222-4222-8222-222222222222"),
            )
            .unwrap();
        let replace_temp_key = replace_source.key().clone();
        authority
            .write_new_regular_from(&mut replace_source, &mut &b"owned replace"[..])
            .unwrap();
        authority.sync_regular(&mut replace_source).unwrap();
        let changed = authority
            .publish_regular(
                &mut replace_source,
                NamespaceManagedPublication::Replace(&stale),
            )
            .unwrap_err();
        assert_eq!(
            changed.downcast_ref::<ManagedPublicationConflict>(),
            Some(&ManagedPublicationConflict::DestinationChanged)
        );
        assert_eq!(replace_source.key(), &replace_temp_key);
        assert_eq!(
            authority
                .inspect_regular(&replace_source)
                .unwrap()
                .byte_size,
            13
        );
        authority.unlink_regular(replace_source).unwrap();
        assert_eq!(
            fs::read(lease.namespace.recovery_dir().join("document.json")).unwrap(),
            b"new winner"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn task8c0_forwarders_retain_producer_ownership_state_and_stream_checks() {
        struct PartialFailure(bool);
        impl std::io::Read for PartialFailure {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    Err(std::io::Error::other("fixture source failed"))
                } else {
                    self.0 = true;
                    buffer[0] = b'x';
                    Ok(1)
                }
            }
        }

        let (root, lease, authority) = authority_fixture(AUTHORITY_USER_A);
        let other_lease = NamespaceLease {
            namespace: UserNamespace::new(root.path(), "22222222-2222-4222-8222-222222222222")
                .unwrap(),
            auth_epoch: 1,
            namespace_epoch: 2,
        };
        let other_root = Arc::new(NamespaceFs::open_data_root(root.path()).unwrap());
        let other = NamespaceStorageAuthority::open(other_root, &other_lease).unwrap();
        let key = ManagedFileKey::new(ManagedUserArea::Recovery, "document.json").unwrap();
        let mut foreign = authority
            .create_temporary_regular_for_uuid(
                &key,
                candidate_uuid("33333333-3333-4333-8333-333333333333"),
            )
            .unwrap();
        let same_user_root = Arc::new(NamespaceFs::open_data_root(root.path()).unwrap());
        let same_user = NamespaceStorageAuthority::open(same_user_root, &lease).unwrap();
        let mut foreign_sink = Vec::new();
        assert!(same_user
            .read_regular_to(&mut foreign, &mut foreign_sink)
            .is_err());
        assert!(foreign_sink.is_empty());
        assert!(other.sync_regular(&mut foreign).is_err());

        let output_key = ManagedFileKey::new(ManagedUserArea::Output, "document.json").unwrap();
        assert!(authority
            .publish_regular(
                &mut foreign,
                NamespaceManagedPublication::Absent(&output_key),
            )
            .is_err());
        assert_ne!(foreign.key(), &output_key);
        assert!(authority
            .publish_regular(&mut foreign, NamespaceManagedPublication::Absent(&key),)
            .is_err());
        authority
            .write_new_regular_from(&mut foreign, &mut &b"bytes"[..])
            .unwrap();
        assert!(authority
            .publish_regular(&mut foreign, NamespaceManagedPublication::Absent(&key),)
            .is_err());
        authority.sync_regular(&mut foreign).unwrap();

        let foreign_key = ManagedFileKey::new(ManagedUserArea::Recovery, "foreign.json").unwrap();
        fs::write(
            other_lease.namespace.recovery_dir().join("foreign.json"),
            b"foreign destination",
        )
        .unwrap();
        let foreign_destination = other.open_existing_regular(&foreign_key).unwrap();
        let foreign_error = authority
            .publish_regular(
                &mut foreign,
                NamespaceManagedPublication::Replace(&foreign_destination),
            )
            .unwrap_err();
        assert!(foreign_error
            .downcast_ref::<ManagedPublicationConflict>()
            .is_none());
        assert_ne!(foreign.key(), &foreign_key);
        assert_eq!(
            fs::read(other_lease.namespace.recovery_dir().join("foreign.json")).unwrap(),
            b"foreign destination"
        );

        let hardlink_path = lease.namespace.recovery_dir().join("hardlink");
        fs::hard_link(
            lease
                .namespace
                .recovery_dir()
                .join(foreign.key().relative_name().as_str()),
            &hardlink_path,
        )
        .unwrap();
        assert!(authority.inspect_regular(&foreign).is_err());
        fs::remove_file(hardlink_path).unwrap();
        authority.unlink_regular(foreign).unwrap();

        let mut poisoned = authority
            .create_temporary_regular_for_uuid(
                &key,
                candidate_uuid("44444444-4444-4444-8444-444444444444"),
            )
            .unwrap();
        let failure = authority
            .write_new_regular_from(&mut poisoned, &mut PartialFailure(false))
            .unwrap_err();
        assert_eq!(
            failure.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::Other
        );
        assert!(authority.sync_regular(&mut poisoned).is_err());
        assert!(authority
            .publish_regular(&mut poisoned, NamespaceManagedPublication::Absent(&key),)
            .is_err());
        authority.unlink_regular(poisoned).unwrap();

        fs::create_dir(lease.namespace.recovery_dir().join("Nested")).unwrap();
        fs::write(lease.namespace.recovery_dir().join("Nested/file"), b"exact").unwrap();
        if lease.namespace.recovery_dir().join("nested/file").exists() {
            let alias_key = ManagedFileKey::new(ManagedUserArea::Recovery, "nested/file").unwrap();
            assert!(authority.open_existing_regular(&alias_key).is_err());
        }

        fs::create_dir(lease.namespace.recovery_dir().join("stale")).unwrap();
        let stale_key =
            ManagedFileKey::new(ManagedUserArea::Recovery, "stale/document.json").unwrap();
        let mut stale_source = authority
            .create_temporary_regular_for_uuid(
                &stale_key,
                candidate_uuid("55555555-5555-4555-8555-555555555555"),
            )
            .unwrap();
        authority
            .write_new_regular_from(&mut stale_source, &mut &b"stale"[..])
            .unwrap();
        authority.sync_regular(&mut stale_source).unwrap();
        fs::rename(
            lease.namespace.recovery_dir().join("stale"),
            lease.namespace.recovery_dir().join("detached"),
        )
        .unwrap();
        fs::create_dir(lease.namespace.recovery_dir().join("stale")).unwrap();
        assert!(authority
            .publish_regular(
                &mut stale_source,
                NamespaceManagedPublication::Absent(&stale_key),
            )
            .is_err());
        assert!(authority.unlink_regular(stale_source).is_err());
        assert!(lease
            .namespace
            .recovery_dir()
            .join("detached/.af-managed-55555555555545558555555555555555.tmp")
            .is_file());
    }

    #[test]
    fn authority_checks_all_files_in_order_and_preserves_callback_errors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap();
        let root = Arc::new(NamespaceFs::open_data_root(&path).unwrap());
        let lease = NamespaceLease {
            namespace: UserNamespace::new(&path, "11111111-1111-4111-8111-111111111111").unwrap(),
            auth_epoch: 1,
            namespace_epoch: 2,
        };
        let authority = NamespaceStorageAuthority::open(Arc::clone(&root), &lease).unwrap();
        let other = NamespaceStorageAuthority::open(Arc::clone(&root), &lease).unwrap();
        let first_key = ManagedFileKey::new(ManagedUserArea::Output, "first").unwrap();
        let second_key = ManagedFileKey::new(ManagedUserArea::Previews, "second").unwrap();
        assert!(authority
            .open_optional_regular(&first_key)
            .unwrap()
            .is_none());
        let first = authority.create_new_regular(&first_key).unwrap();
        let second = authority.create_new_regular(&second_key).unwrap();
        std::fs::write(lease.namespace.output_dir().join("first"), b"first").unwrap();
        std::fs::write(lease.namespace.preview_dir().join("second"), b"two").unwrap();
        let first_info = authority.inspect_regular(&first).unwrap();
        let second_info = authority.inspect_regular(&second).unwrap();
        let checks = [
            NamespaceManagedFileCheck {
                file: &second,
                expected: Some(second_info.identity),
            },
            NamespaceManagedFileCheck {
                file: &first,
                expected: Some(first_info.identity),
            },
        ];
        let ordered = authority
            .with_current_regular_files(&checks, |metadata| Ok(metadata.to_vec()))
            .unwrap();
        assert_eq!(
            ordered.iter().map(|m| m.byte_size).collect::<Vec<_>>(),
            vec![3, 5]
        );
        assert_eq!(
            authority
                .enumerate_regular_names(ManagedUserArea::Output)
                .unwrap(),
            vec![ManagedRelativeName::try_from("first").unwrap()]
        );
        assert_eq!(
            authority
                .inspect_regular(&authority.open_existing_regular(&first_key).unwrap())
                .unwrap(),
            first_info
        );
        assert!(other
            .with_current_regular_files(&checks, |_| -> Result<()> {
                panic!("foreign callback must not run")
            })
            .is_err());
        assert!(authority
            .with_current_regular_files(
                &[NamespaceManagedFileCheck {
                    file: &first,
                    expected: Some(second_info.identity)
                }],
                |_| -> Result<()> { panic!("wrong identity callback must not run") }
            )
            .is_err());
        assert!(authority
            .with_current_regular_files(&[], |_| -> Result<()> {
                panic!("empty callback must not run")
            })
            .is_err());
        let error = authority
            .with_current_regular_files(&checks, |_| -> Result<()> {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into())
            })
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(authority.create_new_regular(&first_key).is_err());
        assert_eq!(
            std::fs::read(lease.namespace.output_dir().join("first")).unwrap(),
            b"first"
        );
    }
    #[test]
    fn authority_retains_exact_root_lease_and_bound_file_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap();
        let root = Arc::new(NamespaceFs::open_data_root(&path).unwrap());
        let lease = NamespaceLease {
            namespace: UserNamespace::new(&path, "11111111-1111-4111-8111-111111111111").unwrap(),
            auth_epoch: 42,
            namespace_epoch: 9,
        };
        let authority = NamespaceStorageAuthority::open(Arc::clone(&root), &lease).unwrap();
        assert!(Arc::ptr_eq(&root, &authority.data_root));
        assert_eq!(authority.lease(), &lease);
        let key = ManagedFileKey::new(ManagedUserArea::CanvasUploads, "a.png").unwrap();
        let file = authority.create_new_regular(&key).unwrap();
        assert_eq!(file.key(), &key);
        assert!(lease
            .namespace
            .path(ManagedUserArea::CanvasUploads)
            .join("a.png")
            .is_file());
    }
}

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
        let stem = component
            .split('.')
            .next()
            .unwrap()
            .trim_end_matches(' ')
            .to_uppercase();
        let numbered_device = stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
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
    pub(crate) fn storage_name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Prompt => "prompt",
            Self::Canvas => "canvas",
            Self::CanvasUploads => "canvas-uploads",
            Self::CanvasExports => "canvas-exports",
            Self::References => "references",
            Self::ReferencesLibrary => "references-library",
            Self::ReferencesImports => "references-imports",
            Self::Previews => "previews",
            Self::Recovery => "recovery",
            Self::DeliveryStaging => "delivery-staging",
            Self::Videos => "videos",
            Self::ToolboxCompressionInputs => "toolbox-compression-inputs",
            Self::ToolboxCompressionResults => "toolbox-compression-results",
            Self::ToolboxConversionInputs => "toolbox-conversion-inputs",
            Self::ToolboxConversionResults => "toolbox-conversion-results",
            Self::ToolboxCropInputs => "toolbox-crop-inputs",
        }
    }

    pub(crate) fn from_storage_name(value: &str) -> Result<Self> {
        MANAGED_USER_AREAS
            .into_iter()
            .find(|area| area.storage_name() == value)
            .ok_or_else(|| anyhow::anyhow!("unknown managed area"))
    }

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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ManagedRelativeName(String);

impl ManagedRelativeName {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ManagedFileKey {
    area: ManagedUserArea,
    relative_name: ManagedRelativeName,
}

impl ManagedFileKey {
    pub(crate) fn new(area: ManagedUserArea, relative_name: &str) -> Result<Self> {
        let physical = format!("{}/{}", area.relative_path(), relative_name);
        ensure!(
            !MANAGED_USER_AREAS.into_iter().any(|other| other != area
                && (physical == other.relative_path()
                    || physical.starts_with(&format!("{}/", other.relative_path())))
                && other
                    .relative_path()
                    .starts_with(&format!("{}/", area.relative_path()))),
            "name belongs to a more specific managed area"
        );
        Ok(Self {
            area,
            relative_name: ManagedRelativeName::try_from(relative_name)?,
        })
    }
    pub(crate) fn area(&self) -> ManagedUserArea {
        self.area
    }
    pub(crate) fn relative_name(&self) -> &ManagedRelativeName {
        &self.relative_name
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StableFileIdentity {
    Unix { device: u64, inode: u64 },
    Windows { volume: u64, file_id: [u8; 16] },
}

impl StableFileIdentity {
    pub(crate) fn to_storage_bytes(self) -> Vec<u8> {
        match self {
            Self::Unix { device, inode } => [
                vec![1],
                device.to_be_bytes().to_vec(),
                inode.to_be_bytes().to_vec(),
            ]
            .concat(),
            Self::Windows { volume, file_id } => {
                [vec![2], volume.to_be_bytes().to_vec(), file_id.to_vec()].concat()
            }
        }
    }
    pub(crate) fn from_storage_bytes(bytes: &[u8]) -> Result<Self> {
        match bytes.first() {
            Some(1) if bytes.len() == 17 => Ok(Self::Unix {
                device: u64::from_be_bytes(bytes[1..9].try_into()?),
                inode: u64::from_be_bytes(bytes[9..17].try_into()?),
            }),
            Some(2) if bytes.len() == 25 => Ok(Self::Windows {
                volume: u64::from_be_bytes(bytes[1..9].try_into()?),
                file_id: bytes[9..25].try_into()?,
            }),
            _ => anyhow::bail!("invalid tagged file identity encoding"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedFileMetadata {
    pub(crate) identity: StableFileIdentity,
    pub(crate) byte_size: u64,
    pub(crate) modified_at: std::time::SystemTime,
    pub(crate) link_count: u64,
}

pub(crate) struct ManagedFileCheck<'a> {
    pub(crate) directory: &'a ManagedDirectoryCapability,
    pub(crate) file: &'a ManagedFileCapability,
    pub(crate) expected: Option<StableFileIdentity>,
}

#[derive(Clone, Copy)]
pub(crate) enum ManagedPublication<'a> {
    Absent(&'a ManagedRelativeName),
    Replace(&'a ManagedFileCapability),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedPublicationConflict {
    DestinationAppeared,
    DestinationChanged,
}

impl std::fmt::Display for ManagedPublicationConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DestinationAppeared => "managed destination appeared",
            Self::DestinationChanged => "managed destination changed",
        })
    }
}
impl std::error::Error for ManagedPublicationConflict {}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ManagedWriteState {
    Existing,
    New,
    Poisoned,
    Written,
    Synced,
    Published,
}

#[cfg(any(unix, windows))]
const MANAGED_WRITE_CHUNK_BYTES: usize = 64 * 1024;

#[cfg(any(unix, windows))]
fn copy_managed_chunks(
    source: &mut dyn std::io::Read,
    mut write_chunk: impl FnMut(&[u8]) -> Result<()>,
) -> Result<u64> {
    let mut buffer = [0_u8; MANAGED_WRITE_CHUNK_BYTES];
    let mut copied = 0_u64;
    loop {
        let count = match source.read(&mut buffer) {
            Ok(0) => return Ok(copied),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        let next = copied
            .checked_add(count as u64)
            .ok_or_else(|| anyhow::anyhow!("managed copy size overflow"))?;
        write_chunk(&buffer[..count])?;
        copied = next;
    }
}

impl TryFrom<&str> for ManagedRelativeName {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        ensure!(
            !value.contains('\\'),
            "backslash is not a managed separator"
        );
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
    write_state: ManagedWriteState,
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
    pub(crate) fn open_optional_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        name: &ManagedRelativeName,
    ) -> Result<Option<ManagedFileCapability>> {
        let _lock = self.lock_mutations()?;
        let mut chain = self.checked_directory_chain(directory)?;
        let leaf = checked_relative_chain(&mut chain, directory.area, name)?;
        let parent = chain.last().unwrap();
        let Some(descriptor) = optional_checked_regular(parent, &leaf)? else {
            let mut current = self.checked_directory_chain(directory)?;
            checked_relative_chain(&mut current, directory.area, name)?;
            ensure!(
                directory_identity(current.last().unwrap())? == directory_identity(parent)?,
                "missing leaf parent detached"
            );
            return Ok(None);
        };
        let identity = regular_file_identity(&descriptor)?;
        let file = ManagedFileCapability {
            descriptor,
            binding_id: self.binding_id,
            area: directory.area,
            relative_name: name.0.clone(),
            parent_identity: directory_identity(parent)?,
            identity,
            write_state: ManagedWriteState::Existing,
        };
        self.checked_file_chain(directory, &file)?;
        Ok(Some(file))
    }

    /// Streams must not reenter this namespace or perform UI/network work.
    /// A failed sink may contain partial bytes; callers must discard it.
    pub(crate) fn read_regular_to(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &mut ManagedFileCapability,
        sink: &mut dyn std::io::Write,
    ) -> Result<u64> {
        self.with_regular_reader(directory, file, |reader| Ok(std::io::copy(reader, sink)?))
    }

    /// Worker-only bounded local reads/seeks and decoding. No network, UI or
    /// namespace reentry. Results are accepted only after retained post-validation.
    pub(crate) fn with_regular_reader<T>(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &mut ManagedFileCapability,
        operation: impl FnOnce(&mut dyn ManagedReadSeek) -> Result<T>,
    ) -> Result<T> {
        use std::io::{Seek, SeekFrom};
        let _lock = self.lock_mutations()?;
        let _chain = self.checked_file_chain(directory, file)?;
        let mut stream = std::fs::File::from(duplicate_descriptor(&file.descriptor)?);
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
            let _lock = self.lock_mutations()?;
            let _chain = self.checked_file_chain(directory, file)?;
            ensure!(
                file.write_state == ManagedWriteState::New,
                "only an owned unwritten temporary can be written"
            );
            file.write_state = ManagedWriteState::Poisoned;
            let mut stream = std::fs::File::from(duplicate_descriptor(&file.descriptor)?);
            stream.seek(SeekFrom::Start(0))?;
            stream
        };
        let copied = copy_managed_chunks(source, |chunk| {
            let _lock = self.lock_mutations()?;
            let _chain = self.checked_file_chain(directory, file)?;
            stream.write_all(chunk)?;
            self.checked_file_chain(directory, file)?;
            Ok(())
        })?;
        let _lock = self.lock_mutations()?;
        let _chain = self.checked_file_chain(directory, file)?;
        file.write_state = ManagedWriteState::Written;
        Ok(copied)
    }

    pub(crate) fn sync_regular(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &mut ManagedFileCapability,
    ) -> Result<()> {
        self.sync_regular_with(directory, file, |descriptor| {
            Ok(rustix::fs::fsync(descriptor)?)
        })
    }

    fn sync_regular_with(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &mut ManagedFileCapability,
        sync: impl FnOnce(&OwnedFd) -> Result<()>,
    ) -> Result<()> {
        let _lock = self.lock_mutations()?;
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
        sync(&file.descriptor)?;
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
        let _lock = self.lock_mutations()?;
        let _chain = self.checked_file_chain(directory, file)?;
        managed_metadata(&file.descriptor)
    }

    pub(crate) fn enumerate_regular_names(
        &self,
        directory: &ManagedDirectoryCapability,
    ) -> Result<Vec<ManagedRelativeName>> {
        let _lock = self.lock_mutations()?;
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

    /// Lock order: namespace, then SQLite. Callback is SQL/scalar-only; it must
    /// commit before returning and must never reenter filesystem/recovery APIs.
    pub(crate) fn with_current_regular_files<T>(
        &self,
        files: &[ManagedFileCheck<'_>],
        operation: impl FnOnce(&[ManagedFileMetadata]) -> Result<T>,
    ) -> Result<T> {
        ensure!(!files.is_empty(), "empty managed file validation set");
        let _lock = self.lock_mutations()?;
        let mut chains = Vec::with_capacity(files.len());
        let mut metadata = Vec::with_capacity(files.len());
        for check in files {
            chains.push(self.checked_file_chain(check.directory, check.file)?);
            let info = managed_metadata(&check.file.descriptor)?;
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
        self.publish_regular_with_current_identity(
            directory,
            source,
            destination,
            regular_file_identity,
        )
    }

    fn publish_regular_with_current_identity(
        &self,
        directory: &ManagedDirectoryCapability,
        source: &mut ManagedFileCapability,
        destination: ManagedPublication<'_>,
        inspect_current: impl FnOnce(&OwnedFd) -> Result<ObjectIdentity>,
    ) -> Result<()> {
        let _lock = self.lock_mutations()?;
        let source_chain = self.checked_file_chain(directory, source)?;
        ensure!(
            source.write_state == ManagedWriteState::Synced,
            "publication requires a written and synced owned temporary"
        );
        let source_parent = source_chain.last().unwrap();
        let source_leaf = OsStr::new(source.relative_name.rsplit('/').next().unwrap());
        let mut target_chain = self.checked_directory_chain(directory)?;
        let target_name = match destination {
            ManagedPublication::Absent(name) => name.clone(),
            ManagedPublication::Replace(file) => {
                ensure!(
                    file.binding_id == self.binding_id && file.area == directory.area,
                    "replacement belongs to another authority"
                );
                ensure!(
                    regular_file_identity(&file.descriptor)? == file.identity
                        && file.identity != source.identity,
                    "invalid retained replacement identity"
                );
                ensure!(
                    rustix::fs::fstat(&file.descriptor)?.st_nlink <= 1,
                    "hardlinked replacement"
                );
                ManagedRelativeName::try_from(file.relative_name.as_str())?
            }
        };
        let leaf = checked_relative_chain(&mut target_chain, directory.area, &target_name)?;
        let parent = target_chain.last().unwrap();
        let parent_identity = directory_identity(parent)?;
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
                match rename_without_replacement(source_parent, source_leaf, parent, &leaf) {
                    Ok(()) => {}
                    Err(rustix::io::Errno::EXIST) => {
                        self.checked_file_chain(directory, source)?;
                        self.checked_directory_chain(directory)?;
                        if optional_checked_regular(parent, &leaf)?.is_some() {
                            return Err(ManagedPublicationConflict::DestinationAppeared.into());
                        }
                        return Err(rustix::io::Errno::EXIST.into());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            ManagedPublication::Replace(file) => {
                let current_identity = current.as_ref().map(inspect_current).transpose()?;
                if current_identity != Some(file.identity) {
                    return Err(ManagedPublicationConflict::DestinationChanged.into());
                }
                rustix::fs::renameat(source_parent, source_leaf, parent, &leaf)?;
            }
        }
        // Kernel commit is final. Nothing below may fail or reopen a pathname.
        source.relative_name = target_name.0;
        source.parent_identity = parent_identity;
        source.write_state = ManagedWriteState::Published;
        Ok(())
    }

    fn checked_directory_chain(
        &self,
        directory: &ManagedDirectoryCapability,
    ) -> Result<Vec<OwnedFd>> {
        self.validate_root_descriptor()?;
        ensure!(
            directory.binding_id == self.binding_id,
            "foreign managed directory authority"
        );
        ensure!(
            directory_identity(&directory.descriptor)? == directory.identity,
            "retained directory changed"
        );
        let mut chain = vec![duplicate_descriptor(&self.root_descriptor)?];
        for (name, expected) in [
            ("accounts", directory.accounts_identity),
            (self.user_public_id.as_str(), directory.namespace_identity),
        ] {
            let child = checked_directory_at(chain.last().unwrap(), OsStr::new(name))?;
            ensure!(
                directory_identity(&child)? == expected,
                "namespace ancestor detached"
            );
            chain.push(child);
        }
        for name in directory.area.relative_path().split('/') {
            chain.push(checked_directory_at(
                chain.last().unwrap(),
                OsStr::new(name),
            )?);
        }
        ensure!(
            directory_identity(chain.last().unwrap())? == directory.identity,
            "managed area detached"
        );
        Ok(chain)
    }

    fn checked_file_chain(
        &self,
        directory: &ManagedDirectoryCapability,
        file: &ManagedFileCapability,
    ) -> Result<Vec<OwnedFd>> {
        ensure!(
            file.binding_id == self.binding_id && file.area == directory.area,
            "foreign managed file authority"
        );
        let info = managed_metadata(&file.descriptor)?;
        ensure!(
            info.identity
                == StableFileIdentity::Unix {
                    device: file.identity.device,
                    inode: file.identity.inode
                },
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
            directory_identity(parent)? == file.parent_identity,
            "managed parent changed"
        );
        let current = optional_checked_regular(parent, &leaf)?
            .ok_or_else(|| anyhow!("managed file disappeared"))?;
        ensure!(
            regular_file_identity(&current)? == file.identity,
            "managed current identity changed"
        );
        Ok(chain)
    }

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
        let mut chain = self.checked_directory_chain(directory)?;
        let leaf = checked_relative_chain(&mut chain, directory.area, name)?;
        let parent = chain.last().unwrap();
        let parent_identity = directory_identity(&parent)?;
        let descriptor = optional_checked_regular(parent, &leaf)?
            .ok_or_else(|| anyhow!("managed file missing"))?;
        let identity = regular_file_identity(&descriptor)?;
        Ok(ManagedFileCapability {
            descriptor,
            binding_id: self.binding_id,
            area: directory.area,
            relative_name: name.0.clone(),
            parent_identity,
            identity,
            write_state: ManagedWriteState::Existing,
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
        let mut chain = self.checked_directory_chain(directory)?;
        let leaf = checked_relative_chain(&mut chain, directory.area, name)?;
        let parent = chain.last().unwrap();
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
        let initialization = (|| {
            rustix::fs::fchmod(&descriptor, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR)
                .context("restrict managed file permissions")?;
            managed_metadata(&descriptor)?;
            prove_entry_spelling(&parent, &leaf, identity)?;
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = initialization {
            return Err(cleanup_failed_managed_creation(
                parent,
                &leaf,
                identity,
                error,
                regular_file_identity,
            ));
        }
        Ok(ManagedFileCapability {
            descriptor,
            binding_id: self.binding_id,
            area: directory.area,
            relative_name: name.0.clone(),
            parent_identity,
            identity,
            write_state: ManagedWriteState::New,
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
            write_state: ManagedWriteState::Published,
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
        ensure!(
            source.identity != destination.identity,
            "cannot replace a file with itself"
        );
        rustix::fs::renameat(
            &source_parent,
            &source_leaf,
            &destination_parent,
            &destination_leaf,
        )
        .context("replace the expected managed file under the mutation lock")?;
        Ok(ManagedFileCapability {
            descriptor: source.descriptor,
            binding_id: self.binding_id,
            area: directory.area,
            relative_name: destination.relative_name,
            parent_identity: destination.parent_identity,
            identity: source.identity,
            write_state: ManagedWriteState::Published,
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
        let accounts = checked_directory_at(&self.root_descriptor, OsStr::new("accounts"))?;
        ensure!(
            directory_identity(&accounts)? == accounts_identity,
            "accounts ancestor is no longer attached to this data root"
        );
        let namespace = checked_directory_at(&accounts, OsStr::new(self.user_public_id.as_str()))?;
        ensure!(
            directory_identity(&namespace)? == namespace_identity,
            "user namespace is no longer attached to its accounts ancestor"
        );
        Ok(namespace)
    }

    fn validate_managed_directory(&self, directory: &ManagedDirectoryCapability) -> Result<()> {
        ensure!(
            directory.binding_id == self.binding_id,
            "managed directory belongs to another namespace authority"
        );
        ensure!(
            directory_identity(&directory.descriptor)? == directory.identity,
            "retained managed directory identity changed"
        );
        let namespace =
            self.reopen_namespace(directory.accounts_identity, directory.namespace_identity)?;
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
fn cleanup_failed_managed_creation(
    parent: &OwnedFd,
    leaf: &OsStr,
    identity: ObjectIdentity,
    error: anyhow::Error,
    inspect_current: impl FnOnce(&OwnedFd) -> Result<ObjectIdentity>,
) -> anyhow::Error {
    if identity.inode != 0
        && open_regular_at(parent, leaf)
            .is_ok_and(|current| inspect_current(&current).is_ok_and(|id| id == identity))
    {
        if let Err(cleanup) = rustix::fs::unlinkat(parent, leaf, rustix::fs::AtFlags::empty()) {
            return error.context(format!("owned temporary cleanup failed: {cleanup}"));
        }
    }
    error
}

#[cfg(unix)]
fn managed_metadata(descriptor: &OwnedFd) -> Result<ManagedFileMetadata> {
    use std::os::unix::fs::MetadataExt;
    let file = std::fs::File::from(duplicate_descriptor(descriptor)?);
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1,
        "managed file must be regular with one link"
    );
    Ok(ManagedFileMetadata {
        identity: StableFileIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        byte_size: metadata.len(),
        modified_at: metadata.modified()?,
        link_count: metadata.nlink(),
    })
}

#[cfg(unix)]
fn prove_entry_spelling(parent: &OwnedFd, name: &OsStr, identity: ObjectIdentity) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let mut entries = rustix::fs::Dir::read_from(parent)?;
    let mut exact = false;
    for entry in &mut entries {
        let entry = entry?;
        if entry.file_name().to_bytes() == name.as_bytes() {
            ensure!(
                !exact && entry.ino() == identity.inode && identity.inode != 0,
                "ambiguous directory entry identity"
            );
            let stat = rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
            ensure!(
                stat.st_dev as u64 == identity.device && stat.st_ino as u64 == identity.inode,
                "directory entry changed"
            );
            exact = true;
        }
    }
    ensure!(exact, "exact stored directory-entry spelling is unproven");
    Ok(())
}

#[cfg(unix)]
fn checked_directory_at(parent: &OwnedFd, name: &OsStr) -> Result<OwnedFd> {
    let child = open_directory_at(parent, name)?;
    prove_entry_spelling(parent, name, directory_identity(&child)?)?;
    Ok(child)
}

#[cfg(unix)]
fn checked_relative_chain(
    chain: &mut Vec<OwnedFd>,
    area: ManagedUserArea,
    name: &ManagedRelativeName,
) -> Result<OsString> {
    ManagedFileKey::new(area, name.as_str())?;
    let components = normal_name_components(name)?;
    let (leaf, parents) = components
        .split_last()
        .ok_or_else(|| anyhow!("missing managed leaf"))?;
    for component in parents {
        chain.push(checked_directory_at(chain.last().unwrap(), component)?);
    }
    Ok(leaf.clone())
}

#[cfg(unix)]
fn optional_checked_regular(parent: &OwnedFd, leaf: &OsStr) -> Result<Option<OwnedFd>> {
    let descriptor = match rustix::fs::openat(
        parent,
        leaf,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    managed_metadata(&descriptor)?;
    prove_entry_spelling(parent, leaf, regular_file_identity(&descriptor)?)?;
    Ok(Some(descriptor))
}

#[cfg(unix)]
fn enumerate_managed(
    parent: &OwnedFd,
    area: ManagedUserArea,
    prefix: &str,
    output: &mut Vec<ManagedRelativeName>,
) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let parent_identity = directory_identity(parent)?;
    let mut entries = rustix::fs::Dir::read_from(parent)?;
    for entry in &mut entries {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let leaf = OsStr::from_bytes(bytes);
        let name = std::str::from_utf8(bytes)?;
        let stat = rustix::fs::statat(parent, leaf, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
        let kind = rustix::fs::FileType::from_raw_mode(stat.st_mode);
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        let relative = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let canonical = ManagedRelativeName::try_from(relative.as_str())?;
        let identity = ObjectIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
        };
        ensure!(
            entry.ino() == identity.inode && identity.inode != 0,
            "enumeration identity is unprovable"
        );
        if ManagedFileKey::new(area, &relative).is_err() {
            // The excluded fixed subarea must still be a canonical directory.
            ensure!(kind.is_dir(), "reserved managed subarea is not a directory");
            checked_directory_at(parent, leaf)?;
            continue;
        }
        if kind.is_dir() {
            let child = checked_directory_at(parent, leaf)?;
            ensure!(
                directory_identity(&child)? == identity,
                "enumerated directory changed"
            );
            enumerate_managed(&child, area, &relative, output)?;
            let current = checked_directory_at(parent, leaf)?;
            ensure!(
                directory_identity(&current)? == identity,
                "enumerated directory detached"
            );
        } else {
            let file = optional_checked_regular(parent, leaf)?
                .ok_or_else(|| anyhow!("enumerated file disappeared"))?;
            ensure!(
                regular_file_identity(&file)? == identity,
                "enumerated file changed"
            );
            output.push(canonical);
        }
    }
    ensure!(
        directory_identity(parent)? == parent_identity,
        "enumerated parent changed"
    );
    Ok(())
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
    #[cfg(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))]
    {
        rustix::fs::renameat_with(
            source_parent,
            source_leaf,
            destination_parent,
            destination_leaf,
            rustix::fs::RenameFlags::NOREPLACE,
        )
    }
    #[cfg(not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    )))]
    {
        let _ = (
            source_parent,
            source_leaf,
            destination_parent,
            destination_leaf,
        );
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
    ensure!(
        file_type.is_dir(),
        "filesystem capability is not a directory"
    );
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
            _ => {
                return Err(anyhow!(
                    "application data root contains a non-normal component"
                ))
            }
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
    prove_entry_spelling(parent, component, directory_identity(&descriptor)?)?;
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
            DirectoryWalk::ExistingOnly => checked_directory_at(&current, OsStr::new(component))?,
            DirectoryWalk::CreateMissing => ensure_directory_at(&current, OsStr::new(component))?,
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
fn open_relative_parent(base: &OwnedFd, name: &ManagedRelativeName) -> Result<(OwnedFd, OsString)> {
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
    pub(crate) fn with_regular_reader<T>(
        &self, _directory: &ManagedDirectoryCapability, _file: &mut ManagedFileCapability,
        _operation: impl FnOnce(&mut dyn ManagedReadSeek) -> Result<T>,
    ) -> Result<T> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn open_optional_regular(
        &self,
        _directory: &ManagedDirectoryCapability,
        _name: &ManagedRelativeName,
    ) -> Result<Option<ManagedFileCapability>> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn read_regular_to(
        &self,
        _directory: &ManagedDirectoryCapability,
        _file: &mut ManagedFileCapability,
        _sink: &mut dyn std::io::Write,
    ) -> Result<u64> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn write_new_regular_from(
        &self,
        _directory: &ManagedDirectoryCapability,
        _file: &mut ManagedFileCapability,
        _source: &mut dyn std::io::Read,
    ) -> Result<u64> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn sync_regular(
        &self,
        _directory: &ManagedDirectoryCapability,
        _file: &mut ManagedFileCapability,
    ) -> Result<()> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn inspect_regular(
        &self,
        _directory: &ManagedDirectoryCapability,
        _file: &ManagedFileCapability,
    ) -> Result<ManagedFileMetadata> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn enumerate_regular_names(
        &self,
        _directory: &ManagedDirectoryCapability,
    ) -> Result<Vec<ManagedRelativeName>> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn with_current_regular_files<T>(
        &self,
        _files: &[ManagedFileCheck<'_>],
        _operation: impl FnOnce(&[ManagedFileMetadata]) -> Result<T>,
    ) -> Result<T> {
        unsupported_namespace_capabilities()
    }
    pub(crate) fn publish_regular(
        &self,
        _directory: &ManagedDirectoryCapability,
        _source: &mut ManagedFileCapability,
        _destination: ManagedPublication<'_>,
    ) -> Result<()> {
        unsupported_namespace_capabilities()
    }

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

    #[cfg(any(unix, windows))]
    fn managed_fixture(
        area: ManagedUserArea,
    ) -> (
        tempfile::TempDir,
        UserNamespace,
        NamespaceFs,
        ManagedDirectoryCapability,
    ) {
        let root = temporary_directory();
        let ns = UserNamespace::new(root.path(), USER_A).unwrap();
        let fs =
            NamespaceFs::for_namespace(&NamespaceFs::open_data_root(root.path()).unwrap(), &ns)
                .unwrap();
        let dirs = fs.ensure_managed_dirs().unwrap();
        let dir = fs.open_managed_dir(&dirs, area).unwrap();
        (root, ns, fs, dir)
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_entry_spelling_is_exact_without_case_folding() {
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Output);
        fs::write(ns.output_dir().join("Photo.png"), b"original").unwrap();
        let exact = ManagedRelativeName::try_from("Photo.png").unwrap();
        let file = fs.open_existing_regular(&dir, &exact).unwrap();
        assert!(fs.inspect_regular(&dir, &file).is_ok());
        let alias = ManagedRelativeName::try_from("photo.png").unwrap();
        if ns.output_dir().join("photo.png").exists() {
            assert!(fs.open_existing_regular(&dir, &alias).is_err());
            assert!(fs.open_optional_regular(&dir, &alias).is_err());
        } else {
            fs::write(ns.output_dir().join("photo.png"), b"distinct").unwrap();
            let other = fs.open_existing_regular(&dir, &alias).unwrap();
            assert_ne!(
                fs.inspect_regular(&dir, &file).unwrap().identity,
                fs.inspect_regular(&dir, &other).unwrap().identity
            );
        }
        assert_eq!(
            fs::read(ns.output_dir().join("Photo.png")).unwrap(),
            b"original"
        );
        fs::create_dir(ns.output_dir().join("Nested")).unwrap();
        fs::write(ns.output_dir().join("Nested/file"), b"nested").unwrap();
        if ns.output_dir().join("nested").exists() {
            assert!(fs
                .open_optional_regular(&dir, &ManagedRelativeName::try_from("nested/file").unwrap())
                .is_err());
            assert!(fs
                .create_new_regular(&dir, &ManagedRelativeName::try_from("nested/new").unwrap())
                .is_err());
        }
        fs::write(ns.output_dir().join("caf\u{e9}"), b"unicode").unwrap();
        let listed = fs::read_dir(ns.output_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .find(|n| n.starts_with("caf"))
            .unwrap();
        fs.open_existing_regular(
            &dir,
            &ManagedRelativeName::try_from(listed.as_str()).unwrap(),
        )
        .unwrap();
        let alternate = if listed == "caf\u{e9}" {
            "cafe\u{301}"
        } else {
            "caf\u{e9}"
        };
        if ns.output_dir().join(alternate).exists() {
            assert!(fs
                .open_optional_regular(&dir, &ManagedRelativeName::try_from(alternate).unwrap())
                .is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn managed_area_alias_is_rejected_before_directory_preparation() {
        let (_root, ns, fs, _dir) = managed_fixture(ManagedUserArea::Output);
        fs::rename(ns.output_dir(), ns.root().join("OUT")).unwrap();
        if ns.output_dir().exists() {
            assert!(fs.ensure_managed_dirs().is_err());
        }
        assert!(ns.root().join("OUT").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn managed_metadata_preserves_platform_identity() {
        use std::os::unix::fs::MetadataExt;
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Output);
        let path = ns.output_dir().join("file");
        fs::write(&path, b"metadata").unwrap();
        let file = fs
            .open_existing_regular(&dir, &ManagedRelativeName::try_from("file").unwrap())
            .unwrap();
        let actual = fs::metadata(&path).unwrap();
        let info = fs.inspect_regular(&dir, &file).unwrap();
        assert_eq!(
            info.identity,
            StableFileIdentity::Unix {
                device: actual.dev(),
                inode: actual.ino()
            }
        );
        assert_eq!(info.byte_size, 8);
        assert_eq!(info.modified_at, actual.modified().unwrap());
        assert_eq!(info.link_count, 1);
        fs::hard_link(&path, ns.output_dir().join("link")).unwrap();
        assert!(fs.inspect_regular(&dir, &file).is_err());
        assert!(fs
            .open_optional_regular(&dir, &ManagedRelativeName::try_from("link").unwrap())
            .is_err());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_streams_require_owned_new_file_and_sync_before_publication() {
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let name = ManagedRelativeName::try_from("temporary").unwrap();
        let target = ManagedRelativeName::try_from("document").unwrap();
        assert!(fs.open_optional_regular(&dir, &target).unwrap().is_none());
        let mut file = fs.create_new_regular(&dir, &name).unwrap();
        assert!(fs
            .publish_regular(&dir, &mut file, ManagedPublication::Absent(&target))
            .is_err());
        assert_eq!(
            fs.write_new_regular_from(&dir, &mut file, &mut &b"content"[..])
                .unwrap(),
            7
        );
        assert!(fs
            .publish_regular(&dir, &mut file, ManagedPublication::Absent(&target))
            .is_err());
        fs.sync_regular(&dir, &mut file).unwrap();
        fs.publish_regular(&dir, &mut file, ManagedPublication::Absent(&target))
            .unwrap();
        assert!(fs
            .write_new_regular_from(&dir, &mut file, &mut &b"overwrite"[..])
            .is_err());
        for _ in 0..2 {
            let mut bytes = Vec::new();
            assert_eq!(fs.read_regular_to(&dir, &mut file, &mut bytes).unwrap(), 7);
            assert_eq!(bytes, b"content");
        }
        let mut existing = fs.open_existing_regular(&dir, &target).unwrap();
        assert!(fs
            .write_new_regular_from(&dir, &mut existing, &mut &b"overwrite"[..])
            .is_err());
        struct ShortSink(Vec<u8>);
        impl std::io::Write for ShortSink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.push(bytes[0]);
                Ok(1)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut short = ShortSink(Vec::new());
        assert_eq!(
            fs.read_regular_to(&dir, &mut existing, &mut short).unwrap(),
            7
        );
        assert_eq!(short.0, b"content");
        struct FailedSink(Vec<u8>);
        impl std::io::Write for FailedSink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    self.0.push(bytes[0]);
                    Ok(1)
                } else {
                    Err(std::io::Error::other("sink failed"))
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut failed = FailedSink(Vec::new());
        assert!(fs
            .read_regular_to(&dir, &mut existing, &mut failed)
            .is_err());
        assert_eq!(failed.0, b"c");
        let mut reread = Vec::new();
        fs.read_regular_to(&dir, &mut existing, &mut reread)
            .unwrap();
        assert_eq!(reread, b"content");
        let mut poisoned = fs.create_new_regular(&dir, &name).unwrap();
        struct Fails(bool);
        impl std::io::Read for Fails {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    Err(std::io::Error::other("source failed"))
                } else {
                    self.0 = true;
                    buf[0] = 1;
                    Ok(1)
                }
            }
        }
        assert!(fs
            .write_new_regular_from(&dir, &mut poisoned, &mut Fails(false))
            .is_err());
        assert!(fs.sync_regular(&dir, &mut poisoned).is_err());
        assert!(fs
            .publish_regular(&dir, &mut poisoned, ManagedPublication::Replace(&existing))
            .is_err());
        fs.unlink_within(&dir, poisoned).unwrap();
        assert_eq!(
            fs::read(ns.recovery_dir().join("document")).unwrap(),
            b"content"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_chunked_source_read_releases_namespace_mutation_lock() {
        use std::sync::mpsc;
        use std::time::Duration;

        struct WaitForIndependentOperation {
            started: Option<mpsc::Sender<()>>,
            completed: mpsc::Receiver<()>,
            yielded: bool,
        }
        impl std::io::Read for WaitForIndependentOperation {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                if self.yielded {
                    return Ok(0);
                }
                self.started
                    .take()
                    .unwrap()
                    .send(())
                    .map_err(|_| std::io::Error::other("fixture control stopped"))?;
                self.completed.recv_timeout(Duration::from_secs(3)).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "namespace lock held during source read",
                    )
                })?;
                bytes[..5].copy_from_slice(b"owned");
                self.yielded = true;
                Ok(5)
            }
        }

        let (root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        fs::write(ns.recovery_dir().join("sentinel"), b"sentinel").unwrap();
        let independent =
            NamespaceFs::for_namespace(&NamespaceFs::open_data_root(root.path()).unwrap(), &ns)
                .unwrap();
        let independent_dirs = independent.ensure_managed_dirs().unwrap();
        let independent_dir = independent
            .open_managed_dir(&independent_dirs, ManagedUserArea::Recovery)
            .unwrap();
        let sentinel = independent
            .open_existing_regular(
                &independent_dir,
                &ManagedRelativeName::try_from("sentinel").unwrap(),
            )
            .unwrap();
        let mut temporary = fs
            .create_new_regular(
                &dir,
                &ManagedRelativeName::try_from("temporary").unwrap(),
            )
            .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();

        let (write_result, control_result) = std::thread::scope(|scope| {
            let control = scope.spawn(move || -> Result<()> {
                started_rx.recv_timeout(Duration::from_secs(3)).map_err(|_| {
                    anyhow::anyhow!("source read did not start before fixture timeout")
                })?;
                let metadata = independent.inspect_regular(&independent_dir, &sentinel)?;
                ensure!(metadata.byte_size == 8, "sentinel metadata changed");
                completed_tx
                    .send(())
                    .map_err(|_| anyhow::anyhow!("source stopped before control completion"))?;
                Ok(())
            });
            let mut source = WaitForIndependentOperation {
                started: Some(started_tx),
                completed: completed_rx,
                yielded: false,
            };
            let write_result = fs.write_new_regular_from(&dir, &mut temporary, &mut source);
            let control_result = control.join().expect("control thread panicked");
            (write_result, control_result)
        });

        control_result.unwrap();
        assert_eq!(write_result.unwrap(), 5);
        let destination = ManagedRelativeName::try_from("document").unwrap();
        assert!(fs
            .publish_regular(
                &dir,
                &mut temporary,
                ManagedPublication::Absent(&destination)
            )
            .is_err());
        fs.sync_regular(&dir, &mut temporary).unwrap();
        fs.publish_regular(
            &dir,
            &mut temporary,
            ManagedPublication::Absent(&destination),
        )
        .unwrap();
        assert_eq!(fs::read(ns.recovery_dir().join("document")).unwrap(), b"owned");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_chunked_short_reads_and_interrupted_read_copy_exact_bounded_bytes() {
        struct ChunkedSource {
            bytes: Vec<u8>,
            offset: usize,
            next_size: usize,
            interrupted: bool,
            largest_buffer: usize,
        }
        impl std::io::Read for ChunkedSource {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.largest_buffer = self.largest_buffer.max(buffer.len());
                if buffer.len() > 64 * 1024 {
                    return Err(std::io::Error::other("source buffer exceeded 64 KiB"));
                }
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                if self.offset == self.bytes.len() {
                    return Ok(0);
                }
                let sizes = [7, 64 * 1024, 19, 32 * 1024];
                let count = sizes[self.next_size % sizes.len()]
                    .min(buffer.len())
                    .min(self.bytes.len() - self.offset);
                buffer[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
                self.offset += count;
                self.next_size += 1;
                Ok(count)
            }
        }
        struct NeverRead;
        impl std::io::Read for NeverRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("successful managed write retried its source")
            }
        }

        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let expected = (0..(2 * 64 * 1024 + 137))
            .map(|index| ((index * 37 + 11) % 251) as u8)
            .collect::<Vec<_>>();
        let mut source = ChunkedSource {
            bytes: expected.clone(),
            offset: 0,
            next_size: 0,
            interrupted: false,
            largest_buffer: 0,
        };
        let mut temporary = fs
            .create_new_regular(
                &dir,
                &ManagedRelativeName::try_from("temporary").unwrap(),
            )
            .unwrap();
        assert_eq!(
            fs.write_new_regular_from(&dir, &mut temporary, &mut source)
                .unwrap(),
            expected.len() as u64
        );
        assert!(source.interrupted);
        assert!(source.next_size >= 4);
        assert!(source.largest_buffer <= 64 * 1024);
        assert_eq!(fs::read(ns.recovery_dir().join("temporary")).unwrap(), expected);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fs.write_new_regular_from(&dir, &mut temporary, &mut NeverRead)
        }))
        .unwrap()
        .is_err());
        let destination = ManagedRelativeName::try_from("document").unwrap();
        assert!(fs
            .publish_regular(
                &dir,
                &mut temporary,
                ManagedPublication::Absent(&destination)
            )
            .is_err());
        fs.sync_regular(&dir, &mut temporary).unwrap();
        fs.publish_regular(
            &dir,
            &mut temporary,
            ManagedPublication::Absent(&destination),
        )
        .unwrap();
        assert_eq!(fs::read(ns.recovery_dir().join("document")).unwrap(), expected);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_chunked_source_errors_poison_empty_and_partial_attempts() {
        struct FailsAfterPrefix {
            prefix: &'static [u8],
            delivered: bool,
        }
        impl std::io::Read for FailsAfterPrefix {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if !self.delivered && !self.prefix.is_empty() {
                    buffer[..self.prefix.len()].copy_from_slice(self.prefix);
                    self.delivered = true;
                    return Ok(self.prefix.len());
                }
                Err(std::io::Error::other("controlled source failure"))
            }
        }

        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        fs::write(ns.recovery_dir().join("document"), b"existing").unwrap();
        let existing = fs
            .open_existing_regular(
                &dir,
                &ManagedRelativeName::try_from("document").unwrap(),
            )
            .unwrap();
        for (name, prefix) in [("empty-failure", &b""[..]), ("partial-failure", &b"prefix"[..])]
        {
            let relative = ManagedRelativeName::try_from(name).unwrap();
            let mut temporary = fs.create_new_regular(&dir, &relative).unwrap();
            assert!(fs
                .write_new_regular_from(
                    &dir,
                    &mut temporary,
                    &mut FailsAfterPrefix {
                        prefix,
                        delivered: false,
                    },
                )
                .is_err());
            assert_eq!(fs::read(ns.recovery_dir().join(name)).unwrap(), prefix);
            assert!(fs
                .write_new_regular_from(&dir, &mut temporary, &mut &b"retry"[..])
                .is_err());
            assert!(fs.sync_regular(&dir, &mut temporary).is_err());
            assert!(fs
                .publish_regular(
                    &dir,
                    &mut temporary,
                    ManagedPublication::Replace(&existing)
                )
                .is_err());
            assert_eq!(fs::read(ns.recovery_dir().join("document")).unwrap(), b"existing");
            fs.unlink_within(&dir, temporary).unwrap();
            assert!(!ns.recovery_dir().join(name).exists());
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_chunked_source_panic_after_prefix_poison_attempts() {
        struct PanicsAfterPrefix(bool);
        impl std::io::Read for PanicsAfterPrefix {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    buffer[..6].copy_from_slice(b"prefix");
                    Ok(6)
                } else {
                    panic!("controlled source panic")
                }
            }
        }

        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let name = ManagedRelativeName::try_from("temporary").unwrap();
        let mut temporary = fs.create_new_regular(&dir, &name).unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fs.write_new_regular_from(&dir, &mut temporary, &mut PanicsAfterPrefix(false))
        }));
        assert!(panic.is_err());
        assert_eq!(fs::read(ns.recovery_dir().join("temporary")).unwrap(), b"prefix");
        assert!(fs
            .write_new_regular_from(&dir, &mut temporary, &mut &b"retry"[..])
            .is_err());
        assert!(fs.sync_regular(&dir, &mut temporary).is_err());
        assert!(fs
            .publish_regular(
                &dir,
                &mut temporary,
                ManagedPublication::Absent(&ManagedRelativeName::try_from("document").unwrap())
            )
            .is_err());
        fs.unlink_within(&dir, temporary).unwrap();
        assert!(!ns.recovery_dir().join("temporary").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_chunked_leaf_replacement_between_chunks_stops_before_next_write() {
        struct ReplacesLeaf {
            temporary: PathBuf,
            moved: PathBuf,
            step: u8,
        }
        impl std::io::Read for ReplacesLeaf {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                match self.step {
                    0 => {
                        buffer[..6].copy_from_slice(b"prefix");
                        self.step = 1;
                        Ok(6)
                    }
                    1 => {
                        fs::rename(&self.temporary, &self.moved)?;
                        fs::write(&self.temporary, b"replacement")?;
                        buffer[..6].copy_from_slice(b"suffix");
                        self.step = 2;
                        Ok(6)
                    }
                    _ => Ok(0),
                }
            }
        }

        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let temporary_path = ns.recovery_dir().join("temporary");
        let moved_path = ns.recovery_dir().join("moved");
        let mut temporary = fs
            .create_new_regular(
                &dir,
                &ManagedRelativeName::try_from("temporary").unwrap(),
            )
            .unwrap();
        let result = fs.write_new_regular_from(
            &dir,
            &mut temporary,
            &mut ReplacesLeaf {
                temporary: temporary_path.clone(),
                moved: moved_path.clone(),
                step: 0,
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&moved_path).unwrap(), b"prefix");
        assert_eq!(fs::read(&temporary_path).unwrap(), b"replacement");
        fs::remove_file(&temporary_path).unwrap();
        fs::rename(&moved_path, &temporary_path).unwrap();
        assert!(fs
            .write_new_regular_from(&dir, &mut temporary, &mut &b"retry"[..])
            .is_err());
        assert!(fs.sync_regular(&dir, &mut temporary).is_err());
        assert!(fs
            .publish_regular(
                &dir,
                &mut temporary,
                ManagedPublication::Absent(&ManagedRelativeName::try_from("document").unwrap())
            )
            .is_err());
        fs.unlink_within(&dir, temporary).unwrap();
        assert!(!temporary_path.exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_chunked_leaf_replacement_at_eof_refuses_success() {
        struct ReplacesLeafAtEof {
            temporary: PathBuf,
            moved: PathBuf,
            yielded: bool,
        }
        impl std::io::Read for ReplacesLeafAtEof {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if !self.yielded {
                    buffer[..6].copy_from_slice(b"prefix");
                    self.yielded = true;
                    return Ok(6);
                }
                fs::rename(&self.temporary, &self.moved)?;
                fs::write(&self.temporary, b"replacement")?;
                Ok(0)
            }
        }

        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let temporary_path = ns.recovery_dir().join("temporary");
        let moved_path = ns.recovery_dir().join("moved");
        let mut temporary = fs
            .create_new_regular(
                &dir,
                &ManagedRelativeName::try_from("temporary").unwrap(),
            )
            .unwrap();
        assert!(fs
            .write_new_regular_from(
                &dir,
                &mut temporary,
                &mut ReplacesLeafAtEof {
                    temporary: temporary_path.clone(),
                    moved: moved_path.clone(),
                    yielded: false,
                },
            )
            .is_err());
        assert_eq!(fs::read(&moved_path).unwrap(), b"prefix");
        assert_eq!(fs::read(&temporary_path).unwrap(), b"replacement");
        fs::remove_file(&temporary_path).unwrap();
        fs::rename(&moved_path, &temporary_path).unwrap();
        assert!(fs
            .write_new_regular_from(&dir, &mut temporary, &mut &b"retry"[..])
            .is_err());
        assert!(fs.sync_regular(&dir, &mut temporary).is_err());
        assert!(fs
            .publish_regular(
                &dir,
                &mut temporary,
                ManagedPublication::Absent(&ManagedRelativeName::try_from("document").unwrap())
            )
            .is_err());
        fs.unlink_within(&dir, temporary).unwrap();
        assert!(!temporary_path.exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_chunked_ancestor_detachment_between_chunks_stops_before_next_write() {
        struct DetachesArea {
            area: PathBuf,
            detached: PathBuf,
            step: u8,
        }
        impl std::io::Read for DetachesArea {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                match self.step {
                    0 => {
                        buffer[..6].copy_from_slice(b"prefix");
                        self.step = 1;
                        Ok(6)
                    }
                    1 => {
                        fs::rename(&self.area, &self.detached)?;
                        fs::create_dir(&self.area)?;
                        buffer[..6].copy_from_slice(b"suffix");
                        self.step = 2;
                        Ok(6)
                    }
                    _ => Ok(0),
                }
            }
        }

        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let area_path = ns.recovery_dir();
        let detached_path = ns.root().join("detached-recovery");
        let mut temporary = fs
            .create_new_regular(
                &dir,
                &ManagedRelativeName::try_from("temporary").unwrap(),
            )
            .unwrap();
        let result = fs.write_new_regular_from(
            &dir,
            &mut temporary,
            &mut DetachesArea {
                area: area_path.clone(),
                detached: detached_path.clone(),
                step: 0,
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read(detached_path.join("temporary")).unwrap(), b"prefix");
        assert!(!area_path.join("temporary").exists());
        fs::remove_dir(&area_path).unwrap();
        fs::rename(&detached_path, &area_path).unwrap();
        assert!(fs
            .write_new_regular_from(&dir, &mut temporary, &mut &b"retry"[..])
            .is_err());
        assert!(fs.sync_regular(&dir, &mut temporary).is_err());
        assert!(fs
            .publish_regular(
                &dir,
                &mut temporary,
                ManagedPublication::Absent(&ManagedRelativeName::try_from("document").unwrap())
            )
            .is_err());
        fs.unlink_within(&dir, temporary).unwrap();
        assert!(!area_path.join("temporary").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_enumeration_stays_in_canonical_area() {
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Canvas);
        fs::create_dir(ns.canvas_dir().join("nested")).unwrap();
        fs::write(ns.canvas_dir().join("nested/b"), b"b").unwrap();
        fs::write(ns.canvas_dir().join("a"), b"a").unwrap();
        fs::write(ns.canvas_dir().join("uploads/private"), b"excluded").unwrap();
        #[cfg(unix)]
        symlink(ns.canvas_dir().join("nested"), ns.canvas_dir().join("link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(
            ns.canvas_dir().join("nested"),
            ns.canvas_dir().join("link"),
        )
        .unwrap();
        let names = fs.enumerate_regular_names(&dir).unwrap();
        assert_eq!(
            names.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            ["a", "nested/b"]
        );
        for name in names {
            fs.open_optional_regular(&dir, &name).unwrap().unwrap();
        }
        #[cfg(unix)]
        {
            fs::write(ns.canvas_dir().join("bad."), b"untouched").unwrap();
            assert!(fs.enumerate_regular_names(&dir).is_err());
            assert_eq!(
                fs::read(ns.canvas_dir().join("bad.")).unwrap(),
                b"untouched"
            );
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_validation_holds_all_opened_bindings_through_callback() {
        let (_root, _ns, fs, dir) = managed_fixture(ManagedUserArea::Output);
        let one = fs
            .create_new_regular(&dir, &ManagedRelativeName::try_from("one").unwrap())
            .unwrap();
        let two = fs
            .create_new_regular(&dir, &ManagedRelativeName::try_from("two").unwrap())
            .unwrap();
        let checks = [
            ManagedFileCheck {
                directory: &dir,
                file: &one,
                expected: None,
            },
            ManagedFileCheck {
                directory: &dir,
                file: &two,
                expected: None,
            },
        ];
        assert_eq!(
            fs.with_current_regular_files(&checks, |info| {
                assert_eq!(info.len(), 2);
                assert_ne!(info[0].identity, info[1].identity);
                Ok(17)
            })
            .unwrap(),
            17
        );
        assert!(fs.with_current_regular_files(&[], |_| Ok(())).is_err());
        let wrong = [ManagedFileCheck {
            directory: &dir,
            file: &one,
            expected: Some(StableFileIdentity::Windows {
                volume: 0,
                file_id: [0; 16],
            }),
        }];
        assert!(fs
            .with_current_regular_files::<()>(&wrong, |_| panic!("invalid identity callback"))
            .is_err());
        let one_id = fs.inspect_regular(&dir, &one).unwrap().identity;
        assert!(fs
            .with_current_regular_files::<()>(
                &[ManagedFileCheck {
                    directory: &dir,
                    file: &two,
                    expected: Some(one_id)
                }],
                |_| panic!("wrong same-area file callback")
            )
            .is_err());
        let operation_error = fs
            .with_current_regular_files::<()>(&checks, |_| {
                Err(std::io::Error::other("SQL failure").into())
            })
            .unwrap_err();
        assert!(operation_error.downcast_ref::<std::io::Error>().is_some());
        assert!(operation_error
            .downcast_ref::<ManagedPublicationConflict>()
            .is_none());
        use std::sync::mpsc;
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            fs.with_current_regular_files(&checks, |_| {
                scope.spawn(|| {
                    started_tx.send(()).unwrap();
                    fs.create_new_regular(&dir, &ManagedRelativeName::try_from("mutator").unwrap())
                        .unwrap();
                    done_tx.send(()).unwrap();
                });
                started_rx.recv().unwrap();
                assert!(done_rx
                    .recv_timeout(std::time::Duration::from_millis(100))
                    .is_err());
                Ok(())
            })
            .unwrap();
            done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        });
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_publication_conflicts_preserve_owned_temporary() {
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let name = ManagedRelativeName::try_from("temp").unwrap();
        let target = ManagedRelativeName::try_from("document").unwrap();
        let mut source = fs.create_new_regular(&dir, &name).unwrap();
        fs.write_new_regular_from(&dir, &mut source, &mut &b"new"[..])
            .unwrap();
        fs.sync_regular(&dir, &mut source).unwrap();
        fs::write(ns.recovery_dir().join("document"), b"winner").unwrap();
        let error = fs
            .publish_regular(&dir, &mut source, ManagedPublication::Absent(&target))
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<ManagedPublicationConflict>(),
            Some(&ManagedPublicationConflict::DestinationAppeared)
        );
        let stale = fs.open_existing_regular(&dir, &target).unwrap();
        fs::rename(
            ns.recovery_dir().join("document"),
            ns.recovery_dir().join("old"),
        )
        .unwrap();
        fs::write(ns.recovery_dir().join("document"), b"new winner").unwrap();
        let error = fs
            .publish_regular(&dir, &mut source, ManagedPublication::Replace(&stale))
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<ManagedPublicationConflict>(),
            Some(&ManagedPublicationConflict::DestinationChanged)
        );
        fs.unlink_within(&dir, source).unwrap();
        assert_eq!(
            fs::read(ns.recovery_dir().join("document")).unwrap(),
            b"new winner"
        );
        assert!(!ns.recovery_dir().join("temp").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_publication_rejects_invalid_targets_and_source_without_conflict() {
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let temp = ManagedRelativeName::try_from("temporary").unwrap();
        let target = ManagedRelativeName::try_from("document").unwrap();
        let mut source = fs.create_new_regular(&dir, &temp).unwrap();
        fs.write_new_regular_from(&dir, &mut source, &mut &b"new"[..])
            .unwrap();
        fs.sync_regular(&dir, &mut source).unwrap();
        fs::create_dir(ns.recovery_dir().join("document")).unwrap();
        let error = fs
            .publish_regular(&dir, &mut source, ManagedPublication::Absent(&target))
            .unwrap_err();
        assert!(error.downcast_ref::<ManagedPublicationConflict>().is_none());
        fs::remove_dir(ns.recovery_dir().join("document")).unwrap();
        fs::write(ns.recovery_dir().join("document"), b"old").unwrap();
        let destination = fs.open_existing_regular(&dir, &target).unwrap();
        fs.publish_regular(&dir, &mut source, ManagedPublication::Replace(&destination))
            .unwrap();
        assert_eq!(
            fs::read(ns.recovery_dir().join("document")).unwrap(),
            b"new"
        );
        assert!(fs
            .write_new_regular_from(&dir, &mut source, &mut &b"bad"[..])
            .is_err());
        fs.unlink_within(&dir, source).unwrap();
        let mut source = fs.create_new_regular(&dir, &temp).unwrap();
        fs.write_new_regular_from(&dir, &mut source, &mut &b"new"[..])
            .unwrap();
        fs.sync_regular(&dir, &mut source).unwrap();
        fs::rename(
            ns.recovery_dir().join("temporary"),
            ns.recovery_dir().join("moved"),
        )
        .unwrap();
        fs::write(ns.recovery_dir().join("temporary"), b"foreign").unwrap();
        let error = fs
            .publish_regular(&dir, &mut source, ManagedPublication::Absent(&target))
            .unwrap_err();
        assert!(error.downcast_ref::<ManagedPublicationConflict>().is_none());
        assert!(fs.unlink_within(&dir, source).is_err());
        assert_eq!(
            fs::read(ns.recovery_dir().join("temporary")).unwrap(),
            b"foreign"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn managed_checks_reject_foreign_authority_missing_parent_and_detached_area() {
        let (root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let name = ManagedRelativeName::try_from("file").unwrap();
        let file = fs.create_new_regular(&dir, &name).unwrap();
        let foreign =
            NamespaceFs::for_namespace(&NamespaceFs::open_data_root(root.path()).unwrap(), &ns)
                .unwrap();
        assert!(foreign.inspect_regular(&dir, &file).is_err());
        assert!(foreign
            .with_current_regular_files::<()>(
                &[ManagedFileCheck {
                    directory: &dir,
                    file: &file,
                    expected: None
                }],
                |_| panic!("foreign callback")
            )
            .is_err());
        assert!(fs
            .open_optional_regular(
                &dir,
                &ManagedRelativeName::try_from("missing/leaf").unwrap()
            )
            .is_err());
        fs::rename(ns.recovery_dir(), ns.root().join("detached")).unwrap();
        fs::create_dir(ns.recovery_dir()).unwrap();
        assert!(fs.open_optional_regular(&dir, &name).is_err());
        assert!(fs.enumerate_regular_names(&dir).is_err());
        assert!(fs
            .with_current_regular_files::<()>(
                &[ManagedFileCheck {
                    directory: &dir,
                    file: &file,
                    expected: None
                }],
                |_| panic!("detached callback")
            )
            .is_err());
        assert!(fs.unlink_within(&dir, file).is_err());
        assert!(ns.root().join("detached/file").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn managed_enumeration_rejects_undecodable_entries_and_hardlinks() {
        use std::os::unix::ffi::OsStringExt;
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Output);
        fs::write(ns.output_dir().join("first"), b"bytes").unwrap();
        let invalid = ns.output_dir().join(OsString::from_vec(vec![255]));
        match fs::write(&invalid, b"untouched") {
            Ok(()) => {
                assert!(fs.enumerate_regular_names(&dir).is_err());
                fs::remove_file(&invalid).unwrap();
            }
            Err(error) => {
                assert_eq!(
                    error.raw_os_error(),
                    Some(rustix::io::Errno::ILSEQ.raw_os_error())
                );
                eprintln!("fixture filesystem rejects undecodable filenames: {error}");
            }
        }
        fs::hard_link(
            ns.output_dir().join("first"),
            ns.output_dir().join("second"),
        )
        .unwrap();
        assert!(fs.enumerate_regular_names(&dir).is_err());
        assert_eq!(fs::read(ns.output_dir().join("first")).unwrap(), b"bytes");
    }

    #[cfg(unix)]
    #[test]
    fn managed_injected_sync_failure_poisons_publication_and_retains_cleanup() {
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let mut file = fs
            .create_new_regular(&dir, &ManagedRelativeName::try_from("temp").unwrap())
            .unwrap();
        fs.write_new_regular_from(&dir, &mut file, &mut &b"bytes"[..])
            .unwrap();
        assert!(fs
            .sync_regular_with(&dir, &mut file, |_| Err(std::io::Error::other(
                "injected fsync error"
            )
            .into()))
            .is_err());
        let error = fs
            .publish_regular(
                &dir,
                &mut file,
                ManagedPublication::Absent(&ManagedRelativeName::try_from("document").unwrap()),
            )
            .unwrap_err();
        assert!(error.downcast_ref::<ManagedPublicationConflict>().is_none());
        assert!(fs.sync_regular(&dir, &mut file).is_err());
        fs.unlink_within(&dir, file).unwrap();
        assert!(!ns.recovery_dir().join("temp").exists());
        assert!(!ns.recovery_dir().join("document").exists());
    }

    #[cfg(unix)]
    #[test]
    fn managed_replacement_identity_failure_is_not_a_conflict() {
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let mut source = fs
            .create_new_regular(&dir, &ManagedRelativeName::try_from("temp").unwrap())
            .unwrap();
        fs.write_new_regular_from(&dir, &mut source, &mut &b"new"[..])
            .unwrap();
        fs.sync_regular(&dir, &mut source).unwrap();
        fs::write(ns.recovery_dir().join("document"), b"old").unwrap();
        let destination = fs
            .open_existing_regular(&dir, &ManagedRelativeName::try_from("document").unwrap())
            .unwrap();
        let error = fs
            .publish_regular_with_current_identity(
                &dir,
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
        assert_eq!(
            fs::read(ns.recovery_dir().join("document")).unwrap(),
            b"old"
        );
        fs::rename(
            ns.recovery_dir().join("document"),
            ns.recovery_dir().join("held"),
        )
        .unwrap();
        let missing = fs
            .publish_regular(&dir, &mut source, ManagedPublication::Replace(&destination))
            .unwrap_err();
        assert_eq!(
            missing.downcast_ref::<ManagedPublicationConflict>(),
            Some(&ManagedPublicationConflict::DestinationChanged)
        );
        fs.unlink_within(&dir, source).unwrap();
        assert_eq!(fs::read(ns.recovery_dir().join("held")).unwrap(), b"old");
        assert!(!ns.recovery_dir().join("temp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn managed_failed_creation_zero_inode_cannot_authorize_unlink() {
        let (_root, ns, fs, dir) = managed_fixture(ManagedUserArea::Recovery);
        let mut file = fs
            .create_new_regular(&dir, &ManagedRelativeName::try_from("temp").unwrap())
            .unwrap();
        fs.write_new_regular_from(&dir, &mut file, &mut &b"owned"[..])
            .unwrap();
        let _lock = fs.lock_mutations().unwrap();
        let zero = ObjectIdentity {
            device: file.identity.device,
            inode: 0,
        };
        let error = cleanup_failed_managed_creation(
            &dir.descriptor,
            OsStr::new("temp"),
            zero,
            std::io::Error::from_raw_os_error(5).into(),
            |_| Ok(zero),
        );
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
        assert_eq!(fs::read(ns.recovery_dir().join("temp")).unwrap(), b"owned");
        let error = cleanup_failed_managed_creation(
            &dir.descriptor,
            OsStr::new("temp"),
            file.identity,
            std::io::Error::from_raw_os_error(5).into(),
            regular_file_identity,
        );
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
        assert!(!ns.recovery_dir().join("temp").exists());
    }

    #[test]
    fn managed_names_reject_lexical_and_overlapping_area_aliases() {
        for name in [
            "a//b", "a/", "a/./b", "a\\b", "CON", "aux.txt", "a:stream", "a.", "a ", "", "/a",
            "../a", "a\0b",
        ] {
            assert!(
                ManagedRelativeName::try_from(name).is_err(),
                "admitted {name:?}"
            );
        }
        for (area, name) in [
            (ManagedUserArea::Canvas, "uploads/a"),
            (ManagedUserArea::Canvas, "exports"),
            (ManagedUserArea::References, "library/a"),
            (ManagedUserArea::References, "imports/a"),
        ] {
            assert!(ManagedFileKey::new(area, name).is_err());
        }
        assert!(ManagedFileKey::new(ManagedUserArea::CanvasUploads, "a").is_ok());
        assert!(ManagedFileKey::new(ManagedUserArea::ReferencesLibrary, "a").is_ok());
        assert!(ManagedFileKey::new(ManagedUserArea::Canvas, "arbitrary/a").is_ok());
        for area in MANAGED_USER_AREAS {
            assert_eq!(
                ManagedUserArea::from_storage_name(area.storage_name()).unwrap(),
                area
            );
        }
        assert!(ManagedUserArea::from_storage_name("out").is_err());
    }

    #[test]
    fn managed_metadata_preserves_platform_identity_codec() {
        let unix = StableFileIdentity::Unix {
            device: u64::MAX,
            inode: 0x8000_0000_0000_0001,
        };
        assert_eq!(
            unix.to_storage_bytes(),
            vec![1, 255, 255, 255, 255, 255, 255, 255, 255, 128, 0, 0, 0, 0, 0, 0, 1]
        );
        let windows = StableFileIdentity::Windows {
            volume: u64::MAX,
            file_id: [128; 16],
        };
        assert_eq!(
            windows.to_storage_bytes(),
            vec![
                2, 255, 255, 255, 255, 255, 255, 255, 255, 128, 128, 128, 128, 128, 128, 128, 128,
                128, 128, 128, 128, 128, 128, 128, 128
            ]
        );
        for id in [
            unix,
            windows,
            StableFileIdentity::Windows {
                volume: u64::MAX,
                file_id: [129; 16],
            },
        ] {
            assert_eq!(
                StableFileIdentity::from_storage_bytes(&id.to_storage_bytes()).unwrap(),
                id
            );
        }
        assert_ne!(
            windows,
            StableFileIdentity::Windows {
                volume: u64::MAX,
                file_id: [
                    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 129
                ]
            }
        );
        for bad in [
            vec![],
            vec![0; 17],
            vec![1; 16],
            vec![1; 18],
            vec![2; 24],
            vec![2; 26],
        ] {
            assert!(StableFileIdentity::from_storage_bytes(&bad).is_err());
        }
    }

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
        symlink(external.path(), namespace.output_dir().join("linked-file")).unwrap();
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
        let result = namespace_fs
            .rename_within_after_commit(&output, source, &destination, || {
                fs::remove_file(namespace.output_dir().join("destination.bin")).unwrap();
                symlink(
                    external.path(),
                    namespace.output_dir().join("destination.bin"),
                )
                .unwrap();
            })
            .unwrap();
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
