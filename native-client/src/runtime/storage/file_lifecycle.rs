use super::*;
use std::time::SystemTime;

// TEMP(team-accounts): remove these closed compile adapters in Task 10.
pub(super) fn initialize_storage_index() {}
pub(super) fn rebuild_storage_references(_store: &Store) -> bool {
    false
}
pub(super) fn cleanup_orphaned_durable_copies_at_startup() {}
pub(super) fn cleanup_orphaned_durable_copies_at_shutdown() {}
pub(super) fn indexed_reference_count(_path: &Path) -> u64 {
    u64::MAX
}
pub(super) fn remove_indexed_file(_path: &Path) {}
pub(super) fn managed_file_registration(path: &Path) -> ManagedFileRegistration {
    ManagedFileRegistration {
        path: path.to_path_buf(),
        kind: "unavailable".into(),
        byte_size: 0,
        managed: false,
        retention_policy: "external".into(),
    }
}
pub(super) fn managed_file_registration_for_namespace(
    authority: &NamespaceStorageAuthority,
    key: &ManagedFileKey,
    kind: &str,
    retention_policy: &str,
) -> FileIndexResult<NamespacedManagedFileRegistration> {
    let file = authority
        .open_existing_regular(key)
        .map_err(FileIndexError::Capability)?;
    NamespacedManagedFileRegistration::new(authority, file, kind, retention_policy)
}
// Candidates are metadata only: recovery references have not been confirmed.
pub(super) fn discover_orphan_candidates_for_namespace(
    index: &FileIndex,
    authority: &NamespaceStorageAuthority,
    minimum_age: Duration,
) -> FileIndexResult<Vec<OrphanCandidate>> {
    let now = SystemTime::now();
    let cutoff = now
        .checked_sub(minimum_age)
        .ok_or_else(|| FileIndexError::InvalidValue("orphan age exceeds SystemTime".into()))?;
    let mut candidates = Vec::new();
    for area in [
        ManagedUserArea::CanvasUploads,
        ManagedUserArea::ReferencesLibrary,
    ] {
        let names = authority
            .enumerate_regular_names(area)
            .map_err(FileIndexError::Capability)?;
        for name in names {
            let key =
                ManagedFileKey::new(area, name.as_str()).map_err(FileIndexError::Capability)?;
            let file = authority
                .open_existing_regular(&key)
                .map_err(FileIndexError::Capability)?;
            // This fixed index operation owns its single file guard and SQLite
            // query. Do not wrap it in another namespace-taking callback.
            if let Some(candidate) =
                index.orphan_candidate_for_namespace(authority, &file, cutoff)?
            {
                candidates.push(candidate);
            }
        }
    }
    Ok(candidates)
}
pub(super) fn rebuild_storage_references_for_namespace(
    _authority: &NamespaceStorageAuthority,
    _store: &Store,
) -> FileIndexResult<()> {
    Err(FileIndexError::RecoveryReferencesUnavailable)
}
pub(super) fn cleanup_orphaned_durable_copies_at_startup_for_namespace(
    _authority: &NamespaceStorageAuthority,
) -> FileIndexResult<()> {
    Err(FileIndexError::RecoveryReferencesUnavailable)
}
pub(super) fn cleanup_orphaned_durable_copies_at_shutdown_for_namespace(
    _authority: &NamespaceStorageAuthority,
) -> FileIndexResult<()> {
    Err(FileIndexError::RecoveryReferencesUnavailable)
}
fn all_store_references(groups: &ReferenceGroups) -> impl Iterator<Item = &ReferenceData> {
    groups
        .character
        .iter()
        .chain(groups.scene.iter())
        .chain(groups.ui.iter())
        .chain(groups.effect.iter())
}
pub(super) fn managed_output_path(path_text: &str) -> Option<PathBuf> {
    let path = usable_file_path(path_text)?;
    let output_root = configured_output_directory();
    if !safe_managed_subdirectory(&output_root) {
        return None;
    }
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        return None;
    }
    is_path_within(&output_root, &path).then_some(path)
}

