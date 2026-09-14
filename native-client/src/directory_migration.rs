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
    retained: std::sync::Arc<RetainedMigration>,
    pub bytes: u64,
    pub files: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct MigrationManifestEntry {
    pub relative: PathBuf,
    pub directory: bool,
    pub size: u64,
    pub modified: SystemTime,
}

impl MigrationPlan {
    pub fn manifest(&self) -> Vec<MigrationManifestEntry> {
        self.entries.iter().map(|entry| MigrationManifestEntry {
            relative: entry.relative.clone(), directory: entry.directory,
            size: entry.size, modified: entry.modified,
        }).collect()
    }
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
        let retained_source = RetainedDirectory::open(&source)?;
        let retained_destination = RetainedDirectory::open(&destination)?;
        let entries = scan(&source)?;
        let retained = std::sync::Arc::new(RetainedMigration::capture(retained_source, retained_destination, &entries)?);
        retained.check_targets(&entries)?;
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
            retained,
            bytes,
            files,
        })
    }

    #[cfg(test)]
    pub fn execute(
        &self,
        commit: impl FnOnce() -> io::Result<()>,
        progress: impl FnMut(u64, u64),
    ) -> io::Result<Vec<PathBuf>> {
        self.execute_with_cleanup(commit, progress, true)
    }

    /// Account relocation retains its verified originals as a recovery copy.
    /// Failed copies retain partial destinations; no pathname rollback can delete
    /// a replacement file created by another process.
    pub fn copy_retaining_source(
        &self,
        commit: impl FnOnce() -> io::Result<()>,
        mut progress: impl FnMut(u64, u64),
    ) -> io::Result<()> {
        copy_batch_retaining_source(std::slice::from_ref(self), commit, |done, total| {
            progress(done, total);
            Ok(())
        })
    }

    #[cfg(test)]
    fn execute_with_cleanup(
        &self,
        commit: impl FnOnce() -> io::Result<()>,
        mut progress: impl FnMut(u64, u64),
        cleanup_source: bool,
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

        if !cleanup_source {
            return Ok(Vec::new());
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

    #[cfg(test)]
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

/// Copies every prepared directory while retaining each source, then validates
/// every pinned source and copied target before committing all mappings once.
pub(crate) fn copy_batch_retaining_source(
    plans: &[MigrationPlan],
    commit: impl FnOnce() -> io::Result<()>,
    mut progress: impl FnMut(u64, u64) -> io::Result<()>,
) -> io::Result<()> {
    let total = plans.iter().try_fold(0u64, |total, plan| {
        total
            .checked_add(plan.bytes)
            .ok_or_else(|| io::Error::other("迁移文件总大小超出支持范围"))
    })?;
    let result = (|| {
        progress(0, total)?;
        let mut copied = Vec::with_capacity(plans.len());
        let mut completed = 0u64;
        for plan in plans {
            let outputs = plan.retained.copy_files(plan, |done| {
                progress(completed.saturating_add(done).min(total), total)
            })?;
            completed = completed
                .checked_add(plan.bytes)
                .ok_or_else(|| io::Error::other("迁移进度超出支持范围"))?;
            copied.push(outputs);
        }

        // No callback or other caller-controlled work may occur between this
        // cross-plan verification and the single durable mapping commit.
        for (plan, outputs) in plans.iter().zip(&copied) {
            plan.retained.verify_copy(plan, outputs)?;
        }
        commit()
    })();

    // Never unlink by a display path during rollback: another process could
    // replace it after the identity check. Originals and partial copies stay.
    result.map_err(|error: io::Error| {
        io::Error::other(format!(
            "{error}；原文件已保留，目标中可能有未完成副本，请检查后重试"
        ))
    })
}


// A plan pins both root directory chains before confirmation. Path checks
// alone cannot make a later File::open safe: a parent or leaf may be replaced
// between metadata and open. Unix descends with openat(O_NOFOLLOW); Windows
// pins every ancestor against deletion and opens leaves as reparse points.
#[derive(Debug)]
struct RetainedDirectory {
    path: PathBuf,
    chain: Vec<std::sync::Arc<fs::File>>,
}
#[derive(Debug)]
struct RetainedEntry {
    entry: Entry,
    identity: (u64, u64, u64),
    stamp: Option<(u64, SystemTime, i64, i64)>,
}
#[derive(Debug)]
struct RetainedMigration {
    source: RetainedDirectory,
    destination: RetainedDirectory,
    entries: Vec<RetainedEntry>,
}

fn identity(file: &fs::File) -> io::Result<(u64, u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        Ok((m.dev(), m.ino(), 0))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((info.dwVolumeSerialNumber as u64, info.nFileIndexHigh as u64, info.nFileIndexLow as u64))
    }
    #[cfg(not(any(unix, windows)))]
    { let _ = file; Err(io::Error::other("此平台不支持安全目录迁移")) }
}

fn verify_regular(file: &fs::File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_file() || is_link(&m) { return Err(io::Error::other("迁移文件不是普通文件")); }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.nlink() != 1 { return Err(io::Error::other("不支持迁移硬链接文件")); }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.nNumberOfLinks != 1 { return Err(io::Error::other("不支持迁移硬链接文件")); }
    }
    Ok(())
}

