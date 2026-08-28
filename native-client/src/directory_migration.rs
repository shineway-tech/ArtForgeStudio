//! Filesystem-only migration. No user files are removed until the caller durably
//! commits the new location. Kept independent of the UI for real filesystem tests.
use std::path::{Component, Path, PathBuf};
use std::{
    fs,
    io::{self, Read, Write},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    relative: PathBuf,
    directory: bool,
    size: u64,
    modified: SystemTime,
}

#[derive(Clone, Debug)]
pub(crate) struct MigrationPlan {
    pub source: PathBuf,
    pub destination: PathBuf,
    entries: Vec<Entry>,
    pub bytes: u64,
    pub files: usize,
}

impl MigrationPlan {
    pub fn prepare(source: &Path, destination: &Path, protected: &[PathBuf]) -> io::Result<Self> {
        let source = checked_directory(source)?;
        let destination = checked_directory(destination)?;
        for variable in ["USERPROFILE", "HOME", "SystemRoot", "ProgramFiles"] {
            if let Some(root) = std::env::var_os(variable).and_then(|p| fs::canonicalize(p).ok()) {
                if path_parts(&source) == path_parts(&root)
                    || path_parts(&destination) == path_parts(&root)
                {
                    return Err(io::Error::other(
                        "不能迁移整个用户目录或系统目录，请选择专用子文件夹",
                    ));
                }
            }
        }
        if overlaps(&source, &destination) {
            return Err(io::Error::other("新旧目录不能相同，也不能互相包含"));
        }
        for root in protected {
            if overlaps(
                &destination,
                &fs::canonicalize(root).unwrap_or_else(|_| root.clone()),
            ) {
                return Err(io::Error::other(
                    "目标目录与程序数据或其他素材目录重叠，请选择独立文件夹",
                ));
            }
        }
        let entries = scan(&source)?;
        check_conflicts(&destination, &entries)?;
        let bytes = entries
            .iter()
            .filter(|e| !e.directory)
            .map(|e| e.size)
            .sum();
        let files = entries.iter().filter(|e| !e.directory).count();
        Ok(Self {
            source,
            destination,
            entries,
            bytes,
            files,
        })
    }

    pub fn execute(
        &self,
        commit: impl FnOnce() -> io::Result<()>,
        mut progress: impl FnMut(u64, u64),
    ) -> io::Result<Vec<PathBuf>> {
        self.revalidate()?;
        check_conflicts(&self.destination, &self.entries)?;
        // create_new and create_dir never replace an existing destination, including
        // conflicts introduced after the confirmation dialog was shown.
        let mut created: Vec<(PathBuf, bool, Option<(u64, SystemTime)>)> = Vec::new();
        let result = (|| {
            let mut done = 0;
            progress(0, self.bytes);
            for entry in &self.entries {
                let source = self.source.join(&entry.relative);
                let destination = self.destination.join(&entry.relative);
                checked_directory(destination.parent().unwrap())?;
                if entry.directory {
                    fs::create_dir(&destination)?;
                    created.push((destination, true, None));
                } else {
                    ensure_regular_file(&source)?;
                    let mut input = fs::File::open(&source)?;
                    let mut output = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&destination)?;
                    created.push((destination.clone(), false, None));
                    let copied = (|| {
                        let mut buffer = [0u8; 128 * 1024];
                        loop {
                            let count = input.read(&mut buffer)?;
                            if count == 0 {
                                break;
                            }
                            output.write_all(&buffer[..count])?;
                            done += count as u64;
                            progress(done.min(self.bytes), self.bytes);
                        }
                        output.sync_all()?;
                        Ok::<_, io::Error>(())
                    })();
                    drop(output);
                    created.last_mut().unwrap().2 = file_stamp(&destination).ok();
                    copied?;
                    if !same_contents(&source, &destination)? {
                        return Err(io::Error::other("文件复制校验失败，原文件已保留"));
                    }
                }
            }
            self.revalidate()?;
            for entry in self.entries.iter().filter(|e| !e.directory) {
                if !same_contents(
                    &self.source.join(&entry.relative),
                    &self.destination.join(&entry.relative),
                )? {
                    return Err(io::Error::other("迁移期间文件发生变化，请重试"));
                }
            }
            #[cfg(unix)]
            {
                for (path, directory, _) in &created {
                    if *directory {
                        fs::File::open(path)?.sync_all()?;
                    }
                }
                fs::File::open(&self.destination)?.sync_all()?;
            }
            commit()
        })();
        if let Err(error) = result {
            let mut incomplete = false;
            for (path, directory, stamp) in created.iter().rev() {
                if checked_directory(path.parent().unwrap()).is_err() {
                    incomplete = true;
                    continue;
                }
                let removed = if *directory {
                    fs::remove_dir(path)
                } else if stamp.is_some()
                    && file_stamp(path).ok() == *stamp
                    && ensure_regular_file(path).is_ok()
                {
                    fs::remove_file(path)
                } else {
                    incomplete = true;
                    continue;
                };
                incomplete |= removed.is_err();
            }
            return Err(io::Error::other(if incomplete {
                format!("{error}；原目录未改动，目标目录中部分文件未能回退，请检查后重试")
            } else {
                error.to_string()
            }));
        }