pub(super) fn managed_preview_path(path: &Path) -> Option<PathBuf> {
    let preview_root = app_data_dir().join("cache").join("previews");
    if !safe_managed_subdirectory(&preview_root) {
        return None;
    }
    if !path.parent().is_some_and(safe_managed_subdirectory) {
        return None;
    }
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    is_path_within(&preview_root, path).then_some(path.to_path_buf())
}

pub(super) fn path_has_live_ui_reference(state: &AppState, path: &Path) -> bool {
    let scalar_paths = [
        state.get_custom_prompt_reference_path().to_string(),
        state.get_crop_source_path().to_string(),
        state.get_enhance_source_path().to_string(),
        state.get_enhance_result_path().to_string(),
        state.get_watermark_source_path().to_string(),
        state.get_watermark_result_path().to_string(),
        state.get_colorize_source_path().to_string(),
        state.get_colorize_result_path().to_string(),
        state.get_image_editor_source_path().to_string(),
        state.get_cutout_result_path().to_string(),
    ];
    if scalar_paths
        .iter()
        .any(|candidate| paths_refer_to_same_file(Path::new(candidate), path))
    {
        return true;
    }
    let compression = state.get_compression_images();
    let conversion = state.get_conversion_images();
    compression.iter().chain(conversion.iter()).any(|item| {
        paths_refer_to_same_file(Path::new(item.source_path.as_str()), path)
            || paths_refer_to_same_file(Path::new(item.result_path.as_str()), path)
    })
}

pub(super) fn store_references_path(store: &Store, path: &Path) -> bool {
    let asset_references_path = |item: &AssetData| {
        paths_refer_to_same_file(Path::new(&item.source_path), path)
            || item
                .reference_paths
                .iter()
                .any(|candidate| paths_refer_to_same_file(Path::new(candidate), path))
    };
    if store.assets.iter().any(asset_references_path)
        || store.generations.iter().any(asset_references_path)
        || store.inspiration.iter().any(asset_references_path)
        || all_store_references(&store.references).any(|reference| {
            paths_refer_to_same_file(Path::new(&reference.source_path), path)
        })
        || store
            .canvas_notes
            .iter()
            .any(|note| paths_refer_to_same_file(Path::new(&note.image_path), path))
        || store.canvas_workspaces.iter().any(|(workspace_id, workspace)| {
            workspace_id != &normalize_canvas_workspace_id(&store.active_canvas_workspace_id)
                && workspace
                    .notes
                    .iter()
                    .any(|note| paths_refer_to_same_file(Path::new(&note.image_path), path))
        })
        || store
            .canvas_references
            .iter()
            .any(|reference| paths_refer_to_same_file(Path::new(&reference.source_path), path))
        || store.canvas_workspaces.iter().any(|(workspace_id, workspace)| {
            workspace_id != &normalize_canvas_workspace_id(&store.active_canvas_workspace_id)
                && workspace.references.iter().any(|reference| {
                    paths_refer_to_same_file(Path::new(&reference.source_path), path)
                })
        })
    {
        return true;
    }
    store.custom_prompt_profiles.values().any(|profile| {
        paths_refer_to_same_file(Path::new(&profile.reference_path), path)
            || profile
                .reference_paths
                .iter()
                .any(|candidate| paths_refer_to_same_file(Path::new(candidate), path))
    })
}

pub(super) fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    if left.as_os_str().is_empty() || right.as_os_str().is_empty() {
        return false;
    }
    let left = canonical_or_absolute(left);
    let right = canonical_or_absolute(right);
    match (left, right) {
        (Some(left), Some(right)) => paths_equal(&left, &right),
        _ => false,
    }
}

fn usable_file_path(path_text: &str) -> Option<PathBuf> {
    let trimmed = path_text.trim();
    if trimmed.is_empty() || trimmed == "failed" {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return None;
    }
    Some(path)
}

fn is_path_within(root: &Path, candidate: &Path) -> bool {
    let Some(root) = canonical_or_absolute(root) else {
        return false;
    };
    let Some(candidate) = canonical_or_absolute(candidate) else {
        return false;
    };
    candidate != root && path_starts_with(&candidate, &root)
}