impl RetainedDirectory {
    fn handle(&self) -> &fs::File { self.chain.last().unwrap() }
    fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() { return Err(io::Error::other("迁移目录必须为绝对路径")); }
        let mut components = path.components();
        #[cfg(unix)]
        let (anchor, handle) = {
            if components.next() != Some(Component::RootDir) { return Err(io::Error::other("无效目录根")); }
            let fd = rustix::fs::open("/", rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC, rustix::fs::Mode::empty())?;
            (PathBuf::from("/"), fs::File::from(fd))
        };
        #[cfg(windows)]
        let (anchor, handle) = {
            use std::path::Prefix;
            let prefix = match components.next() {
                Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => p,
                _ => return Err(io::Error::other("迁移需要本地磁盘目录")),
            };
            if components.next() != Some(Component::RootDir) { return Err(io::Error::other("无效目录根")); }
            let mut anchor = PathBuf::from(prefix.as_os_str()); anchor.push("\\");
            let handle = windows_open(&anchor, true, false)?;
            (anchor, handle)
        };
        #[cfg(not(any(unix, windows)))]
        let (anchor, handle): (PathBuf, fs::File) = return Err(io::Error::other("此平台不支持安全目录迁移"));
        let mut result = Self { path: anchor, chain: vec![std::sync::Arc::new(handle)] };
        for component in components {
            let Component::Normal(name) = component else { return Err(io::Error::other("迁移目录包含非标准路径")); };
            result = result.child_directory(name, false)?;
        }
        Ok(result)
    }
    fn child(&self, name: &std::ffi::OsStr, directory: bool, create: bool) -> io::Result<fs::File> {
        let mut parts = Path::new(name).components();
        if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
            return Err(io::Error::other("无效迁移文件名"));
        }
        #[cfg(unix)]
        let file = {
            use rustix::fs::{Mode, OFlags};
            if directory && create { rustix::fs::mkdirat(self.handle(), name, Mode::RWXU)?; }
            let flags = OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
                | if directory { OFlags::RDONLY | OFlags::DIRECTORY }
                  else if create { OFlags::RDWR | OFlags::CREATE | OFlags::EXCL }
                  else { OFlags::RDONLY };
            fs::File::from(rustix::fs::openat(self.handle(), name, flags, Mode::RUSR | Mode::WUSR)?)
        };
        #[cfg(windows)]
        let file = {
            // Every parent is held without FILE_SHARE_DELETE. The verified path
            // cannot be renamed into a reparse point while this chain is alive.
            if directory && create { fs::create_dir(self.path.join(name))?; }
            windows_open(&self.path.join(name), directory, create && !directory)?
        };
        #[cfg(not(any(unix, windows)))]
        let file: fs::File = return Err(io::Error::other("此平台不支持安全目录迁移"));
        let metadata = file.metadata()?;
        if is_link(&metadata) || metadata.is_dir() != directory {
            return Err(io::Error::other("目录或文件被替换为链接/特殊文件"));
        }
        if !directory { verify_regular(&file)?; }
        Ok(file)
    }
    fn staging_file(&self) -> io::Result<(std::ffi::OsString, fs::File)> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        loop {
            let name = std::ffi::OsString::from(format!(
                ".elunvi-migration-{}-{}-{}.tmp", std::process::id(),
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ));
            match self.child(&name, false, true) {
                Ok(file) => return Ok((name, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
    fn replace_file(&self, temporary: &std::ffi::OsStr, name: &std::ffi::OsStr, output: &fs::File) -> io::Result<()> {
        self.verify()?;
        verify_regular(output)?;
        #[cfg(unix)]
        {
            // Both names resolve beneath the pinned parent; rename never follows
            // the destination leaf. Verify the staging name still owns our inode.
            let staged = self.child(temporary, false, false)?;
            if identity(&staged)? != identity(output)? {
                return Err(io::Error::other("迁移临时文件身份已变化"));
            }
            rustix::fs::renameat(self.handle(), temporary, self.handle(), name)?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
            use windows_sys::Win32::Storage::FileSystem::{SetFileInformationByHandle, FileRenameInfo, FILE_RENAME_INFO};
            // Rename the opened file itself. Ancestors and the staging handle
            // deny delete sharing, so neither can be exchanged beneath us.
            let _ = temporary;
            let wide: Vec<u16> = self.path.join(name).as_os_str().encode_wide().collect();
            let bytes = std::mem::size_of::<FILE_RENAME_INFO>() + wide.len() * 2;
            let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
            let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
            unsafe {
                (*info).Anonymous.ReplaceIfExists = true;
                (*info).RootDirectory = std::ptr::null_mut();
                (*info).FileNameLength = (wide.len() * 2) as u32;
                std::ptr::copy_nonoverlapping(wide.as_ptr(), (*info).FileName.as_mut_ptr(), wide.len());
                if SetFileInformationByHandle(output.as_raw_handle(), FileRenameInfo, info.cast(), bytes as u32) == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        return Err(io::Error::other("此平台不支持安全目录迁移"));
        Ok(())
    }
    fn child_directory(&self, name: &std::ffi::OsStr, create: bool) -> io::Result<Self> {
        let child = self.child(name, true, create)?;
        let mut chain = self.chain.clone(); chain.push(std::sync::Arc::new(child));
        Ok(Self { path: self.path.join(name), chain })
    }
    fn verify(&self) -> io::Result<()> {
        let current = Self::open(&self.path)?;
        if current.chain.len() != self.chain.len() { return Err(io::Error::other("迁移目录已变化")); }
        for (before, after) in self.chain.iter().zip(&current.chain) {
            if identity(before)? != identity(after)? { return Err(io::Error::other("迁移目录身份已变化，请重新选择")); }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn windows_open(path: &Path, directory: bool, create: bool) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE};
    let file = fs::OpenOptions::new().read(true).write(create).create_new(create)
        .access_mode(FILE_GENERIC_READ | if create { FILE_GENERIC_WRITE | DELETE } else { 0 })
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | if directory { FILE_FLAG_BACKUP_SEMANTICS } else { 0 })
        .open(path)?;
    let m = file.metadata()?;
    if is_link(&m) || (directory && !m.is_dir()) { return Err(io::Error::other("迁移路径包含重解析点")); }
    Ok(file)
}

fn retained_stamp(file: &fs::File) -> io::Result<(u64, SystemTime, i64, i64)> {
    let m = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok((m.len(), m.modified()?, m.ctime(), m.ctime_nsec()))
    }
    #[cfg(not(unix))]
    { Ok((m.len(), m.modified()?, 0, 0)) }
}
fn read_at(file: &fs::File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    { use std::os::unix::fs::FileExt; file.read_at(buffer, offset) }
    #[cfg(windows)]
    { use std::os::windows::fs::FileExt; file.seek_read(buffer, offset) }
    #[cfg(not(any(unix, windows)))]
    { let _ = (file, buffer, offset); Err(io::Error::other("此平台不支持安全目录迁移")) }
}
fn retained_equal(left: &fs::File, right: &fs::File) -> io::Result<bool> {
    verify_regular(left)?; verify_regular(right)?;
    let before = (retained_stamp(left)?, retained_stamp(right)?);
    if before.0.0 != before.1.0 { return Ok(false); }
    let mut offset = 0;
    let mut a = [0u8; 65536]; let mut b = [0u8; 65536];
    while offset < before.0.0 {
        let limit = ((before.0.0 - offset) as usize).min(a.len());
        let count = read_at(left, &mut a[..limit], offset)?;
        if count == 0 { return Ok(false); }
        let mut filled = 0;
        while filled < count {
            let n = read_at(right, &mut b[filled..count], offset + filled as u64)?;
            if n == 0 { return Ok(false); } filled += n;
        }
        if a[..count] != b[..count] { return Ok(false); }
        offset += count as u64;
    }
    Ok(before == (retained_stamp(left)?, retained_stamp(right)?))
}
impl RetainedMigration {
    // Retain only the roots. A large gallery must not require one descriptor per
    // image; every operation reopens beneath those roots and checks the saved ID.
    fn parent(root: &RetainedDirectory, entries: &[RetainedEntry], relative: &Path) -> io::Result<RetainedDirectory> {
        let mut parent = RetainedDirectory { path: root.path.clone(), chain: root.chain.clone() };
        let mut prefix = PathBuf::new();
        for part in relative.parent().unwrap_or(Path::new("")).components() {
            let Component::Normal(name) = part else { return Err(io::Error::other("无效迁移路径")); };
            prefix.push(name);
            let expected = entries.iter().find(|held| held.entry.directory && held.entry.relative == prefix)
                .ok_or_else(|| io::Error::other("迁移父目录尚未验证"))?;
            parent = parent.child_directory(name, false)?;
            if identity(parent.handle())? != expected.identity { return Err(io::Error::other("迁移父目录身份已变化")); }
        }
        Ok(parent)
    }
    fn open_entry(root: &RetainedDirectory, entries: &[RetainedEntry], held: &RetainedEntry) -> io::Result<(RetainedDirectory, fs::File)> {
        let parent = Self::parent(root, entries, &held.entry.relative)?;
        let file = parent.child(held.entry.relative.file_name().unwrap(), held.entry.directory, false)?;
        if identity(&file)? != held.identity { return Err(io::Error::other("迁移文件身份已变化")); }
        Ok((parent, file))
    }
    fn capture(source: RetainedDirectory, destination: RetainedDirectory, snapshot: &[Entry]) -> io::Result<Self> {
        let mut entries = Vec::with_capacity(snapshot.len());
        for entry in snapshot {
            let parent = Self::parent(&source, &entries, &entry.relative)?;
            let file = parent.child(entry.relative.file_name().ok_or_else(|| io::Error::other("缺少文件名"))?, entry.directory, false)?;
            if !entry.directory {
                let m = file.metadata()?;
                if m.len() != entry.size || m.modified()? != entry.modified { return Err(io::Error::other("准备期间源文件已变化")); }
            }
            let stamp = if entry.directory { None } else { Some(retained_stamp(&file)?) };
            entries.push(RetainedEntry { entry: entry.clone(), identity: identity(&file)?, stamp });
        }
        source.verify()?; destination.verify()?;
        Ok(Self { source, destination, entries })
    }
    fn check_targets(&self, entries: &[Entry]) -> io::Result<()> {
        self.destination.verify()?;
        'entry: for entry in entries {
            let mut parent = RetainedDirectory { path: self.destination.path.clone(), chain: self.destination.chain.clone() };
            for part in entry.relative.parent().unwrap_or(Path::new("")).components() {
                let Component::Normal(name) = part else { return Err(io::Error::other("无效迁移路径")); };
                match parent.child_directory(name, false) {
                    Ok(child) => parent = child,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue 'entry,
                    Err(error) => return Err(error),
                }
            }
            match parent.child(entry.relative.file_name().unwrap(), entry.directory, false) {
                Ok(_) => {},
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => return Err(error),
            }
        }
        self.destination.verify()
    }
    fn verify_source(&self, plan: &MigrationPlan) -> io::Result<()> {
        self.source.verify()?; self.destination.verify()?;
        if scan(&plan.source)? != plan.entries { return Err(io::Error::other("目录内容已变化，请重新选择")); }
        for held in &self.entries {
            let (_parent, file) = Self::open_entry(&self.source, &self.entries, held)?;
            if !held.entry.directory && Some(retained_stamp(&file)?) != held.stamp {
                return Err(io::Error::other("源文件内容已变化，请重新选择"));
            }
        }
        Ok(())
    }
    fn copy_files(
        &self,
        plan: &MigrationPlan,
        mut progress: impl FnMut(u64) -> io::Result<()>,
    ) -> io::Result<Vec<RetainedEntry>> {
        self.verify_source(plan)?;
        self.check_targets(&plan.entries)?;
        let mut outputs: Vec<RetainedEntry> = Vec::new();
        let mut done = 0;
        for source in &self.entries {
            let (_source_parent, input) =
                Self::open_entry(&self.source, &self.entries, source)?;
            let parent = Self::parent(&self.destination, &outputs, &source.entry.relative)?;
            parent.verify()?;
            let name = source.entry.relative.file_name().unwrap();
            let existing = match parent.child(name, source.entry.directory, false) {
                Ok(file) => Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            let previous = existing.as_ref().map(|file| Ok::<_, io::Error>((identity(file)?, retained_stamp(file)?))).transpose()?;
            let (temporary, mut output) = if source.entry.directory {
                (None, match existing { Some(file) => file, None => parent.child(name, true, true)? })
            } else {
                // Release the previous target before Windows replaces it; never
                // truncate it until the new sibling has been copied and checked.
                drop(existing);
                let (name, file) = parent.staging_file()?;
                (Some(name), file)
            };
            if !source.entry.directory {
                let before = retained_stamp(&input)?;
                if Some(before) != source.stamp {
                    return Err(io::Error::other("源文件内容已变化"));
                }
                let mut offset = 0;
                let mut buffer = [0u8; 128 * 1024];
                loop {
                    let n = read_at(&input, &mut buffer, offset)?;
                    if n == 0 {
                        break;
                    }
                    output.write_all(&buffer[..n])?;
                    offset += n as u64;
                    done += n as u64;
                    progress(done.min(plan.bytes))?;
                    if offset > source.entry.size {
                        return Err(io::Error::other("复制期间源文件增长"));
                    }
                }
                output.sync_all()?;
                if before != retained_stamp(&input)? || !retained_equal(&input, &output)? {
                    return Err(io::Error::other("文件复制校验失败，原文件已保留"));
                }
            }
            if let Some(temporary) = temporary {
                let current = match parent.child(name, false, false) {
                    Ok(file) => Some((identity(&file)?, retained_stamp(&file)?)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                };
                if current != previous {
                    return Err(io::Error::other("复制期间目标文件已变化，请重试"));
                }
                parent.replace_file(&temporary, name, &output)?;
            }
            #[cfg(unix)]
            parent.handle().sync_all()?;
            let stamp = if source.entry.directory {
                None
            } else {
                Some(retained_stamp(&output)?)
            };
            outputs.push(RetainedEntry {
                entry: source.entry.clone(),
                identity: identity(&output)?,
                stamp,
            });
        }
        Ok(outputs)
    }

    fn verify_copy(&self, plan: &MigrationPlan, outputs: &[RetainedEntry]) -> io::Result<()> {
        self.verify_source(plan)?;
        if outputs.len() != self.entries.len() {
            return Err(io::Error::other("迁移目标文件不完整"));
        }
        for (source, target) in self.entries.iter().zip(outputs) {
            let (_source_parent, input) =
                Self::open_entry(&self.source, &self.entries, source)?;
            let (_target_parent, output) =
                Self::open_entry(&self.destination, outputs, target)?;
            if !target.entry.directory && !retained_equal(&input, &output)? {
                return Err(io::Error::other("迁移期间文件发生变化"));
            }
            #[cfg(unix)]
            if target.entry.directory {
                output.sync_all()?;
            }
        }
        self.source.verify()?;
        self.destination.verify()?;
        #[cfg(unix)]
        self.destination.handle().sync_all()?;
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
    fn account_copy_overwrites_files_and_merges_directories() {
        let f = Fixture::new();
        fs::create_dir_all(f.new_dir().join("nested/empty")).unwrap();
        fs::write(f.new_dir().join(".hidden"), b"old destination").unwrap();
        fs::write(f.new_dir().join("nested/作品.png"), b"partial").unwrap();
        fs::write(f.new_dir().join("nested/unrelated"), b"keep").unwrap();
        f.plan().copy_retaining_source(|| Ok(()), |_, _| {}).unwrap();
        for root in [f.old(), f.new_dir()] {
            assert_eq!(fs::read(root.join(".hidden")).unwrap(), b"hidden bytes");
            assert_eq!(fs::read(root.join("nested/作品.png")).unwrap(), b"image bytes");
        }
        assert_eq!(fs::read(f.new_dir().join("nested/unrelated")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(f.new_dir()).unwrap().count(), 2);
    }

    #[test]
    fn account_copy_retry_after_failed_commit_overwrites_partial_destination() {
        let f = Fixture::new();
        assert!(f.plan().copy_retaining_source(|| Err(io::Error::other("commit failed")), |_, _| {}).is_err());
        fs::write(f.new_dir().join("nested/作品.png"), b"partial").unwrap();
        f.plan().copy_retaining_source(|| Ok(()), |_, _| {}).unwrap();
        assert_eq!(fs::read(f.new_dir().join("nested/作品.png")).unwrap(), b"image bytes");
    }

    #[test]
    fn account_copy_failed_verification_keeps_existing_destination_intact() {
        let f = Fixture::new();
        fs::write(f.new_dir().join(".hidden"), b"previous destination").unwrap();
        let mut changed = false;
        let result = f.plan().copy_retaining_source(|| panic!("must not commit"), |done, _| {
            if done > 0 && !changed {
                changed = true;
                fs::write(f.old().join(".hidden"), b"changed source").unwrap();
            }
        });
        assert!(result.is_err());
        assert!(changed);
        assert_eq!(fs::read(f.new_dir().join(".hidden")).unwrap(), b"previous destination");
    }

    #[test]
    fn account_copy_overwrites_regular_file_created_after_prepare() {
        let f = Fixture::new();
        let plan = f.plan();
        fs::write(f.new_dir().join(".hidden"), b"late destination").unwrap();
        plan.copy_retaining_source(|| Ok(()), |_, _| {}).unwrap();
        assert_eq!(fs::read(f.new_dir().join(".hidden")).unwrap(), b"hidden bytes");
    }

    #[test]
    fn account_copy_rejects_destination_type_mismatches() {
        let f = Fixture::new();
        fs::create_dir(f.new_dir().join(".hidden")).unwrap();
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[]).is_err());
        fs::remove_dir(f.new_dir().join(".hidden")).unwrap();
        fs::write(f.new_dir().join("nested"), b"keep file").unwrap();
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[]).is_err());
        assert_eq!(fs::read(f.new_dir().join("nested")).unwrap(), b"keep file");
    }

    #[test]
    fn account_copy_rejects_hardlinked_destination_file() {
        let f = Fixture::new();
        let external = f.0.join("external");
        fs::write(&external, b"external data").unwrap();
        fs::hard_link(&external, f.new_dir().join(".hidden")).unwrap();
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[]).is_err());
        assert_eq!(fs::read(external).unwrap(), b"external data");
    }

    #[test]
    fn account_copy_does_not_overwrite_destination_changed_during_copy() {
        let f = Fixture::new();
        fs::write(f.new_dir().join(".hidden"), b"previous destination").unwrap();
        let mut changed = false;
        let result = f.plan().copy_retaining_source(|| panic!("must not commit"), |done, _| {
            if done > 0 && !changed {
                changed = true;
                assert_eq!(fs::read(f.new_dir().join(".hidden")).unwrap(), b"previous destination");
                fs::write(f.new_dir().join(".hidden"), b"concurrent edit").unwrap();
            }
        });
        assert!(result.is_err());
        assert!(changed);
        assert_eq!(fs::read(f.new_dir().join(".hidden")).unwrap(), b"concurrent edit");
    }

    #[cfg(unix)]
    #[test]
    fn account_copy_rejects_destination_links_and_special_files() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let external = f.0.join("external");
        fs::write(&external, b"external data").unwrap();
        symlink(&external, f.new_dir().join(".hidden")).unwrap();
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[]).is_err());
        fs::remove_file(f.new_dir().join(".hidden")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(f.new_dir().join(".hidden")).status().unwrap().success());
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[]).is_err());
        fs::remove_file(f.new_dir().join(".hidden")).unwrap();
        symlink(f.old().join("nested"), f.new_dir().join("nested")).unwrap();
        assert!(MigrationPlan::prepare(&f.old(), &f.new_dir(), &[]).is_err());
        assert_eq!(fs::read(external).unwrap(), b"external data");
    }

    #[cfg(unix)]
    #[test]
    fn account_copy_rejects_target_symlink_installed_during_staging() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        fs::write(f.new_dir().join(".hidden"), b"previous destination").unwrap();
        let external = f.0.join("external");
        fs::write(&external, b"external data").unwrap();
        let mut changed = false;
        let result = f.plan().copy_retaining_source(|| panic!("must not commit"), |done, _| {
            if done > 0 && !changed {
                changed = true;
                fs::remove_file(f.new_dir().join(".hidden")).unwrap();
                symlink(&external, f.new_dir().join(".hidden")).unwrap();
            }
        });
        assert!(result.is_err());
        assert!(changed);
        assert_eq!(fs::read(external).unwrap(), b"external data");
        assert!(fs::symlink_metadata(f.new_dir().join(".hidden")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn batch_copy_commits_once_after_copying_every_plan() {
        let fixtures = [Fixture::new(), Fixture::new(), Fixture::new()];
        let plans: Vec<_> = fixtures.iter().map(Fixture::plan).collect();
        let total = plans.iter().map(|plan| plan.bytes).sum();
        let commits = std::cell::Cell::new(0);
        let mut progress = Vec::new();

        copy_batch_retaining_source(
            &plans,
            || {
                commits.set(commits.get() + 1);
                for fixture in &fixtures {
                    assert_eq!(
                        fs::read(fixture.new_dir().join("nested/作品.png"))?,
                        b"image bytes"
                    );
                    assert_eq!(
                        fs::read(fixture.new_dir().join(".hidden"))?,
                        b"hidden bytes"
                    );
                }
                Ok(())
            },
            |done, total| {
                progress.push((done, total));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(commits.get(), 1);
        assert_eq!(progress.first(), Some(&(0, total)));
        assert_eq!(progress.last(), Some(&(total, total)));
        assert!(progress.windows(2).all(|window| window[0].0 <= window[1].0));
        for fixture in &fixtures {
            assert_eq!(fs::read(fixture.old().join(".hidden")).unwrap(), b"hidden bytes");
        }
    }

    #[test]
    fn batch_copy_later_conflict_prevents_commit_and_keeps_partial_outputs() {
        let fixtures = [Fixture::new(), Fixture::new(), Fixture::new()];
        let plans: Vec<_> = fixtures.iter().map(Fixture::plan).collect();
        fs::create_dir(fixtures[1].new_dir().join(".hidden")).unwrap();
        let commits = std::cell::Cell::new(0);

        let result = copy_batch_retaining_source(
            &plans,
            || {
                commits.set(commits.get() + 1);
                Ok(())
            },
            |_, _| Ok(()),
        );

        assert!(result.is_err());
        assert_eq!(commits.get(), 0);
        assert_eq!(
            fs::read(fixtures[0].new_dir().join("nested/作品.png")).unwrap(),
            b"image bytes"
        );
        assert!(fixtures[1].new_dir().join(".hidden").is_dir());
        assert_eq!(fs::read_dir(fixtures[2].new_dir()).unwrap().count(), 0);
    }

    #[test]
    fn batch_copy_revalidates_an_earlier_source_after_later_copy() {
        let fixtures = [Fixture::new(), Fixture::new(), Fixture::new()];
        let plans: Vec<_> = fixtures.iter().map(Fixture::plan).collect();
        let first_bytes = plans[0].bytes;
        let mut changed = false;

        let result = copy_batch_retaining_source(
            &plans,
            || panic!("changed source must not commit"),
            |done, _| {
                if done > first_bytes && !changed {
                    changed = true;
                    fs::write(fixtures[0].old().join(".hidden"), b"edited after copy")?;
                }
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(changed);
        assert_eq!(
            fs::read(fixtures[0].old().join(".hidden")).unwrap(),
            b"edited after copy"
        );
        assert!(fixtures[0].new_dir().join(".hidden").exists());
    }

    #[test]
    fn batch_copy_detects_replacement_of_an_earlier_target() {
        let fixtures = [Fixture::new(), Fixture::new(), Fixture::new()];
        let plans: Vec<_> = fixtures.iter().map(Fixture::plan).collect();
        let first_bytes = plans[0].bytes;
        let target = fixtures[0].new_dir().join(".hidden");
        let mut replaced = false;

        let result = copy_batch_retaining_source(
            &plans,
            || panic!("replaced target must not commit"),
            |done, _| {
                if done > first_bytes && !replaced {
                    replaced = true;
                    fs::remove_file(&target)?;
                    fs::write(&target, b"replacement")?;
                }
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(replaced);
        assert_eq!(fs::read(target).unwrap(), b"replacement");
        assert!(fixtures[0].old().join(".hidden").exists());
    }

    #[test]
    fn batch_copy_commit_failure_retains_all_sources_and_destinations() {
        let fixtures = [Fixture::new(), Fixture::new(), Fixture::new()];
        let plans: Vec<_> = fixtures.iter().map(Fixture::plan).collect();

        let result = copy_batch_retaining_source(
            &plans,
            || Err(io::Error::other("mapping commit failed")),
            |_, _| Ok(()),
        );

        assert!(result.is_err());
        for fixture in &fixtures {
            for root in [fixture.old(), fixture.new_dir()] {
                assert_eq!(fs::read(root.join(".hidden")).unwrap(), b"hidden bytes");
                assert_eq!(
                    fs::read(root.join("nested/作品.png")).unwrap(),
                    b"image bytes"
                );
            }
        }
    }

    #[test]
    fn batch_copy_supports_empty_sources() {
        let fixtures = [Fixture::new(), Fixture::new(), Fixture::new()];
        for fixture in &fixtures {
            fs::remove_dir_all(fixture.old()).unwrap();
            fs::create_dir(fixture.old()).unwrap();
        }
        let plans: Vec<_> = fixtures.iter().map(Fixture::plan).collect();
        let commits = std::cell::Cell::new(0);
        let mut progress = Vec::new();

        copy_batch_retaining_source(
            &plans,
            || {
                commits.set(commits.get() + 1);
                Ok(())
            },
            |done, total| {
                progress.push((done, total));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(commits.get(), 1);
        assert_eq!(progress, vec![(0, 0)]);
        assert!(fixtures
            .iter()
            .all(|fixture| fs::read_dir(fixture.new_dir()).unwrap().next().is_none()));
    }

    #[cfg(unix)]
    #[test]
    fn account_copy_rejects_replaced_root_after_confirmation() {
        let f = Fixture::new();
        let plan = f.plan();
        fs::rename(f.new_dir(), f.0.join("held-target")).unwrap();
        fs::create_dir(f.new_dir()).unwrap();
        assert!(plan.copy_retaining_source(|| panic!("replacement must not commit"), |_, _| {}).is_err());
        assert_eq!(fs::read_dir(f.new_dir()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn account_copy_refuses_leaf_symlink_installed_after_validation() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let plan = f.plan();
        let private = f.0.join("other-user-file");
        fs::write(&private, b"must never copy").unwrap();
        let mut swapped = false;
        assert!(plan.copy_retaining_source(|| panic!("link must not commit"), |_, _| {
            if !swapped {
                swapped = true;
                fs::remove_file(f.old().join(".hidden")).unwrap();
                symlink(&private, f.old().join(".hidden")).unwrap();
            }
        }).is_err());
        assert!(!f.new_dir().join(".hidden").exists());
        assert_eq!(fs::read(&private).unwrap(), b"must never copy");
    }

    #[cfg(unix)]
    #[test]
    fn account_copy_does_not_write_through_replaced_destination_parent() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let plan = f.plan();
        let external = f.0.join("external"); fs::create_dir(&external).unwrap();
        let mut swapped = false;
        assert!(plan.copy_retaining_source(|| panic!("replaced target must not commit"), |_, _| {
            if !swapped {
                swapped = true;
                fs::rename(f.new_dir(), f.0.join("held-target")).unwrap();
                symlink(&external, f.new_dir()).unwrap();
            }
        }).is_err());
        assert_eq!(fs::read_dir(external).unwrap().count(), 0);
    }

    #[test]
    fn account_copy_large_gallery_does_not_retain_each_source_file() {
        let f = Fixture::new();
        for i in 0..600 { fs::write(f.old().join(format!("image-{i:04}")), b"pixels").unwrap(); }
        let plan = f.plan();
        assert_eq!(plan.retained.entries.len(), 604);
        // RetainedEntry contains only identity/stamp metadata; roots alone own
        // File handles. Copy opens at most two file handles plus ancestor chains.
        plan.copy_retaining_source(|| Ok(()), |_, _| {}).unwrap();
        assert_eq!(fs::read(f.new_dir().join("image-0599")).unwrap(), b"pixels");
    }

    #[test]
    fn account_copy_retains_originals_after_commit() {
        let f = Fixture::new();
        let committed = std::cell::Cell::new(false);
        f.plan().copy_retaining_source(|| {
            assert_eq!(fs::read(f.new_dir().join("nested/作品.png"))?, b"image bytes");
            committed.set(true);
            Ok(())
        }, |_, _| {}).unwrap();
        assert!(committed.get());
        for root in [f.old(), f.new_dir()] {
            assert_eq!(fs::read(root.join("nested/作品.png")).unwrap(), b"image bytes");
            assert_eq!(fs::read(root.join(".hidden")).unwrap(), b"hidden bytes");
            assert!(root.join("nested/empty").is_dir());
        }
    }

    #[test]
    fn account_copy_commit_failure_preserves_source_and_partial_destination() {
        let f = Fixture::new();
        fs::write(f.new_dir().join("unrelated.txt"), b"keep").unwrap();
        assert!(f.plan().copy_retaining_source(
            || Err(io::Error::other("mapping commit failed")), |_, _| {},
        ).is_err());
        assert_eq!(fs::read(f.old().join("nested/作品.png")).unwrap(), b"image bytes");
        assert_eq!(fs::read(f.new_dir().join("unrelated.txt")).unwrap(), b"keep");
        assert_eq!(fs::read(f.new_dir().join("nested/作品.png")).unwrap(), b"image bytes");
        assert_eq!(fs::read(f.new_dir().join(".hidden")).unwrap(), b"hidden bytes");
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
    fn legacy_cleanup_rejects_conflicts_before_changing_any_files() {
        let f = Fixture::new();
        fs::write(f.new_dir().join(".hidden"), b"destination bytes").unwrap();
        assert!(f.plan().execute(|| panic!("must not commit"), |_, _| {}).is_err());
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
