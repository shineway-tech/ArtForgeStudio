//! Persistent volume identities, separate from live dev/inode capabilities.
use super::*;
use std::os::fd::AsRawFd;

#[repr(C)]
struct AttributeList {
    bitmap_count: u16,
    reserved: u16,
    common: u32,
    volume: u32,
    directory: u32,
    file: u32,
    fork: u32,
}

extern "C" {
    fn fgetattrlist(fd: std::ffi::c_int, attributes: *mut AttributeList,
        buffer: *mut std::ffi::c_void, size: usize, options: std::ffi::c_uint) -> std::ffi::c_int;
}

pub(super) fn from_descriptor(descriptor: &OwnedFd, identity: ObjectIdentity) -> Result<StableFileIdentity> {
    // ATTR_VOL_INFO | ATTR_VOL_UUID, from the retained descriptor, not a pathname.
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getattrlist.2.html
    let mut attributes = AttributeList { bitmap_count: 5, reserved: 0, common: 0,
        volume: 0x8004_0000, directory: 0, file: 0, fork: 0 };
    let mut buffer = [0u8; 20];
    // SAFETY: attrlist matches the Darwin ABI; the kernel receives a writable
    // buffer of its declared length and the descriptor is held for this call.
    let result = unsafe { fgetattrlist(descriptor.as_raw_fd(), &mut attributes,
        buffer.as_mut_ptr().cast(), buffer.len(), 0) };
    if result != 0 { return Err(std::io::Error::last_os_error()).context("read persistent volume UUID"); }
    ensure!(u32::from_ne_bytes(buffer[..4].try_into()?) == 20, "invalid volume UUID response");
    let volume_uuid: [u8; 16] = buffer[4..].try_into()?;
    ensure!(volume_uuid != [0; 16], "filesystem has no persistent volume UUID");
    Ok(StableFileIdentity::MacOs { volume_uuid, inode: identity.inode })
}

fn upgrade_at(path: &Path, saved: StableFileIdentity, regular: bool, required: bool) -> Result<StableFileIdentity> {
    let StableFileIdentity::Unix { device, inode } = saved else { return Ok(saved); };
    let inspect = || -> Result<StableFileIdentity> {
        let descriptor = if regular {
            let parent = open_absolute_directory(path.parent().ok_or_else(|| anyhow!("missing file parent"))?)?;
            open_regular_at(&parent, path.file_name().ok_or_else(|| anyhow!("missing file name"))?)?
        } else { open_absolute_directory(path)? };
        let actual = if regular { regular_file_identity(&descriptor)? } else { directory_identity(&descriptor)? };
        // Old records have no volume UUID. Never guess across a changed device
        // number: that requires an independently verified repair, not adoption.
        ensure!(actual == ObjectIdentity { device, inode }, "legacy directory identity changed; verified repair required");
        if regular { ensure!(rustix::fs::fstat(&descriptor)?.st_nlink == 1, "linked ownership file"); }
        from_descriptor(&descriptor, actual)
    };
    match inspect() {
        Ok(identity) => Ok(identity),
        Err(error) if required => Err(error),
        // History is used only to rebase paths. Missing old destinations must
        // not prevent access to the current material directory.
        Err(_) => Ok(saved),
    }
}

impl UserNamespace {
    pub(crate) fn upgrade_legacy_macos_identities(&self) -> Result<Self> {
        let mut mappings = self.mappings.clone();
        for mapping in &mut mappings {
            let area = ManagedUserArea::from_storage_name(&mapping.area)?;
            let active = self.path(area) == mapping.target;
            mapping.identity = upgrade_at(&mapping.target, mapping.identity, false, active)?;
            mapping.source_identity = upgrade_at(&mapping.source, mapping.source_identity, false, false)?;
            if let Some(owner) = &mut mapping.material_owner {
                let root = mapping.target.parent().ok_or_else(|| anyhow!("material parent missing"))?;
                owner.root = upgrade_at(root, owner.root, false, active)?;
                owner.file = upgrade_at(&root.join(MATERIAL_OWNER_FILE), owner.file, true, active)?;
                if active {
                    ensure!(*owner == material_directory_owner(root, self.user_public_id())?, "material directory ownership changed");
                }
            }
            for entry in &mut mapping.pending_rebind {
                let key = ManagedFileKey::new(area, &entry.path)?;
                if let Some(after) = entry.after {
                    entry.after = Some(upgrade_at(&mapping.target.join(key.relative_name().as_str()), after, true, true)?);
                }
            }
        }
        let data_root = self.root.parent().and_then(Path::parent).ok_or_else(|| anyhow!("namespace root missing"))?;
        mappings.into_iter().try_fold(Self::new(data_root, self.user_public_id())?, |namespace, mapping| namespace.with_mapping(mapping))
    }
}