pub(super) fn safe_managed_subdirectory(directory: &Path) -> bool {
    let output = configured_output_directory();
    if !directory.starts_with(app_data_dir()) && (directory == output || directory.starts_with(&output)) {
        return crate::directory_migration::checked_directory(directory).is_ok();
    }
    let data = app_data_dir();
    let Ok(data_metadata) = fs::symlink_metadata(&data) else {
        return false;
    };
    if !data_metadata.file_type().is_dir() || data_metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(relative) = directory.strip_prefix(&data) else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return false;
    }
    let mut current = data;
    let Ok(mut canonical_parent) = current.canonicalize() else {
        return false;
    };
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return false;
        };
        current.push(component);
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            return false;
        };
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return false;
        }
        let Ok(canonical_child) = current.canonicalize() else {
            return false;
        };
        if canonical_child.parent() != Some(canonical_parent.as_path()) {
            return false;
        }
        canonical_parent = canonical_child;
    }
    is_path_within(&app_data_dir(), directory)
}

pub(super) fn ensure_managed_subdirectory(directory: &Path) -> bool {
    let output = configured_output_directory();
    if !directory.starts_with(app_data_dir()) && (directory == output || directory.starts_with(&output)) {
        if crate::directory_migration::checked_directory(&output).is_err() { return false; }
        let Ok(relative) = directory.strip_prefix(&output) else { return false; };
        let mut current = output;
        for component in relative.components() {
            let std::path::Component::Normal(component) = component else { return false; };
            current.push(component);
            match fs::create_dir(&current) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
                Err(_) => return false,
            }
            if crate::directory_migration::checked_directory(&current).is_err() { return false; }
        }
        return true;
    }
    let data = app_data_dir();
    let Ok(relative) = directory.strip_prefix(&data) else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return false;
    }
    let mut current = data;
    let Ok(mut canonical_parent) = current.canonicalize() else {
        return false;
    };
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return false;
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if fs::create_dir(&current).is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
        let Ok(canonical_child) = current.canonicalize() else {
            return false;
        };
        if canonical_child.parent() != Some(canonical_parent.as_path()) {
            return false;
        }
        canonical_parent = canonical_child;
    }
    safe_managed_subdirectory(directory)
}

fn canonical_or_absolute(path: &Path) -> Option<PathBuf> {
    if let Ok(path) = path.canonicalize() {
        return Some(path);
    }
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        std::env::current_dir().ok().map(|directory| directory.join(path))
    }
}

#[cfg(windows)]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(windows)]
fn path_starts_with(candidate: &Path, root: &Path) -> bool {
    candidate
        .to_string_lossy()
        .to_ascii_lowercase()
        .starts_with(&format!("{}\\", root.to_string_lossy().to_ascii_lowercase()))
        || candidate
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with(&format!("{}/", root.to_string_lossy().to_ascii_lowercase()))
}

#[cfg(not(windows))]
fn path_starts_with(candidate: &Path, root: &Path) -> bool {
    candidate.starts_with(root)
}


