use super::*;

#[test]
fn core_video_tga_hint_header_rejects_huge_dimensions_before_pixel_decode() {
    let mut bytes=Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3,5,image::Rgb([20,40,80])))
        .write_to(&mut Cursor::new(&mut bytes),image::ImageFormat::Tga).unwrap();
    validate_video_image_header(Path::new("missing.tga"),&bytes).unwrap();
    assert_eq!(decode_image_bytes(Path::new("missing.tga"),&bytes).unwrap().0.width(),3);
    let mut header=vec![0u8;18]; header[2]=2; header[12..14].copy_from_slice(&12000u16.to_le_bytes());
    header[14..16].copy_from_slice(&12000u16.to_le_bytes()); header[16]=24;
    let error=validate_video_image_header(Path::new("missing.tga"),&header).unwrap_err();
    assert!(error.to_string().contains("dimensions exceed policy"));
}

fn app() -> (scoped_inputs::Fixture, AppWindow) {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        },
    )))
    .unwrap();
    let app = AppWindow::new().unwrap();
    let fixture=scoped_inputs::Fixture::new();
    wire_video_generation_callbacks(&app, fixture.context.clone());
    app.global::<AppState>().set_page("video-generation".into());
    (fixture, app)
}

fn png(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    image::RgbaImage::from_pixel(80, 120, image::Rgba([44, 106, 190, 255]))
        .save(&path)
        .unwrap();
    path
}