        // The caller has persisted the new location. Cleanup is intentionally
        // conservative: locks or external edits leave a recoverable original copy.
        let mut leftovers = Vec::new();
        for entry in self.entries.iter().rev() {
            let source = self.source.join(&entry.relative);
            let destination = self.destination.join(&entry.relative);
            if checked_directory(source.parent().unwrap()).is_err() {
                leftovers.push(source);
                continue;
            }
            let result = if entry.directory {
                fs::remove_dir(&source)
            } else if file_stamp(&source).ok() == Some((entry.size, entry.modified))
                && same_contents(&source, &destination).unwrap_or(false)
            {
                fs::remove_file(&source)
            } else {
                leftovers.push(source);
                continue;
            };
            if result.is_err() {
                leftovers.push(source);
            }
        }
        progress(self.bytes, self.bytes);
        Ok(leftovers)
    }

    fn revalidate(&self) -> io::Result<()> {
        if checked_directory(&self.source)? != self.source
            || checked_directory(&self.destination)? != self.destination
            || scan(&self.source)? != self.entries
        {
            return Err(io::Error::other("目录内容已变化，请重新选择后迁移"));
        }
        Ok(())
    }
}

pub(crate) fn checked_directory(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(io::Error::other("请选择独立文件夹，不能使用磁盘根目录"));
    }
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        let metadata = fs::symlink_metadata(ancestor)?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err(io::Error::other("迁移目录不能包含符号链接或目录联接"));
        }
    }
    fs::canonicalize(path)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn ensure_regular_file(path: &Path) -> io::Result<()> {
    checked_directory(
        path.parent()
            .ok_or_else(|| io::Error::other("无效文件路径"))?,
    )?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(io::Error::other("不支持迁移链接或特殊文件"));
    }
    Ok(())
}

fn file_stamp(path: &Path) -> io::Result<(u64, SystemTime)> {
    let metadata = fs::symlink_metadata(path)?;
    Ok((metadata.len(), metadata.modified()?))
}

fn scan(root: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        checked_directory(&directory)?;
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if is_link(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
                return Err(io::Error::other("目录包含链接或特殊文件，未进行迁移"));
            }
            if metadata.is_dir() {
                pending.push(path.clone());
            }
            entries.push(Entry {
                relative: path
                    .strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_owned(),
                directory: metadata.is_dir(),
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                // Directory timestamps change as files are copied or deleted; only
                // the names and file contents participate in the snapshot.
                modified: if metadata.is_file() {
                    metadata.modified()?
                } else {
                    SystemTime::UNIX_EPOCH
                },
            });
        }
    }
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(entries)
}

