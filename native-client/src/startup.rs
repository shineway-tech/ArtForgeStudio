use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Stage {
    Restart,
    Instance,
    Window,
    Data,
    Tray,
    Services,
    EventLoop,
    Running,
}

impl Stage {
    fn current() -> Self {
        match STAGE.load(Ordering::Relaxed) {
            1 => Self::Instance,
            2 => Self::Window,
            3 => Self::Data,
            4 => Self::Tray,
            5 => Self::Services,
            6 => Self::EventLoop,
            7 => Self::Running,
            _ => Self::Restart,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Restart => "等待旧客户端退出失败，请完全退出旧客户端后重试。",
            Self::Instance => "客户端可能已在运行，请检查任务栏或系统托盘。",
            Self::Window | Self::EventLoop => {
                "无法初始化显示窗口，请更新显卡驱动，或联系工作人员获取兼容性支持。"
            }
            Self::Data => "无法打开本地数据，请检查数据目录权限及磁盘空间，不要删除已有作品。",
            Self::Tray => "无法初始化系统托盘，请重启资源管理器或电脑后重试。",
            Self::Services => "无法初始化客户端服务，请联系工作人员并提供启动诊断日志。",
            Self::Running => "客户端运行异常，请重新打开并提供启动诊断日志。",
        }
    }
}

static STAGE: AtomicU8 = AtomicU8::new(Stage::Restart as u8);
static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
const MAX_LOG_BYTES: u64 = 256 * 1024;

pub(crate) fn set_stage(stage: Stage) {
    STAGE.store(stage as u8, Ordering::Relaxed);
    record("stage", stage, None);
}

pub(crate) fn run() -> anyhow::Result<()> {
    install_panic_hook();
    let explicit_backend = std::env::var_os("SLINT_BACKEND").is_some();
    if !explicit_backend {
        std::env::set_var(
            "SLINT_BACKEND",
            if cfg!(windows) {
                "winit-femtovg"
            } else {
                "winit-software"
            },
        );
    }
    set_stage(Stage::Restart);
    let result = crate::restart::wait_if_requested()
        .map_err(anyhow::Error::from)
        .and_then(|()| crate::runtime::run());
    if let Err(error) = &result {
        let stage = Stage::current();
        let window_error = error
            .chain()
            .any(|cause| cause.is::<slint::PlatformError>());
        if should_retry_software(cfg!(windows), explicit_backend, stage, window_error) {
            record("retry-software", stage, Some(error.as_ref()));
            let child = crate::restart::waiting_child_command()
                .and_then(|mut command| command.env("SLINT_BACKEND", "winit-software").spawn());
            match child {
                Ok(_) => return Ok(()),
                Err(error) => record("retry-failed", stage, Some(&error)),
            }
        }
        record("error", stage, Some(error.as_ref()));
        show_failure(stage);
    }
    result
}

fn should_retry_software(
    windows: bool,
    explicit_backend: bool,
    stage: Stage,
    window_error: bool,
) -> bool {
    windows
        && !explicit_backend
        && window_error
        && matches!(stage, Stage::Window | Stage::EventLoop)
}

fn log_path() -> Option<&'static Path> {
    LOG_PATH
        .get_or_init(|| {
            let local = std::env::var_os("LOCALAPPDATA")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from);
            local
                .into_iter()
                .chain(std::iter::once(std::env::temp_dir()))
                .map(|root| root.join("ElunviCanvas").join("logs"))
                .find_map(|directory| {
                    std::fs::create_dir_all(&directory).ok()?;
                    let path = directory.join("startup.log");
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .ok()?;
                    Some(path)
                })
        })
        .as_deref()
}

fn diagnostic_line(
    event: &str,
    stage: Stage,
    error: Option<&(dyn std::error::Error + 'static)>,
) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let io_code = error
        .and_then(|error| error.downcast_ref::<std::io::Error>())
        .and_then(std::io::Error::raw_os_error);
    format!(
        "time={timestamp} version={} os={} arch={} event={event} stage={stage:?} io_code={io_code:?}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn append_diagnostic(path: &Path, line: &str) -> std::io::Result<()> {
    let truncate = path
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES);
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(path)?
        .write_all(line.as_bytes())
}

fn record(event: &str, stage: Stage, error: Option<&(dyn std::error::Error + 'static)>) {
    if let Some(path) = log_path() {
        let _ = append_diagnostic(path, &diagnostic_line(event, stage, error));
    }
}

fn show_failure(stage: Stage) {
    let log_hint = log_path()
        .map(|path| format!("启动诊断日志：{}", path.display()))
        .unwrap_or_else(|| "无法保存启动诊断日志，请检查临时目录的写入权限。".into());
    let message = format!("{}\n\n{log_hint}", stage.description());
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "Elunvi Canvas".encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("{message}");
}

#[cfg(windows)]
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|_| {
        let stage = Stage::current();
        record("panic", stage, None);
        show_failure(stage);
    }));
}

#[cfg(not(windows))]
fn install_panic_hook() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_retry_is_limited_to_automatic_windows_window_startup() {
        for stage in [Stage::Window, Stage::EventLoop] {
            assert!(should_retry_software(true, false, stage, true));
            assert!(!should_retry_software(true, true, stage, true));
            assert!(!should_retry_software(false, false, stage, true));
            assert!(!should_retry_software(true, false, stage, false));
        }
        for stage in [
            Stage::Restart,
            Stage::Instance,
            Stage::Data,
            Stage::Tray,
            Stage::Services,
            Stage::Running,
        ] {
            assert!(!should_retry_software(true, false, stage, true));
        }
    }

    #[test]
    fn diagnostic_does_not_include_error_content() {
        let error = std::io::Error::other("private-token prompt payment-url response-body");
        let line = diagnostic_line("error", Stage::Data, Some(&error));
        assert!(line.contains("stage=Data"));
        for secret in ["private-token", "prompt", "payment-url", "response-body"] {
            assert!(!line.contains(secret));
        }
        let error = std::io::Error::from_raw_os_error(5);
        assert!(diagnostic_line("error", Stage::Data, Some(&error)).contains("io_code=Some(5)"));
    }

    #[test]
    fn diagnostics_append_and_remain_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("startup.log");
        append_diagnostic(&path, "first\n").unwrap();
        append_diagnostic(&path, "second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        std::fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();
        append_diagnostic(&path, "latest\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "latest\n");
    }
}
