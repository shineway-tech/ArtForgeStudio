use super::*;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::SystemTime;

const STORAGE_INDEX_FILE_NAME: &str = "storage-index.sqlite3";
const STARTUP_CANVAS_ORPHAN_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const STARTUP_REFERENCE_ORPHAN_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DAMAGED_INDEX_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const DAMAGED_INDEX_MAX_FILES: usize = 3;
const DAMAGED_INDEX_MAX_BYTES: u64 = 64 * 1024 * 1024;
static FILE_REFERENCE_INDEX_HEALTHY: AtomicBool = AtomicBool::new(false);

pub(super) fn initialize_storage_index() {
    let path = app_data_dir().join(STORAGE_INDEX_FILE_NAME);
    if let Err(error) = initialize_global_file_index(path) {
        // The index is derived state. A failure must never prevent users from opening
        // the client or accessing their existing images.
        eprintln!("failed to initialize local file index: {error}");
    }
    cleanup_old_damaged_indexes();
}

fn cleanup_old_damaged_indexes() {
    let directory = app_data_dir();
    let Ok(entries) = fs::read_dir(&directory) else {
        return;
    };
    let now = SystemTime::now();
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("storage-index.corrupt-") {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let expired = now
            .duration_since(modified)
            .ok()
            .is_some_and(|age| age >= DAMAGED_INDEX_RETENTION);
        candidates.push((path, modified, metadata.len(), expired));
    }
    candidates.sort_by(|left, right| right.1.cmp(&left.1));
    let mut retained_bytes = 0_u64;
    for (index, (path, _, size, expired)) in candidates.into_iter().enumerate() {
        let exceeds_count = index >= DAMAGED_INDEX_MAX_FILES;
        let exceeds_bytes = retained_bytes.saturating_add(size) > DAMAGED_INDEX_MAX_BYTES;
        if expired || exceeds_count || exceeds_bytes {
            let _ = fs::remove_file(path);
        } else {
            retained_bytes = retained_bytes.saturating_add(size);
        }
    }
}

pub(super) fn rebuild_storage_references(store: &Store) -> bool {
    FILE_REFERENCE_INDEX_HEALTHY.store(false, AtomicOrdering::Release);
    let Some(index) = global_file_index() else {
        return false;
    };

    let mut references = Vec::new();
    for item in &store.assets {
        collect_asset_references(&mut references, "asset", item);
    }
    for item in &store.generations {
        collect_asset_references(&mut references, "generation", item);
    }
    for item in &store.inspiration {
        collect_asset_references(&mut references, "inspiration", item);
    }
    for reference in all_store_references(&store.references) {
        collect_path_reference(
            &mut references,
            &reference.source_path,
            "prompt-reference",
            &reference.id,
        );
    }
    for note in &store.canvas_notes {
        collect_path_reference(
            &mut references,
            &note.image_path,
            "canvas-node",
            &note.id,
        );
    }
    for (profile_id, profile) in &store.custom_prompt_profiles {
        collect_path_reference(
            &mut references,
            &profile.reference_path,
            "custom-prompt-reference",
            profile_id,
        );
        for path in &profile.reference_paths {
            collect_path_reference(
                &mut references,
                path,
                "custom-prompt-reference",
                profile_id,
            );
        }
    }
    match pending_recovery_file_references() {
        Ok(pending_references) => {
            for (owner_type, owner_id, path) in pending_references {
                collect_path_reference(
                    &mut references,
                    &path,
                    &owner_type,
                    &owner_id,
                );
            }
        }
        Err(error) => {
            eprintln!("failed to index recovery file references: {error}");
            return false;
        }
    }
    let healthy = match index.replace_all_references(&references) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("failed to rebuild local file references: {error}");
            false
        }
    };
    FILE_REFERENCE_INDEX_HEALTHY.store(healthy, AtomicOrdering::Release);
    healthy
}

pub(super) fn cleanup_orphaned_durable_copies_at_startup() {
    cleanup_unreferenced_direct_files(
        &app_data_dir().join("canvas").join("uploads"),
        STARTUP_CANVAS_ORPHAN_AGE,
    );
    cleanup_unreferenced_direct_files(
        &app_data_dir().join("references").join("library"),
        STARTUP_REFERENCE_ORPHAN_AGE,
    );
}

pub(super) fn cleanup_orphaned_durable_copies_at_shutdown() {
    cleanup_unreferenced_direct_files(
        &app_data_dir().join("canvas").join("uploads"),
        Duration::ZERO,
    );
    cleanup_unreferenced_direct_files(
        &app_data_dir().join("references").join("library"),
        Duration::ZERO,
    );
}

