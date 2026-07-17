#[cfg(target_os = "macos")]
pub(crate) fn schedule_application_icon_install() {
    slint::Timer::single_shot(std::time::Duration::ZERO, || {
        if let Err(error) = install_macos_app_icon() {
            eprintln!("failed to install macOS application icon: {error:#}");
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn schedule_application_icon_install() {}

#[cfg(target_os = "macos")]
fn install_macos_app_icon() -> anyhow::Result<()> {
    use anyhow::{anyhow, Context};
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| anyhow!("macOS application icon must be installed on the main thread"))?;
    let icon_data = NSData::with_bytes(include_bytes!("../assets/app-icon.png"));
    let icon = NSImage::initWithData(NSImage::alloc(), &icon_data)
        .context("decode embedded macOS application icon")?;
    let application = NSApplication::sharedApplication(main_thread);

    // SAFETY: AppKit retains the supplied NSImage and this runs on the main thread.
    unsafe { application.setApplicationIconImage(Some(&icon)) };
    Ok(())
}
