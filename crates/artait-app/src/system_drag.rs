use std::path::Path;

#[cfg(windows)]
pub fn start_file_drag(path: &Path) -> std::io::Result<()> {
    windows::start(path)
}

#[cfg(not(windows))]
pub fn start_file_drag(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "system file drag is only implemented on Windows",
    ))
}

#[cfg(windows)]
mod windows;
