mod directory_migration;
mod drag_preview;
mod image_formats;
mod platform;
mod restart;
mod runtime;
mod startup;

pub fn run() -> anyhow::Result<()> {
    startup::run()
}