impl NamespaceStorageAuthority {
    pub(crate) fn validate_macos_upgrade_directory(&self, area: ManagedUserArea) -> Result<()> {
        self.checked_runtime_access()?;
        self.fs.open_managed_dir(&self.directories, area)?;
        Ok(())
    }
    /// Only for one-time conversion. Ordinary file reads still require the
    /// exact persistent identity; no inode-only matching is introduced.
    pub(crate) fn legacy_macos_file_identity(&self, file: &NamespaceManagedFile) -> Result<StableFileIdentity> {
        self.inspect_regular(file)?;
        let identity = file.capability.identity;
        Ok(StableFileIdentity::Unix { device: identity.device, inode: identity.inode })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_identity_survives_mount_number_change_but_distinguishes_files() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().canonicalize().unwrap();
        let descriptor = open_absolute_directory(&path).unwrap();
        let raw = directory_identity(&descriptor).unwrap();
        let original = from_descriptor(&descriptor, raw).unwrap();
        let remounted = from_descriptor(&descriptor, ObjectIdentity { device: raw.device + 1, ..raw }).unwrap();
        assert_eq!(original, remounted);
        assert_ne!(original, from_descriptor(&descriptor, ObjectIdentity { inode: raw.inode + 1, ..raw }).unwrap());
        assert_eq!(StableFileIdentity::from_storage_bytes(&original.to_storage_bytes()).unwrap(), original);
    }

    #[test]
    fn legacy_upgrade_requires_exact_object_and_rejects_symlinks() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().canonicalize().unwrap();
        let descriptor = open_absolute_directory(&path).unwrap();
        let raw = directory_identity(&descriptor).unwrap();
        let saved = StableFileIdentity::Unix { device: raw.device, inode: raw.inode };
        assert_eq!(upgrade_at(&path, saved, false, true).unwrap(), from_descriptor(&descriptor, raw).unwrap());
        let stale = StableFileIdentity::Unix { device: raw.device + 1, inode: raw.inode };
        assert!(upgrade_at(&path, stale, false, true).is_err());
        assert_eq!(upgrade_at(&path, stale, false, false).unwrap(), stale);
        let alias = path.join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(upgrade_at(&alias, saved, false, true).is_err());
    }

    #[test]
    fn renamed_legacy_private_directory_remains_protected_before_its_account_upgrades() {
        let private = tempfile::tempdir().unwrap();
        let candidate = tempfile::tempdir().unwrap();
        let path = candidate.path().canonicalize().unwrap();
        let root = NamespaceFs::open_data_root(&private.path().canonicalize().unwrap()).unwrap();
        assert!(ExternalExportDestination::open(&root, &path).is_ok());
        let descriptor = open_absolute_directory(&path).unwrap();
        let raw = directory_identity(&descriptor).unwrap();
        register_mapped_private_identity(StableFileIdentity::Unix { device: raw.device, inode: raw.inode }).unwrap();
        assert!(ExternalExportDestination::open(&root, &path).is_err());
    }

    #[test]
    fn legacy_material_bindings_upgrade_without_accepting_another_volume_or_owner() {
        use std::os::unix::fs::MetadataExt;
        let legacy = |path: &Path| {
            let metadata = path.metadata().unwrap();
            StableFileIdentity::Unix { device: metadata.dev(), inode: metadata.ino() }
        };
        let private = tempfile::tempdir().unwrap();
        let chosen = tempfile::tempdir().unwrap();
        let private_path = private.path().canonicalize().unwrap();
        let owner = "11111111-1111-4111-8111-111111111111";
        let root = Arc::new(NamespaceFs::open_data_root(&private_path).unwrap());
        let material = prepare_material_directory(&root, &chosen.path().canonicalize().unwrap(), owner).unwrap();
        let target = material.join("输入素材");
        std::fs::create_dir(&target).unwrap();
        let namespace = UserNamespace::new(&private_path, owner).unwrap();
        let source = namespace.path(ManagedUserArea::Input);
        std::fs::create_dir_all(&source).unwrap();
        let mapping = AccountDirectoryMapping { version: 3, area: "input".into(),
            identity: legacy(&target), source_identity: legacy(&source), source, target: target.clone(),
            manifest_json: "[]".into(), pending_rebind: Vec::new(),
            material_owner: Some(MaterialOwnerBinding { owner_user_id: owner.into(),
                root: legacy(&material), file: legacy(&material.join(MATERIAL_OWNER_FILE)) }) };
        let old = namespace.with_mapping(mapping).unwrap();
        let upgraded = old.upgrade_legacy_macos_identities().unwrap();
        assert_ne!(old, upgraded);
        assert_eq!(upgraded.upgrade_legacy_macos_identities().unwrap(), upgraded);
        upgraded.validate_active_material_owners().unwrap();
        let lease = NamespaceLease { namespace: upgraded.clone(), auth_epoch: 1, namespace_epoch: 1 };
        NamespaceStorageAuthority::open(root.clone(), &lease).unwrap();
        let mut wrong_volume = upgraded.clone();
        let StableFileIdentity::MacOs { volume_uuid, .. } = &mut wrong_volume.mappings[0].identity else { panic!("not upgraded") };
        volume_uuid[0] ^= 1;
        assert!(NamespaceStorageAuthority::open(root, &NamespaceLease { namespace: wrong_volume, ..lease }).is_err());
        std::fs::write(material.join(MATERIAL_OWNER_FILE), br#"{"version":1,"owner_user_id":"22222222-2222-4222-8222-222222222222"}"#).unwrap();
        assert!(upgraded.validate_active_material_owners().is_err());
        assert!(old.upgrade_legacy_macos_identities().is_err());
    }
}