fn cleanup_unreferenced_direct_files(directory: &Path, minimum_age: Duration) {
    if !safe_managed_subdirectory(directory) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let old_enough = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= minimum_age);
        if !old_enough || indexed_reference_count(&path) > 0 {
            continue;
        }
        invalidate_previews_for_source(&path);
        if fs::remove_file(&path).is_ok() {
            remove_indexed_file(&path);
        }
    }
}

fn collect_asset_references(
    references: &mut Vec<FileReferenceRegistration>,
    owner_type: &str,
    item: &AssetData,
) {
    collect_path_reference(references, &item.source_path, owner_type, &item.id);
    let lineage_owner_type = format!("{owner_type}-lineage");
    for path in &item.reference_paths {
        collect_path_reference(references, path, &lineage_owner_type, &item.id);
    }
}

fn all_store_references(groups: &ReferenceGroups) -> impl Iterator<Item = &ReferenceData> {
    groups
        .character
        .iter()
        .chain(groups.scene.iter())
        .chain(groups.ui.iter())
        .chain(groups.effect.iter())
}

fn collect_path_reference(
    references: &mut Vec<FileReferenceRegistration>,
    path_text: &str,
    owner_type: &str,
    owner_id: &str,
) {
    let Some(path) = usable_file_path(path_text) else {
        return;
    };
    references.push(FileReferenceRegistration {
        file: managed_file_registration(&path),
        owner_type: owner_type.to_string(),
        owner_id: owner_id.to_string(),
    });
}

pub(super) fn managed_file_registration(path: &Path) -> ManagedFileRegistration {
    let byte_size = fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0);
    let managed = is_path_within(&app_data_dir(), path) || is_path_within(&configured_output_directory(), path);
    ManagedFileRegistration {
        path: path.to_path_buf(),
        kind: managed_file_kind(path).to_string(),
        byte_size,
        managed,
        retention_policy: if !managed {
            "external"
        } else if is_path_within(&app_data_dir().join("cache"), path) {
            "cache"
        } else if is_path_within(
            &configured_output_directory().join("image-edit-inputs"),
            path,
        ) || is_path_within(
            &configured_output_directory().join("upscale-references"),
            path,
        ) {
            "transient"
        } else if is_path_within(&app_data_dir().join("toolbox"), path)
            || is_path_within(&app_data_dir().join("delivery-staging"), path)
            || is_path_within(&app_data_dir().join("references").join("imports"), path)
        {
            "transient"
        } else {
            "user"
        }
        .to_string(),
    }
}

fn managed_file_kind(path: &Path) -> &'static str {
    let data = app_data_dir();
    if is_path_within(&data.join("cache").join("previews"), path) {
        "preview"
    } else if is_path_within(&configured_output_directory().join("image-edit-inputs"), path) {
        "image-edit-input"
    } else if is_path_within(&configured_output_directory().join("upscale-references"), path) {
        "upscale-input"
    } else if is_path_within(&configured_output_directory(), path) {
        "output"
    } else if is_path_within(&data.join("toolbox"), path) {
        "toolbox"
    } else if is_path_within(&data.join("references"), path) {
        "reference"
    } else if is_path_within(&data.join("canvas"), path) {
        "canvas"
    } else if is_path_within(&data.join("delivery-staging"), path) {
        "delivery-staging"
    } else {
        "source"
    }
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

pub(super) fn indexed_reference_count(path: &Path) -> u64 {
    if !FILE_REFERENCE_INDEX_HEALTHY.load(AtomicOrdering::Acquire) {
        return u64::MAX;
    }
    let Some(index) = global_file_index() else {
        return u64::MAX;
    };
    match index.find_file_by_path(path) {
        Ok(Some(record)) => index.reference_count(record.id).unwrap_or(u64::MAX),
        Ok(None) => 0,
        Err(_) => u64::MAX,
    }
}

pub(super) fn remove_indexed_file(path: &Path) {
    let Some(index) = global_file_index() else {
        return;
    };
    let Ok(Some(record)) = index.find_file_by_path(path) else {
        return;
    };
    if index.reference_count(record.id).unwrap_or(1) == 0 {
        let _ = index.delete_file(record.id);
    }
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

    #[test]
    fn empty_failed_and_relative_paths_are_not_indexed() {
        assert!(usable_file_path("").is_none());
        assert!(usable_file_path("failed").is_none());
        assert!(usable_file_path("relative.png").is_none());
    }

    #[test]
    fn managed_output_rejects_external_paths() {
        let external = std::env::temp_dir().join("artforge-external.png");
        assert!(managed_output_path(&external.display().to_string()).is_none());
    }
}