fn check_conflicts(destination: &Path, entries: &[Entry]) -> io::Result<()> {
    for entry in entries {
        match fs::symlink_metadata(destination.join(&entry.relative)) {
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "目标文件夹已有同名文件或子目录：{}，未覆盖任何内容",
                    entry.relative.display()
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn same_contents(left: &Path, right: &Path) -> io::Result<bool> {
    ensure_regular_file(left)?;
    ensure_regular_file(right)?;
    if fs::metadata(left)?.len() != fs::metadata(right)?.len() {
        return Ok(false);
    }
    let mut left = io::BufReader::new(fs::File::open(left)?);
    let mut right = io::BufReader::new(fs::File::open(right)?);
    let mut a = [0u8; 65536];
    let mut b = [0u8; 65536];
    loop {
        let count = left.read(&mut a)?;
        if count == 0 {
            return Ok(right.read(&mut b)? == 0);
        }
        right.read_exact(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Ok(false);
        }
    }
}

fn path_parts(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    let text = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
    }
    .replace('/', "\\")
    .to_lowercase();
    Path::new(&*text)
        .components()
        .map(|p| p.as_os_str().to_string_lossy().into_owned())
        .collect()
}

pub(crate) fn overlaps(left: &Path, right: &Path) -> bool {
    let a = path_parts(left);
    let b = path_parts(right);
    a.starts_with(&b) || b.starts_with(&a)
}

pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    path_parts(left) == path_parts(right)
}

pub(crate) fn remap_path(value: &str, source: &Path, destination: &Path) -> Option<String> {
    let path = Path::new(value);
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return None;
    }
    let parts = path_parts(path);
    let source_parts = path_parts(source);
    if !parts.starts_with(&source_parts) {
        return None;
    }
    // Retain the original spelling of filenames when matching case-insensitively.
    let suffix: PathBuf = path.components().skip(source_parts.len()).collect();
    Some(destination.join(suffix).display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!(
                "elunvi-migration-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("old/nested/empty")).unwrap();
            fs::create_dir(root.join("new")).unwrap();
            fs::write(root.join("old/nested/作品.png"), b"image bytes").unwrap();
            fs::write(root.join("old/.hidden"), b"hidden bytes").unwrap();
            Self(root)
        }
        fn old(&self) -> PathBuf {
            self.0.join("old")
        }
        fn new_dir(&self) -> PathBuf {
            self.0.join("new")
        }
        fn plan(&self) -> MigrationPlan {
            MigrationPlan::prepare(&self.old(), &self.new_dir(), &[]).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn moves_nested_hidden_files_and_empty_directories_only_after_commit() {
        let f = Fixture::new();
        assert_eq!(f.plan().files, 2);
        fs::write(f.new_dir().join("keep.txt"), b"unrelated").unwrap();
        let mut progress = Vec::new();
        let leftovers = f
            .plan()
            .execute(
                || {
                    assert_eq!(
                        fs::read(f.old().join("nested/作品.png")).unwrap(),
                        b"image bytes"
                    );
                    assert_eq!(
                        fs::read(f.new_dir().join("nested/作品.png")).unwrap(),
                        b"image bytes"
                    );
                    Ok(())
                },
                |done, total| progress.push((done, total)),
            )
            .unwrap();
        assert!(leftovers.is_empty());
        assert_eq!(fs::read_dir(f.old()).unwrap().count(), 0);
        assert!(f.new_dir().join("nested/empty").is_dir());
        assert_eq!(
            fs::read(f.new_dir().join(".hidden")).unwrap(),
            b"hidden bytes"
        );
        assert_eq!(
            fs::read(f.new_dir().join("keep.txt")).unwrap(),
            b"unrelated"
        );
        assert_eq!(progress.last(), Some(&(23, 23)));
    }

    #[test]
    fn persistence_failure_keeps_source_and_rolls_back_only_created_entries() {
        let f = Fixture::new();
        fs::write(f.new_dir().join("keep.txt"), b"unrelated").unwrap();
        let result = f
            .plan()
            .execute(|| Err(io::Error::other("disk full")), |_, _| {});
        assert!(result.is_err());
        assert_eq!(
            fs::read(f.old().join("nested/作品.png")).unwrap(),
            b"image bytes"
        );
        assert_eq!(fs::read_dir(f.new_dir()).unwrap().count(), 1);
        assert_eq!(
            fs::read(f.new_dir().join("keep.txt")).unwrap(),
            b"unrelated"
        );
    }

    #[test]
    fn rejects_conflicts_before_changing_any_files() {
        let f = Fixture::new();
        fs::write(f.new_dir().join(".hidden"), b"destination bytes").unwrap();
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[]).is_err());
        assert_eq!(
            fs::read(f.new_dir().join(".hidden")).unwrap(),
            b"destination bytes"
        );
        assert!(f.old().join("nested/作品.png").exists());
    }

    #[test]
    fn rejects_identical_nested_parent_and_protected_targets() {
        let f = Fixture::new();
        for target in [f.old(), f.old().join("nested"), f.0.clone()] {
            assert!(MigrationPlan::prepare(&f.old(), &target, &[]).is_err());
        }
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[f.new_dir()]).is_err());
    }

    #[test]
    fn rechecks_conflicts_and_source_changes_after_confirmation() {
        let f = Fixture::new();
        let plan = f.plan();
        fs::write(f.new_dir().join(".hidden"), b"late file").unwrap();
        assert!(plan
            .execute(|| panic!("must not commit"), |_, _| {})
            .is_err());
        assert_eq!(fs::read(f.new_dir().join(".hidden")).unwrap(), b"late file");
        fs::remove_file(f.new_dir().join(".hidden")).unwrap();
        fs::write(f.old().join("new.txt"), b"new source").unwrap();
        assert!(plan
            .execute(|| panic!("must not commit"), |_, _| {})
            .is_err());
        assert!(f.old().join("new.txt").exists());
    }

    #[test]
    fn never_removes_a_source_modified_after_commit() {
        let f = Fixture::new();
        let leftovers = f
            .plan()
            .execute(
                || {
                    fs::write(f.old().join(".hidden"), b"changed by another application").unwrap();
                    Ok(())
                },
                |_, _| {},
            )
            .unwrap();
        assert!(!leftovers.is_empty());
        assert_eq!(
            fs::read(f.old().join(".hidden")).unwrap(),
            b"changed by another application"
        );
        assert_eq!(
            fs::read(f.new_dir().join(".hidden")).unwrap(),
            b"hidden bytes"
        );
    }

    #[test]
    fn empty_folder_can_be_migrated() {
        let f = Fixture::new();
        let empty = f.0.join("empty");
        fs::create_dir(&empty).unwrap();
        let plan = MigrationPlan::prepare(&empty, &f.new_dir(), &[]).unwrap();
        assert!(plan.execute(|| Ok(()), |_, _| {}).unwrap().is_empty());
    }

    #[test]
    fn copies_between_test_volumes_when_a_second_root_is_configured() {
        let Some(root) = std::env::var_os("ELUNVI_MIGRATION_TEST_TARGET_ROOT") else {
            return;
        };
        let f = Fixture::new();
        let root = fs::canonicalize(root).unwrap();
        let target = root.join(f.0.file_name().unwrap());
        fs::create_dir(&target).unwrap();
        let owned_target = Fixture(target.clone());
        let plan = MigrationPlan::prepare(&f.old(), &target, &[]).unwrap();
        assert!(plan.execute(|| Ok(()), |_, _| {}).unwrap().is_empty());
        assert_eq!(
            fs::read(target.join("nested/作品.png")).unwrap(),
            b"image bytes"
        );
        assert_eq!(fs::read_dir(f.old()).unwrap().count(), 0);
        drop(owned_target);
    }

    #[cfg(windows)]
    #[test]
    fn locked_source_preserves_files_and_does_not_commit() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = Fixture::new();
        let plan = f.plan();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(f.old().join(".hidden"))
            .unwrap();
        assert!(plan
            .execute(|| panic!("must not commit"), |_, _| {})
            .is_err());
        drop(lock);
        assert!(f.old().join(".hidden").exists());
        assert_eq!(fs::read_dir(f.new_dir()).unwrap().count(), 0);
    }

    #[test]
    fn source_changes_during_copy_abort_before_commit() {
        let f = Fixture::new();
        let mut changed = false;
        let result = f.plan().execute(
            || panic!("must not commit modified files"),
            |done, _| {
                if done > 0 && !changed {
                    changed = true;
                    fs::write(f.old().join(".hidden"), b"user edit").unwrap();
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read(f.old().join(".hidden")).unwrap(), b"user edit");
        assert_eq!(fs::read_dir(f.new_dir()).unwrap().count(), 0);
    }

    #[test]
    fn path_remapping_matches_directory_boundaries_not_string_prefixes() {
        let f = Fixture::new();
        assert_eq!(
            remap_path(
                &f.old().join("nested/作品.png").display().to_string(),
                &f.old(),
                &f.new_dir()
            ),
            Some(
                f.new_dir()
                    .join("nested")
                    .join("作品.png")
                    .display()
                    .to_string()
            )
        );
        assert_eq!(
            remap_path(
                &f.0.join("old-other/file.png").display().to_string(),
                &f.old(),
                &f.new_dir()
            ),
            None
        );
        assert_eq!(
            remap_path("https://example.test/old/image.png", &f.old(), &f.new_dir()),
            None
        );
        assert_eq!(
            remap_path(
                &f.old().join("../outside.png").display().to_string(),
                &f.old(),
                &f.new_dir()
            ),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_remapping_accepts_case_and_verbatim_prefix() {
        assert_eq!(
            remap_path(
                r"e:\OLD\Image.PNG",
                Path::new(r"\\?\E:\old"),
                Path::new(r"E:\new")
            ),
            Some(r"E:\new\Image.PNG".into())
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_unc_paths_keep_their_share_prefix() {
        assert_eq!(
            remap_path(
                r"\\server\share\old\Image.PNG",
                Path::new(r"\\?\UNC\server\share\old"),
                Path::new(r"D:\new")
            ),
            Some(r"D:\new\Image.PNG".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_rejected_without_touching_the_target() {
        let f = Fixture::new();
        std::os::unix::fs::symlink(f.new_dir(), f.old().join("link")).unwrap();
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[]).is_err());
        assert!(f.new_dir().is_dir());
    }
}