#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        index: FileIndex,
        a: NamespaceStorageAuthority,
        b: NamespaceStorageAuthority,
        directory: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().canonicalize().unwrap();
            let root = Arc::new(NamespaceFs::open_data_root(&path).unwrap());
            let make = |user| {
                NamespaceStorageAuthority::open(
                    Arc::clone(&root),
                    &NamespaceLease {
                        namespace: UserNamespace::new(&path, user).unwrap(),
                        auth_epoch: 4,
                        namespace_epoch: 2,
                    },
                )
                .unwrap()
            };
            Self {
                index: FileIndex::initialize(path.join("index.sqlite3")).unwrap(),
                a: make("11111111-1111-4111-8111-111111111111"),
                b: make("22222222-2222-4222-8222-222222222222"),
                directory,
            }
        }
    }
    #[test]
    fn task8b_lifecycle_registration_and_orphans_are_isolated_and_metadata_only() {
        let f = Fixture::new();
        let key = ManagedFileKey::new(ManagedUserArea::CanvasUploads, "same").unwrap();
        f.a.create_new_regular(&key).unwrap();
        f.b.create_new_regular(&key).unwrap();
        let a = managed_file_registration_for_namespace(&f.a, &key, "image", "user").unwrap();
        let b = managed_file_registration_for_namespace(&f.b, &key, "image", "user").unwrap();
        let a = f.index.register_file_for_namespace(&f.a, &a).unwrap();
        let b = f.index.register_file_for_namespace(&f.b, &b).unwrap();
        f.index
            .attach_reference_for_namespace(&f.b, b.id, "asset", "same")
            .unwrap();
        let orphan = ManagedFileKey::new(ManagedUserArea::ReferencesLibrary, "unindexed").unwrap();
        f.a.create_new_regular(&orphan).unwrap();
        f.a.create_new_regular(&ManagedFileKey::new(ManagedUserArea::Output, "excluded").unwrap())
            .unwrap();
        let candidates =
            discover_orphan_candidates_for_namespace(&f.index, &f.a, Duration::ZERO).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].indexed_file_id, Some(a.id));
        assert_eq!(candidates[1].indexed_file_id, None);
        assert!(
            discover_orphan_candidates_for_namespace(&f.index, &f.b, Duration::ZERO)
                .unwrap()
                .is_empty()
        );
        assert!(
            discover_orphan_candidates_for_namespace(&f.index, &f.a, Duration::from_secs(60))
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            discover_orphan_candidates_for_namespace(&f.index, &f.a, Duration::MAX),
            Err(FileIndexError::InvalidValue(_))
        ));
        assert!(f
            .a
            .lease()
            .namespace
            .path(ManagedUserArea::CanvasUploads)
            .join("same")
            .is_file());
    }
    #[test]
    fn task8b_lifecycle_enumeration_error_returns_no_partial_candidates() {
        let f = Fixture::new();
        f.a.create_new_regular(
            &ManagedFileKey::new(ManagedUserArea::CanvasUploads, "valid").unwrap(),
        )
        .unwrap();
        let library =
            f.a.lease()
                .namespace
                .path(ManagedUserArea::ReferencesLibrary);
        fs::rename(&library, library.with_file_name("detached")).unwrap();
        fs::create_dir(&library).unwrap();
        assert!(discover_orphan_candidates_for_namespace(&f.index, &f.a, Duration::ZERO).is_err());
        assert!(f
            .a
            .lease()
            .namespace
            .path(ManagedUserArea::CanvasUploads)
            .join("valid")
            .is_file());
    }
    #[test]
    fn task8b_lifecycle_cleanup_is_unavailable_and_old_adapters_do_no_io() {
        let f = Fixture::new();
        let path = f.a.lease().namespace.output_dir().join("preserve");
        fs::write(&path, b"untouched").unwrap();
        let backup = f.directory.path().join("storage-index.corrupt-old.sqlite3");
        fs::write(&backup, b"retain").unwrap();
        // Detached retained namespace: a new filesystem operation would fail.
        let root = f.a.lease().namespace.root();
        fs::rename(root, root.with_extension("detached")).unwrap();
        let store = Store::default();
        for result in [
            rebuild_storage_references_for_namespace(&f.a, &store),
            cleanup_orphaned_durable_copies_at_startup_for_namespace(&f.a),
            cleanup_orphaned_durable_copies_at_shutdown_for_namespace(&f.a),
        ] {
            assert!(matches!(
                result,
                Err(FileIndexError::RecoveryReferencesUnavailable)
            ));
        }
        initialize_storage_index();
        cleanup_orphaned_durable_copies_at_startup();
        cleanup_orphaned_durable_copies_at_shutdown();
        assert!(!rebuild_storage_references(&store));
        assert_eq!(indexed_reference_count(Path::new("\0bad")), u64::MAX);
        remove_indexed_file(Path::new("\0bad"));
        let inert = managed_file_registration(Path::new("\0bad"));
        assert_eq!(inert.kind, "unavailable");
        assert_eq!(inert.byte_size, 0);
        assert!(!inert.managed);
        assert_eq!(inert.retention_policy, "external");
        assert_eq!(inert.path, PathBuf::from("\0bad"));
        assert!(!root.exists());
        assert_eq!(
            fs::read(root.with_extension("detached").join("out/preserve")).unwrap(),
            b"untouched"
        );
        assert_eq!(fs::read(&backup).unwrap(), b"retain");
    }
    #[test]
    fn empty_failed_and_relative_paths_are_not_indexed() {
        assert!(usable_file_path("").is_none());
        assert!(usable_file_path("failed").is_none());
        assert!(usable_file_path("relative.png").is_none());
    }
}
