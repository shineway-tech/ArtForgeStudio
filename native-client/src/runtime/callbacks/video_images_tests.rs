use super::*;

fn app() -> AppWindow {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    wire_video_generation_callbacks(&app, AppContext::default());
    app.global::<AppState>().set_page("video-generation".into());
    app
}

fn png(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    image::RgbaImage::from_pixel(80, 120, image::Rgba([44, 106, 190, 255]))
        .save(&path)
        .unwrap();
    path
}

fn prepared(paths: &[PathBuf]) -> PreparedVideoImages {
    prepare_video_images(
        paths
            .iter()
            .map(|path| {
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    path.clone(),
                )
            })
            .collect(),
        || true,
    )
}

#[test]
fn asset_candidates_come_from_the_full_saved_library_not_generation_history() {
    let context = AppContext::default();
    let mut store = context.store.borrow_mut();
    let asset = |id: &str, path: &str| AssetData {
        id: id.into(),
        conversation_id: String::new(),
        title: id.into(),
        category: "other".into(),
        kind: "game".into(),
        time: String::new(),
        prompt: String::new(),
        ratio: "1:1".into(),
        quality: String::new(),
        model: String::new(),
        origin: String::new(),
        width: 80,
        height: 120,
        source_path: path.into(),
        reference_paths: vec![],
        cutout_done: false,
        remove_black_done: false,
        upscale_done: false,
        is_new: false,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    store.assets = vec![
        asset("saved-one", "one.png"),
        asset("saved-two", "two.png"),
        asset("failed", "failed"),
    ];
    store.generations = vec![asset("history-only", "history.png")];
    let candidates = video_asset_candidates(&store);
    assert_eq!(
        candidates,
        vec![
            ("saved-one".into(), PathBuf::from("one.png")),
            ("saved-two".into(), PathBuf::from("two.png"))
        ]
    );
    assert_eq!(store.assets.len(), 3);
}

#[test]
fn local_import_accepts_multiple_images_but_skips_duplicates_and_unreadable_files() {
    let dir = tempfile::tempdir().unwrap();
    let first = png(dir.path(), "first.png");
    let second = png(dir.path(), "second.png");
    let broken = dir.path().join("broken.png");
    fs::write(&broken, b"not an image").unwrap();
    let result = prepared(&[
        first.clone(),
        second,
        dir.path().join("./first.png"),
        broken,
        dir.path().join("missing.png"),
    ]);
    assert_eq!(
        result
            .images
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>(),
        ["first.png", "second.png"]
    );
    assert_eq!(result.skipped, 2);
    assert!(first.is_file());
}

#[test]
fn imports_append_without_overwriting_prompt_and_removal_never_deletes_files() {
    let app = app();
    let state = app.global::<AppState>();
    let dir = tempfile::tempdir().unwrap();
    let first = png(dir.path(), "first.png");
    let second = png(dir.path(), "second.png");
    let quote_epoch = AtomicU64::new(7);
    let request_id = Mutex::new("old-request".to_string());
    state.set_video_prompt("User edited video prompt".into());
    let (rows, skipped) = materialize_video_images(prepared(&[first.clone()]));
    append_video_images(&state, rows, skipped, &quote_epoch, &request_id);
    state.set_video_quote_ready(true);
    state.set_video_quote_id("old-quote".into());
    state.set_video_source_file_id("old-file".into());
    let (rows, skipped) = materialize_video_images(prepared(&[first.clone(), second.clone()]));
    append_video_images(&state, rows, skipped, &quote_epoch, &request_id);
    assert_eq!(state.get_video_images().row_count(), 2);
    assert_eq!(state.get_video_prompt(), "User edited video prompt");
    assert!(!state.get_video_quote_ready());
    assert_eq!(state.get_video_quote_id(), "");
    assert_eq!(state.get_video_source_file_id(), "");
    assert_eq!(quote_epoch.load(Ordering::SeqCst), 9);
    assert_eq!(*request_id.lock().unwrap(), "");
    assert!(
        video_image_generation_error(&state).is_some(),
        "multiple images must not silently submit just the first"
    );
    state.invoke_remove_video_image(video_image_key(&first).into());
    assert_eq!(state.get_video_images().row_count(), 1);
    assert_eq!(
        state.get_video_source_path().as_str(),
        second.to_string_lossy()
    );
    assert_eq!(video_image_generation_error(&state), None);
    state.invoke_remove_video_image(video_image_key(&second).into());
    assert_eq!(state.get_video_images().row_count(), 0);
    assert!(video_image_generation_error(&state).is_some());
    assert!(first.is_file() && second.is_file());
}

#[test]
fn asset_picker_multiselect_adds_all_checked_images_and_marks_existing_images() {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::PointerEventButton;
    let app = app();
    let state = app.global::<AppState>();
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        png(dir.path(), "one.png"),
        png(dir.path(), "two.png"),
        png(dir.path(), "three.png"),
    ];
    let epoch = AtomicU64::new(1);
    let quote_epoch = AtomicU64::new(0);
    let request_id = Mutex::new(String::new());
    let (rows, skipped) = materialize_video_images(prepared(&paths[..1]));
    append_video_images(&state, rows, skipped, &quote_epoch, &request_id);
    state.set_video_image_dialog("assets".into());
    finish_video_image_import(
        &state,
        prepared(&paths),
        true,
        &epoch,
        1,
        &quote_epoch,
        &request_id,
    );
    assert!(state.get_video_asset_choices().row_data(0).unwrap().added);
    state.invoke_toggle_video_asset(video_image_key(&paths[0]).into());
    assert_eq!(state.get_video_asset_selected_count(), 0);
    app.window()
        .set_size(slint::LogicalSize::new(1180.0, 760.0));
    app.show().unwrap();
    for label in ["two.png", "three.png"] {
        ElementHandle::find_by_accessible_label(&app, label)
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
    }
    assert_eq!(state.get_video_asset_selected_count(), 2);
    state.invoke_confirm_video_assets();
    assert_eq!(state.get_video_image_dialog(), "");
    assert_eq!(state.get_video_images().row_count(), 3);
    assert_eq!(
        state
            .get_video_images()
            .iter()
            .map(|row| row.title.to_string())
            .collect::<Vec<_>>(),
        ["one.png", "two.png", "three.png"]
    );
    assert!(paths.iter().all(|path| path.is_file()));
    if let Some(folder) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
        fs::create_dir_all(&folder).unwrap();
        let pixels = app.window().take_snapshot().unwrap();
        image::save_buffer(
            PathBuf::from(folder).join("video-image-grid.png"),
            pixels.as_bytes(),
            pixels.width(),
            pixels.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

#[test]
fn cancelled_or_previous_page_imports_cannot_replace_the_current_images() {
    let app = app();
    let state = app.global::<AppState>();
    let dir = tempfile::tempdir().unwrap();
    let paths = [png(dir.path(), "old.png")];
    let epoch = AtomicU64::new(1);
    let quote_epoch = AtomicU64::new(0);
    let request_id = Mutex::new(String::new());
    cancel_video_image_work(&state, &epoch);
    state.set_video_images_loading(true);
    finish_video_image_import(
        &state,
        prepared(&paths),
        false,
        &epoch,
        1,
        &quote_epoch,
        &request_id,
    );
    assert_eq!(state.get_video_images().row_count(), 0);
    assert!(
        state.get_video_images_loading(),
        "a stale import must not clear a new import's loading state"
    );
    state.set_page("assets".into());
    finish_video_image_import(
        &state,
        prepared(&paths),
        false,
        &epoch,
        2,
        &quote_epoch,
        &request_id,
    );
    assert_eq!(state.get_video_images().row_count(), 0);
}

#[test]
fn cancelled_asset_selection_does_not_add_images() {
    let app = app();
    let state = app.global::<AppState>();
    let dir = tempfile::tempdir().unwrap();
    let paths = [png(dir.path(), "one.png")];
    let (mut rows, _) = materialize_video_images(prepared(&paths));
    rows[0].selected = true;
    state.set_video_image_dialog("assets".into());
    state.set_video_asset_choices(ModelRc::new(VecModel::from(rows)));
    state.invoke_close_video_image_dialog();
    state.invoke_confirm_video_assets();
    assert_eq!(state.get_video_images().row_count(), 0);
    assert_eq!(state.get_video_image_dialog(), "");
}

#[test]
fn multi_image_submit_is_blocked_even_when_a_stale_quote_is_marked_ready() {
    let app = app();
    let state = app.global::<AppState>();
    state.set_video_images(ModelRc::new(VecModel::from(vec![
        VideoImageItem::default();
        2
    ])));
    state.set_video_quote_ready(true);
    state.set_video_quote_id("stale-quote".into());
    state.set_video_prompt("Video prompt".into());
    state.invoke_submit_video_generation();
    assert!(!state.get_video_generating());
    assert!(state.get_video_status().contains("单图"));
    state.invoke_request_video_quote("16:9".into(), "720P".into(), 4);
    assert!(!state.get_video_quote_ready());
    assert_eq!(state.get_video_quote_id(), "");
    assert!(!state.get_video_quote_loading());
}
