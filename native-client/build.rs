fn main() {
    println!("cargo:rerun-if-env-changed=ELUNVI_BUILD_CHANNEL");
    let channel = std::env::var("ELUNVI_BUILD_CHANNEL").unwrap_or_else(|_| "local".into());
    assert!(matches!(channel.as_str(), "local" | "release"), "invalid ELUNVI_BUILD_CHANNEL");
    println!("cargo:rustc-env=ELUNVI_BUILD_CHANNEL={channel}");
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
    res.set("ProductName", "Elunvi Canvas");
    res.set("FileDescription", "Elunvi Canvas");
    res.compile().expect("embed Windows application icon");
}

#[cfg(not(target_os = "windows"))]
fn embed_windows_resources() {}
