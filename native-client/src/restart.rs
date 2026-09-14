//! Restart handoff: the replacement must wait until the old process releases its locks.
use std::ffi::OsString;
use std::io;
use std::time::Duration;

const RESTART_ARGUMENT: &str = "--elunvi-restart-after";

fn restart_parent(args: impl IntoIterator<Item = OsString>) -> io::Result<Option<u32>> {
    let mut args = args.into_iter().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(RESTART_ARGUMENT)) {
        return Ok(None);
    }
    let pid = args
        .next()
        .and_then(|value| value.to_str().and_then(|text| text.parse::<u32>().ok()))
        .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32);
    if pid.is_none() || args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid restart handoff",
        ));
    }
    Ok(pid)
}

pub(crate) fn wait_if_requested() -> io::Result<()> {
    if let Some(pid) = restart_parent(std::env::args_os())? {
        wait_for_parent_exit(pid, Duration::from_secs(60))?;
    }
    Ok(())
}

pub(crate) fn spawn_waiting_child() -> io::Result<std::process::Child> {
    let executable = std::env::current_exe()?;
    let mut command = std::process::Command::new(&executable);
    command
        .arg(RESTART_ARGUMENT)
        .arg(std::process::id().to_string());
    if let Some(parent) = executable.parent() {
        command.current_dir(parent);
    }
    command.spawn()
}

fn wait_for_parent_exit(pid: u32, timeout: Duration) -> io::Result<()> {
    if pid == 0 || pid == std::process::id() || pid > i32::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid restart parent",
        ));
    }
    wait_for_process(pid, timeout)
}

#[cfg(windows)]
fn wait_for_process(pid: u32, timeout: Duration) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
    }
    // SYNCHRONIZE only: the replacement can wait, never terminate the old client.
    let raw = unsafe { OpenProcess(0x00100000, 0, pid) };
    if raw.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(87) {
            Ok(())
        } else {
            Err(error)
        };
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    match unsafe {
        WaitForSingleObject(
            handle.as_raw_handle(),
            timeout.as_millis().min(u32::MAX as u128 - 1) as u32,
        )
    } {
        0 => Ok(()),
        258 => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "previous client is still closing",
        )),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(unix)]
fn wait_for_process(pid: u32, timeout: Duration) -> io::Result<()> {
    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    let started = std::time::Instant::now();
    loop {
        // Signal zero probes liveness; it does not send a signal or kill anything.
        if unsafe { kill(pid as i32, 0) } != 0 {
            let error = io::Error::last_os_error();
            // ESRCH is 3 on the supported macOS and Linux targets.
            return if error.raw_os_error() == Some(3) {
                Ok(())
            } else {
                Err(error)
            };
        }
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "previous client is still closing",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(any(unix, windows)))]
fn wait_for_process(_: u32, _: Duration) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "restart is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_arguments_require_one_valid_parent_and_leave_normal_launch_alone() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(restart_parent(args(&["app"])).unwrap(), None);
        assert_eq!(
            restart_parent(args(&["app", "--elunvi-restart-after", "42"])).unwrap(),
            Some(42)
        );
        for values in [
            vec!["app", "--elunvi-restart-after"],
            vec!["app", "--elunvi-restart-after", "0"],
            vec!["app", "--elunvi-restart-after", "bad"],
            vec!["app", "--elunvi-restart-after", "42", "extra"],
        ] {
            assert!(restart_parent(args(&values)).is_err());
        }
    }

    #[test]
    fn restart_wait_rejects_the_current_process() {
        assert!(wait_for_parent_exit(std::process::id(), Duration::from_millis(10)).is_err());
    }

    #[test]
    fn restart_wait_does_not_open_the_new_instance_before_old_process_exits() {
        #[cfg(windows)]
        let mut child = {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Start-Sleep -Milliseconds 500",
                ])
                .creation_flags(0x08000000)
                .spawn()
                .unwrap()
        };
        #[cfg(unix)]
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 0.5"])
            .spawn()
            .unwrap();
        let pid = child.id();
        let reaper = std::thread::spawn(move || child.wait().unwrap());
        assert_eq!(
            wait_for_parent_exit(pid, Duration::from_millis(10))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let started = std::time::Instant::now();
        let result = wait_for_parent_exit(pid, Duration::from_secs(10));
        let elapsed = started.elapsed();
        assert!(reaper.join().unwrap().success());
        result.unwrap();
        assert!(
            elapsed >= Duration::from_millis(300),
            "must wait for the previous process, not race its database locks"
        );
    }
}
