mod drag_preview;
mod image_formats;
mod platform;
mod runtime;

pub fn run() -> anyhow::Result<()> {
    runtime::run()
}
