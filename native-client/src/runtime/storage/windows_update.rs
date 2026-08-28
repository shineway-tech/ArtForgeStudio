use super::*;
use std::os::windows::process::CommandExt;
use std::process::{Child, Stdio};

pub(super) struct Handoff {
    child: Child,
    status_path: PathBuf,
}

fn powershell_command() -> Command {
    let executable =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(executable);
    // PowerShell 7 module paths inherited through the app can break Windows PowerShell 5.1.
    command.env_remove("PSModulePath");
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for the supervisor, not the UI.
    command
}

pub(super) fn launch(package: &Path, version: &str, sha256: &str) -> Result<Handoff> {
    let update_dir = package
        .parent()
        .ok_or_else(|| anyhow!("更新临时目录无效"))?;
    let helper_path = update_dir.join("install-update.ps1");
    let status_path = update_dir.join("helper-status.txt");
    let current_exe = std::env::current_exe().context("无法确定当前程序位置")?;
    let mut helper = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&helper_path)?;
    helper.write_all(include_bytes!("windows-update.ps1"))?;
    helper.sync_all()?;
    drop(helper);
    let child = powershell_command()
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&helper_path)
        .arg("-PackagePath").arg(package)
        .arg("-AppExePath").arg(&current_exe)
        .arg("-ExpectedVersion").arg(version)
        .arg("-ExpectedSha256").arg(sha256)
        .arg("-ParentProcessId").arg(std::process::id().to_string())
        .arg("-StatusPath").arg(&status_path)
        .arg("-ResultPath").arg(app_data_dir().join("update-result.json"))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().context("无法启动 Windows 更新辅助程序")?;
    Ok(Handoff { child, status_path })
}

pub(super) fn poll(weak: Weak<AppWindow>, handoff: Rc<RefCell<Handoff>>, attempts: u16) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let Some(app) = weak.upgrade() else { return };
        let mut pending = handoff.borrow_mut();
        let status = fs::read_to_string(&pending.status_path).unwrap_or_default();
        let running = matches!(pending.child.try_wait(), Ok(None));
        if status.trim() == "ready" && running {
            let _ = app.window().hide();
            let _ = slint::quit_event_loop();
            return;
        }
        if status.trim() == "failed" || !running || attempts == 0 {
            if running {
                let _ = pending.child.kill();
                let _ = pending.child.wait();
            }
            let state = app.global::<AppState>();
            state.set_update_stage("failed".into());
            state.set_update_download_message(
                "更新辅助程序未能准备就绪，当前程序未关闭。请检查目录权限后重试。".into(),
            );
            return;
        }
        drop(pending);
        poll(weak, handoff, attempts - 1);
    });
}

#[derive(Deserialize)]
struct Receipt {
    schema: u32,
    target: String,
    version: String,
    status: String,
}

fn failure_message(
    text: &str,
    current_exe: &Path,
    current_version: &str,
) -> Option<(String, &'static str)> {
    let receipt: Receipt = serde_json::from_str(text).ok()?;
    if receipt.schema != 1
        || !receipt
            .target
            .eq_ignore_ascii_case(&current_exe.to_string_lossy())
        || !compare_versions(&receipt.version, current_version).is_gt()
    {
        return None;
    }
    let message = match receipt.status.as_str() {
        "install_failed" => {
            "上次更新安装失败，已重新打开当前程序，版本未升级。请检查目录权限或磁盘空间后重试。"
        }
        "version_mismatch" | "installed" => {
            "上次安装后版本校验未通过，当前程序仍不是目标版本。请重试更新。"
        }
        "restart_failed" => "上次更新未能自动重新打开程序。当前版本尚未升级，请重试更新。",
        "reboot_required" => "安装器报告需要重新启动电脑才能完成更新，请先保存工作再手动重启。",
        _ => return None,
    };
    Some((receipt.version, message))
}

pub(super) fn restore_result(app: &AppWindow) {
    let Ok(text) = fs::read_to_string(app_data_dir().join("update-result.json")) else {
        return;
    };
    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };
    let Some((version, message)) = failure_message(&text, &current_exe, env!("CARGO_PKG_VERSION"))
    else {
        return;
    };
    let state = app.global::<AppState>();
    state.set_latest_version(version.into());
    state.set_update_available(true);
    state.set_update_stage("failed".into());
    state.set_update_download_message(message.into());
    state.set_update_result_open(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_update_supervisor_installs_restarts_and_reports_failures_in_fixtures() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/tests/windows-update-helper.ps1");
        let result = powershell_command()
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(script)
            .output()
            .expect("run Windows update supervisor fixtures");
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn failed_update_receipt_is_only_shown_for_the_affected_old_copy() {
        let path = Path::new(r"E:\Elunvi Canvas\ElunviCanvas.exe");
        let receipt = serde_json::json!({
            "schema": 1, "target": path, "version": "1.0.22", "status": "install_failed"
        })
        .to_string();
        assert!(failure_message(&receipt, path, "1.0.21")
            .unwrap()
            .1
            .contains("安装失败"));
        assert!(failure_message(&receipt, path, "1.0.22").is_none());
        assert!(
            failure_message(&receipt, Path::new(r"C:\Other\ElunviCanvas.exe"), "1.0.21").is_none()
        );
        assert!(failure_message("not json", path, "1.0.21").is_none());
    }
}