fn prepared(fixture:&scoped_inputs::Fixture, paths: &[PathBuf]) -> PreparedVideoImages {
    let persistence=fixture.persistence.clone();
    let candidates=paths.iter().map(|path|(path.file_name().unwrap().to_string_lossy().into_owned(),path.clone())).collect();
    std::thread::scope(|threads|threads.spawn(move||prepare_video_images(&persistence,candidates,||true)).join().unwrap())
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
    let _fixture=scoped_inputs::Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let first = png(dir.path(), "first.png");
    let second = png(dir.path(), "second.png");
    let broken = dir.path().join("broken.png");
    fs::write(&broken, b"not an image").unwrap();
    let result = prepared(&_fixture, &[
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
    let (_fixture, app) = app();
    let state = app.global::<AppState>();
    let dir = tempfile::tempdir().unwrap();
    let first = png(dir.path(), "first.png");
    let second = png(dir.path(), "second.png");
    let quote_epoch = AtomicU64::new(7);
    let request_id = Mutex::new("old-request".to_string());
    state.set_video_prompt("User edited video prompt".into());
    let (rows, skipped) = materialize_video_images(prepared(&_fixture, &[first.clone()]));
    append_video_images(&state, rows, skipped, &quote_epoch, &request_id);
    state.set_video_quote_ready(true);
    state.set_video_quote_id("old-quote".into());
    state.set_video_source_file_id("old-file".into());
    let (rows, skipped) = materialize_video_images(prepared(&_fixture, &[first.clone(), second.clone()]));
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
    assert!(_fixture.persistence.owns_path(Path::new(state.get_video_source_path().as_str())));
    assert_ne!(state.get_video_source_path().as_str(),second.to_string_lossy());
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
    let (_fixture, app) = app();
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
    let (rows, skipped) = materialize_video_images(prepared(&_fixture, &paths[..1]));
    append_video_images(&state, rows, skipped, &quote_epoch, &request_id);
    state.invoke_open_video_asset_picker();
    scoped_inputs::pump(||!state.get_video_images_loading());
    state.set_video_image_dialog("assets".into());
    finish_video_image_import(
        &state,
        prepared(&_fixture, &paths),
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
    let (_fixture, app) = app();
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
        prepared(&_fixture, &paths),
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
        prepared(&_fixture, &paths),
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
    let (_fixture, app) = app();
    let state = app.global::<AppState>();
    let dir = tempfile::tempdir().unwrap();
    let paths = [png(dir.path(), "one.png")];
    let (mut rows, _) = materialize_video_images(prepared(&_fixture, &paths));
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
    let (_fixture, app) = app();
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

pub(in crate::runtime) mod scoped_inputs {
    use super::*;
    pub(in crate::runtime) struct Fixture {
        pub context: AppContext,
        pub persistence: PrivatePersistence,
        pub authority: Arc<NamespaceStorageAuthority>,
        pub writer: client_state::tests::Fixture,
        _index_root: tempfile::TempDir,
    }
    impl Fixture {
        pub(in crate::runtime) fn reject_notification_inserts_for_test(&self) {
            self.writer.reject_notification_inserts_for_test();
        }
        pub(in crate::runtime) fn new() -> Self {
            let writer = client_state::tests::Fixture::new(false, false);
            let index_root = tempfile::tempdir().unwrap();
            let session = Arc::new(SessionManager::new(Arc::new(crate::runtime::test_support::MemoryRefreshTokenStore::default())));
            let scope = session.install_tokens_for_user(&TokenSet {
                access_token:"input-access".into(),access_expires_in_seconds:1800,refresh_token:"input-refresh".into(),
                refresh_expires_at:"2099-01-01T00:00:00Z".into(),token_type:"X-Token".into(),
            }, "11111111-1111-4111-8111-111111111111").unwrap();
            let lease = writer.lease(&scope.owner_user_id, scope.auth_epoch, 1);
            writer.activate(lease.clone()).unwrap();
            let root = writer.data_root_capability_arc();
            let index = FileIndex::initialize(index_root.path().join("input-index.sqlite3")).unwrap();
            let backend = Arc::new(BackendRuntime { api: ApiClient::new(ApiClientConfig {
                base_url:reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),app_version:"999.0.0".into(),timeout:Duration::from_millis(50),
            }, DeviceIdentity { id:Uuid::new_v4().to_string(),name:"input-fixture".into(),platform:"macos".into() },session).unwrap() });
            let context = AppContext {
                backend:Some(backend.clone()),data_root_capability:Some(root.clone()),file_index:Some(index.clone()),
                current_user_id:Arc::new(Mutex::new(Some(scope.owner_user_id))),
                ..Default::default()
            };
            context.user_activity.activate(lease.clone()).unwrap();
            *context.active_namespace.lock().unwrap() = Some(lease.clone());
            backend.api.bind_user_work(UserWorkAdmission::new(context.active_namespace.clone(),context.user_activity.clone())).unwrap();
            let persistence = PrivatePersistence::for_test_with_storage((*writer).clone(),lease.clone(),
                context.user_activity.clone(),backend.api.upgrade_latch().clone(),root,backend.api.clone(),index);
            context.store.borrow_mut().private_persistence = Some(persistence.clone());
            let authority = persistence.storage_authority().unwrap();
            Self { context,persistence,authority,writer,_index_root:index_root }
        }
        pub(super) fn owned(&self, path: &Path) -> PathBuf {
            let authority=self.authority.clone();
            std::thread::scope(|threads| threads.spawn(move || {
                let bytes = authority.read_image_source(path, MAX_VIDEO_IMAGE_BYTES).unwrap();
                persist_reference_image_for_namespace(&authority,&decode_reference_bytes(&bytes).unwrap()).unwrap()
            }).join().unwrap())
        }
        pub(in crate::runtime) fn drain(&self) {
            let workers=drain_delivery_commit_workers_for_lease_for_test(self.authority.lease());
            self.context.user_activity.begin_quiesce(self.authority.lease()).unwrap().retire();
            workers.unwrap();
        }
    }
    fn setup() -> (Fixture,AppWindow,Arc<AtomicU64>,Arc<AtomicU64>,Arc<Mutex<String>>) {
        i_slint_backend_testing::init_no_event_loop();
        let f = Fixture::new();
        let app = AppWindow::new().unwrap();
        app.global::<AppState>().set_page("video-generation".into());
        let quote = Arc::new(AtomicU64::new(5));
        let key = Arc::new(Mutex::new("retained-key".into()));
        let epoch = wire_video_image_callbacks(&app,f.context.store.clone(),quote.clone(),key.clone());
        (f,app,epoch,quote,key)
    }
    pub(in crate::runtime) fn pump(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now()+Duration::from_secs(6);
        while !predicate() && Instant::now()<deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(predicate(),"captured video-image completion missing");
    }
    pub(super) fn asset(path: &Path) -> AssetData {
        AssetData {
            id:"asset".into(),conversation_id:String::new(),title:"source.png".into(),category:"other".into(),kind:"game".into(),
            time:String::new(),prompt:String::new(),ratio:"1:1".into(),quality:String::new(),model:String::new(),origin:String::new(),
            width:80,height:120,source_path:path.to_string_lossy().into_owned(),reference_paths:vec![],cutout_done:false,
            remove_black_done:false,upscale_done:false,is_new:false,delivery_recoverable:false,delivery_downloading:false,
        }
    }
    #[test]
    fn core_video_removing_the_viewer_seed_clears_original_source_association() {
        let (f,app,_epoch,_quote,_key)=setup();
        let input=tempfile::tempdir().unwrap();
        let source=f.owned(&png(input.path(),"viewer-seed.png"));
        let state=app.global::<AppState>();
        state.set_video_source_id("original-viewer-A".into());
        state.set_video_images(ModelRc::new(VecModel::from(vec![VideoImageItem {
            id:"captured-row".into(),source_path:source.to_string_lossy().into_owned().into(),..Default::default()
        }])));
        state.invoke_remove_video_image("captured-row".into());
        assert_eq!(state.get_video_images().row_count(),0);
        assert_eq!(state.get_video_source_id(),"","removed A must not own the next chosen source");
        assert!(source.is_file());
        f.drain();
    }
    #[test]
    fn core_video_asset_picker_binds_selected_asset_not_previous_viewer() {
        let (f,app,_epoch,_quote,_key)=setup();
        let input=tempfile::tempdir().unwrap();
        let source=f.owned(&png(input.path(),"asset-B.png"));
        let mut selected=asset(&source); selected.id="selected-asset-B".into();
        f.context.store.borrow_mut().assets.push(selected);
        let state=app.global::<AppState>();
        state.set_video_source_id("previous-viewer-A".into());
        state.on_request_video_quote(|_,_,_| {});
        state.invoke_open_video_asset_picker();
        pump(|| !state.get_video_images_loading());
        let row=state.get_video_asset_choices().row_data(0).expect("owned B choice");
        state.invoke_toggle_video_asset(row.id); state.invoke_confirm_video_assets();
        assert_eq!(state.get_video_images().row_count(),1);
        assert_eq!(state.get_video_source_id(),"selected-asset-B");
        assert!(source.is_file());
        f.drain();
    }
    #[test]
    fn video_image_remove_is_denied_after_exact_upgrade_without_invalidating_quote() {
        let (f,app,_epoch,quote,key)=setup();
        let input=tempfile::tempdir().unwrap();
        let source=f.owned(&png(input.path(),"upgrade.png"));
        app.global::<AppState>().set_video_images(ModelRc::new(VecModel::from(vec![VideoImageItem {
            id:"A".into(),source_path:source.to_string_lossy().into_owned().into(),..Default::default()
        }])));
        f.persistence.upgrade_latch().trip(RequiredUpgrade { minimum_version:Some("99.0.0".into()) });
        app.global::<AppState>().invoke_remove_video_image("A".into());
        assert_eq!(app.global::<AppState>().get_video_images().row_count(),1);
        assert_eq!(quote.load(Ordering::SeqCst),5);
        assert_eq!(key.lock().unwrap().as_str(),"retained-key");
        f.drain();
    }
    #[test]
    fn video_asset_confirmation_cannot_move_old_dialog_pixels_into_replacement_store() {
        let (a,app,_epoch,_quote,_key)=setup();
        app.global::<AppState>().invoke_open_video_asset_picker();
        a.drain();
        app.global::<AppState>().set_video_images_loading(false);
        app.global::<AppState>().set_video_asset_choices(ModelRc::new(VecModel::from(vec![VideoImageItem {
            id:"A".into(),source_path:"private-A".into(),selected:true,..Default::default()
        }])));
        let b=Fixture::new();
        a.context.store.borrow_mut().private_persistence=Some(b.persistence.clone());
        app.global::<AppState>().invoke_confirm_video_assets();
        assert_eq!(app.global::<AppState>().get_video_images().row_count(),0);
        b.drain();
    }
    #[test]
    fn video_image_cancel_counter_exhaustion_never_reuses_zero() {
        let (f,app,epoch,_quote,_key)=setup();
        epoch.store(u64::MAX,Ordering::SeqCst);
        app.global::<AppState>().invoke_close_video_image_dialog();
        assert_eq!(epoch.load(Ordering::SeqCst),u64::MAX);
        f.drain();
    }
    #[test]
    fn core_video_input_sent_success_then_panic_never_publishes_owned_rows() {
        let (f,app,_epoch,_quote,_key)=setup();
        let input=tempfile::tempdir().unwrap();
        let original=f.owned(&png(input.path(),"sent-then-panic.png"));
        f.context.store.borrow_mut().assets.push(asset(&original));
        let (sent,observed)=mpsc::channel();
        set_delivery_preparation_after_send_for_test(move|| {
            let _=sent.send(());
            panic!("controlled video input failure after sending pixels");
        });
        app.global::<AppState>().invoke_open_video_asset_picker();
        let reached=observed.recv_timeout(Duration::from_secs(5));
        let deadline=Instant::now()+Duration::from_secs(5);
        while app.global::<AppState>().get_video_images_loading() && Instant::now()<deadline {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            std::thread::sleep(Duration::from_millis(2));
        }
        let joined=drain_delivery_commit_workers_for_lease_for_test(f.persistence.lease());
        let retired=f.context.user_activity.begin_quiesce(f.persistence.lease()).map(|guard|guard.retire());
        reached.unwrap();retired.unwrap();assert!(joined.is_err());
        assert!(!app.global::<AppState>().get_video_images_loading());
        assert_eq!(app.global::<AppState>().get_video_asset_choices().row_count(),0,
            "a queued success is not an actual successful worker exit");
        assert_eq!(app.global::<AppState>().get_video_images().row_count(),0);
        assert!(original.is_file());
    }
    #[test]
    fn video_asset_import_copies_into_owned_namespace_and_quotes_outside_the_latch() {
        let (f,app,_epoch,_quote,_key)=setup();
        let input=tempfile::tempdir().unwrap();
        let original=f.owned(&png(input.path(),"source.png"));
        f.context.store.borrow_mut().assets.push(asset(&original));
        let invoked=Rc::new(Cell::new(false));
        let observed=invoked.clone();
        let latch=f.persistence.upgrade_latch();
        app.global::<AppState>().on_request_video_quote(move |_,_,_| {
            assert!(latch.snapshot().is_none(),"quote callback must be able to enter its own latch");
            observed.set(true);
        });
        app.global::<AppState>().invoke_open_video_asset_picker();
        pump(|| !app.global::<AppState>().get_video_images_loading());
        let row=app.global::<AppState>().get_video_asset_choices().row_data(0).expect("owned image row");
        assert_ne!(Path::new(row.source_path.as_str()),original.as_path(),"source must be copied before publication");
        assert!(f.persistence.owns_path(Path::new(row.source_path.as_str())));
        assert!(row.image.path().is_none(),"preview must own pixels, not lazily reopen a path");
        app.global::<AppState>().invoke_toggle_video_asset(row.id.clone());
        app.global::<AppState>().invoke_confirm_video_assets();
        assert_eq!(app.global::<AppState>().get_video_images().row_count(),1);
        assert!(invoked.get());
        assert!(original.is_file());
        f.drain();
    }
    #[test]
    fn video_late_import_cannot_replace_another_store_model() {
        let (a,app,_epoch,_quote,_key)=setup();
        let input=tempfile::tempdir().unwrap();
        let original=a.owned(&png(input.path(),"source.png"));
        a.context.store.borrow_mut().assets.push(asset(&original));
        app.global::<AppState>().invoke_open_video_asset_picker();
        a.drain();
        let b=Fixture::new();
        a.context.store.borrow_mut().private_persistence=Some(b.persistence.clone());
        app.global::<AppState>().set_video_asset_choices(ModelRc::new(VecModel::from(vec![VideoImageItem {
            id:"B-only".into(),title:"B-only".into(),..Default::default()
        }])));
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(app.global::<AppState>().get_video_asset_choices().row_data(0).unwrap().id.as_str(),"B-only");
        b.drain();
    }
}
