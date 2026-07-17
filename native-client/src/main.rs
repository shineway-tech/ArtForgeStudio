#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    artforge_studio_native::run()
}
