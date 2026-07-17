fn main() {
    embed_windows_resources();

    std::thread::Builder::new()
        .name("slint-build".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            std::env::set_var("SLINT_ENABLE_EXPERIMENTAL_FEATURES", "1");
            slint_build::compile("ui/app.slint").expect("compile Slint UI");
        })
        .expect("start Slint build thread")
        .join()
        .expect("Slint build thread panicked");
}

#[cfg(target_os = "windows")]
fn embed_windows_resources() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/app.ico");
    res.compile().expect("embed Windows application icon");
}

#[cfg(not(target_os = "windows"))]
fn embed_windows_resources() {}
