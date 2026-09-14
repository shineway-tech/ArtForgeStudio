mod drag_preview;
mod directory_migration;
mod image_formats;
mod platform;
mod restart;
mod runtime;

pub fn run() -> anyhow::Result<()> {
    restart::wait_if_requested()?;
    runtime::run()
}
