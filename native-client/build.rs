fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.compile().expect("embed Windows application icon");
    }

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
