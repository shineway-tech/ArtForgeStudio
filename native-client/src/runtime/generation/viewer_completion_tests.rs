use super::*;

struct Fixture(video_image_callbacks::tests::scoped_inputs::Fixture);

impl std::ops::Deref for Fixture {
    type Target = video_image_callbacks::tests::scoped_inputs::Fixture;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let lease = self.persistence.lease();
        let workers = drain_delivery_commit_workers_for_lease_for_test(lease);
        let previews = drain_activation_preview_workers_for_lease_for_test(lease);
        // Release terminal polling callbacks before the testing backend shuts down.
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        slint::platform::update_timers_and_animations();
        let retired = self
            .context
            .user_activity
            .begin_quiesce(lease)
            .map(|guard| guard.retire());
        if !std::thread::panicking() {
            workers.unwrap();
            previews.unwrap();
            retired.unwrap();
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Completion {
    Stream,
    PartialFailure,
    Paid,
}

fn poll_outcomes(
    f: &Fixture,
    app: &AppWindow,
    record: PendingGenerationRecord,
    outcomes: Vec<GenerationOutcome>,
) {
    let (sender, receiver) = mpsc::channel();
    for outcome in outcomes {
        sender.send(outcome).unwrap();
    }
    drop(sender);
    poll_generation_stream(
        app.as_weak(),
        f.context.clone(),
        f.persistence.clone(),
        f.context.current_account_session_scope().unwrap(),
        vec![],
        Rc::new(RefCell::new(Some(receiver))),
        record.raw_prompt,
        record.category,
        record.mode,
        record.ratio,
        record.quality,
        record.model_code,
        record.task_type,
        record.conversation_id,
        false,
        vec![],
        vec![],
        QuoteContext {
            title: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            width: 0,
            height: 0,
        },
        false,
        record.local_task_id,
        Instant::now(),
    );
    video_image_callbacks::tests::scoped_inputs::pump(|| {
        f.context.generations.active.borrow().is_empty()
    });
}

fn complete(completion: Completion, f: &Fixture, app: &AppWindow, record: PendingGenerationRecord) {
    if matches!(completion, Completion::Paid) {
        finish_paid_viewer_results(
            app.as_weak(),
            f.context.clone(),
            f.persistence.clone(),
            record,
            vec![],
        );
    } else {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(if matches!(completion, Completion::PartialFailure) {
                GenerationOutcome::Failure {
                    reason: "fixture failure".into(),
                    time: "fixture".into(),
                }
            } else {
                GenerationOutcome::Finished
            })
            .unwrap();
        poll_generation_stream(
            app.as_weak(),
            f.context.clone(),
            f.persistence.clone(),
            f.context.current_account_session_scope().unwrap(),
            vec![],
            Rc::new(RefCell::new(Some(receiver))),
            record.raw_prompt,
            record.category,
            record.mode,
            record.ratio,
            record.quality,
            record.model_code,
            record.task_type,
            record.conversation_id,
            false,
            vec![],
            vec![],
            QuoteContext {
                title: String::new(),
                prompt: String::new(),
                ratio: String::new(),
                quality: String::new(),
                width: 0,
                height: 0,
            },
            false,
            record.local_task_id,
            Instant::now(),
        );
    }
    video_image_callbacks::tests::scoped_inputs::pump(|| {
        f.context.generations.active.borrow().is_empty()
    });
}

fn setup() -> (Fixture, AppWindow, PendingGenerationRecord) {
    let f = Fixture(video_image_callbacks::tests::scoped_inputs::Fixture::new());
    let transition = f
        .context
        .namespace_operations
        .try_begin_transition()
        .unwrap();
    let recovery = transition
        .begin_prepublication_recovery(f.persistence.lease())
        .unwrap();
    recovery
        .verify_no_unsupported_imports(&f.authority)
        .unwrap();
    let recovered = recovery.finish().unwrap();
    transition
        .prepare_publication(f.persistence.lease(), recovered)
        .unwrap()
        .publish();
    let scope = f.context.current_account_session_scope().unwrap();
    let mut record = billing_capture_test_support::generation_record(
        &BillingScope {
            request: GroupRequestScope {
                session: scope.clone(),
                account_group_id: Uuid::new_v4().to_string(),
            },
            context_epoch: 1,
        },
        "image_generation",
    );
    record.category = "ui".into();
    let app = AppWindow::new().unwrap();
    let state = app.global::<AppState>();
    state.set_page("generation".into());
    state.set_asset_type(record.category.clone().into());
    state.set_current_conversation_id(record.conversation_id.clone().into());
    state.set_viewer_id("original-image".into());
    state.set_viewer_source("asset".into());
    state.set_viewer_source_path("original.png".into());
    state.set_viewer_image(Image::from_rgba8(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::new(3, 2)));
    state.set_cutout_result_path("original-cutout.png".into());
    state.set_cutout_result_image(Image::from_rgba8(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::new(2, 3)));
    let generated = AssetData {
        id: "new-generation".into(),
        conversation_id: record.conversation_id.clone(),
        title: "Generated".into(),
        category: record.category.clone(),
        kind: "game".into(),
        time: "fixture".into(),
        prompt: String::new(),
        ratio: "1:1".into(),
        quality: "1K".into(),
        model: "fixture-model".into(),
        origin: "image_generation".into(),
        width: 80,
        height: 80,
        source_path: String::new(),
        reference_paths: vec![],
        cutout_done: false,
        remove_black_done: false,
        upscale_done: false,
        is_new: true,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    f.context.store.borrow_mut().assets.push(generated.clone());
    f.context.store.borrow_mut().generations.push(generated);
    f.context.generations.active.borrow_mut().insert(
        record.category.clone(),
        ActiveGeneration {
            task_id: record.local_task_id.clone(),
            category: record.category.clone(),
            conversation_id: record.conversation_id.clone(),
            total_count: 2,
            completed_count: 1,
            success_count: 1,
            latest_success_id: Some("new-generation".into()),
            session_scope: scope,
            ..Default::default()
        },
    );
    (f, app, record)
}

#[test]
fn generation_completion_preserves_cutout_source_and_result() {
    i_slint_backend_testing::init_no_event_loop();
    for completion in [
        Completion::Stream,
        Completion::PartialFailure,
        Completion::Paid,
    ] {
        for processing in [false, true] {
            let (f, app, record) = setup();
            let state = app.global::<AppState>();
            state.set_viewer_open(false);
            state.set_cutout_open(true);
            state.set_cutout_processing(processing);
            complete(completion, &f, &app, record);
            assert_eq!(
                state.get_viewer_id(),
                "original-image",
                "{completion:?}, processing={processing}"
            );
            assert_eq!(state.get_viewer_source(), "asset");
            assert_eq!(state.get_viewer_source_path(), "original.png");
            let size = state.get_viewer_image().size();
            assert_eq!((size.width, size.height), (3, 2));
            assert_eq!(state.get_cutout_result_path(), "original-cutout.png");
            let size = state.get_cutout_result_image().size();
            assert_eq!((size.width, size.height), (2, 3));
            assert_eq!(state.get_cutout_processing(), processing);
            assert!(state.get_cutout_open());
            assert!(!state.get_viewer_open());
            assert!(f
                .context
                .store
                .borrow()
                .generations
                .iter()
                .any(|item| item.id == "new-generation"));
        }
    }
}

#[test]
fn generation_completion_preserves_other_image_workflows() {
    i_slint_backend_testing::init_no_event_loop();
    let scenarios: &[(&str, fn(&AppState))] = &[
        ("viewer", |s| s.set_viewer_open(true)),
        ("remove-black", |s| s.set_viewer_processing(true)),
        ("upscale", |s| s.set_upscale_open(true)),
        ("image-editor", |s| s.set_page("image-editor".into())),
        ("enhance", |s| s.set_page("toolbox-enhance".into())),
        ("watermark", |s| s.set_page("toolbox-watermark".into())),
        ("toolbox-remove-black", |s| {
            s.set_page("toolbox-remove-black".into())
        }),
        ("colorize", |s| s.set_page("toolbox-colorize".into())),
        ("crop", |s| s.set_page("toolbox-crop".into())),
        ("convert", |s| s.set_page("toolbox-convert".into())),
        ("compress", |s| s.set_page("toolbox-compress".into())),
        ("canvas", |s| s.set_page("canvas".into())),
        ("video", |s| s.set_page("video-generation".into())),
        ("other-category", |s| s.set_asset_type("character".into())),
        ("other-conversation", |s| {
            s.set_current_conversation_id("other-conversation".into())
        }),
        ("selection", |s| s.set_generation_selection_mode(true)),
    ];
    for completion in [
        Completion::Stream,
        Completion::PartialFailure,
        Completion::Paid,
    ] {
        for (name, configure) in scenarios {
            let (f, app, record) = setup();
            let state = app.global::<AppState>();
            configure(&state);
            let page = state.get_page();
            let viewer_open = state.get_viewer_open();
            let viewer_processing = state.get_viewer_processing();
            let upscale_open = state.get_upscale_open();
            complete(completion, &f, &app, record);
            assert_eq!(
                state.get_viewer_id(),
                "original-image",
                "{completion:?}: {name}"
            );
            assert_eq!(state.get_viewer_source_path(), "original.png");
            let size = state.get_viewer_image().size();
            assert_eq!((size.width, size.height), (3, 2));
            assert_eq!(state.get_page(), page);
            assert_eq!(state.get_viewer_open(), viewer_open);
            assert_eq!(state.get_viewer_processing(), viewer_processing);
            assert_eq!(state.get_upscale_open(), upscale_open);
        }
    }
}

#[test]
fn generation_completion_opens_result_in_idle_original_workspace() {
    i_slint_backend_testing::init_no_event_loop();
    for completion in [
        Completion::Stream,
        Completion::PartialFailure,
        Completion::Paid,
    ] {
        let (f, app, record) = setup();
        complete(completion, &f, &app, record);
        let state = app.global::<AppState>();
        assert_eq!(state.get_viewer_id(), "new-generation", "{completion:?}");
        assert!(state.get_viewer_open());
    }
}

#[test]
fn generation_completion_disconnect_only_fails_unfinished_slots() {
    i_slint_backend_testing::init_no_event_loop();
    for explicit_failure in [false, true] {
        for (total, completed) in [(1, 1), (4, 4), (4, 1), (1, 0)] {
            let (f, app, mut record) = setup();
            record.count = total;
            {
                let mut store = f.context.store.borrow_mut();
                let template = store.generations[0].clone();
                store.generations = (0..completed)
                    .map(|index| {
                        let mut asset = template.clone();
                        asset.id = format!("saved-{index}");
                        asset
                    })
                    .collect();
                store.assets = store.generations.clone();
            }
            {
                let mut tasks = f.context.generations.active.borrow_mut();
                let task = tasks.get_mut(&record.category).unwrap();
                task.total_count = total;
                task.completed_count = completed;
                task.success_count = completed;
                task.loading_count = total - completed;
                task.latest_success_id = None;
            }
            sync_generation_state_for_current_category(&f.context, &app);
            let outcomes = if explicit_failure {
                vec![GenerationOutcome::Failure {
                    reason: "fixture transport failure".into(),
                    time: "fixture".into(),
                }]
            } else {
                vec![]
            };
            poll_outcomes(&f, &app, record, outcomes);
            let store = f.context.store.borrow();
            assert_eq!(
                store.generations.len(),
                total as usize,
                "total={total}, completed={completed}, explicit={explicit_failure}"
            );
            assert_eq!(
                store
                    .generations
                    .iter()
                    .filter(|item| item.source_path == "failed")
                    .count(),
                (total - completed) as usize
            );
            assert_eq!(
                store
                    .notifications
                    .iter()
                    .filter(|item| !item.success)
                    .count(),
                (total - completed) as usize
            );
            let state = app.global::<AppState>();
            assert!(!state.get_generating());
            assert_eq!(state.get_generation_loading_count(), 0);
            if total == completed {
                assert_eq!(state.get_generation_status(), "生成成功");
            }
        }
    }
}

#[test]
fn generation_completion_disconnect_does_not_duplicate_a_reported_failure() {
    i_slint_backend_testing::init_no_event_loop();
    let (f, app, mut record) = setup();
    record.count = 1;
    {
        let mut store = f.context.store.borrow_mut();
        store.generations.clear();
        store.assets.clear();
    }
    {
        let mut tasks = f.context.generations.active.borrow_mut();
        let task = tasks.get_mut(&record.category).unwrap();
        task.total_count = 1;
        task.completed_count = 0;
        task.success_count = 0;
        task.loading_count = 1;
        task.latest_success_id = None;
    }
    poll_outcomes(
        &f,
        &app,
        record,
        vec![GenerationOutcome::ImageFailure {
            reason: "fixture image failure".into(),
            time: "fixture".into(),
            delivery: None,
        }],
    );
    let store = f.context.store.borrow();
    assert_eq!(store.generations.len(), 1);
    assert_eq!(store.generations[0].source_path, "failed");
    assert_eq!(
        store
            .notifications
            .iter()
            .filter(|item| !item.success)
            .count(),
        1
    );
    assert!(!app.global::<AppState>().get_generating());
    assert_eq!(app.global::<AppState>().get_generation_loading_count(), 0);
}

#[test]
fn generation_completion_paid_late_failure_does_not_add_a_result() {
    i_slint_backend_testing::init_no_event_loop();
    for item_failure in [false, true] {
        let (f, app, mut record) = setup();
        record.count = 1;
        f.context
            .generations
            .active
            .borrow_mut()
            .get_mut(&record.category)
            .unwrap()
            .total_count = 1;
        let outcome = if item_failure {
            GenerationOutcome::ImageFailure {
                reason: "late fixture failure".into(),
                time: "fixture".into(),
                delivery: None,
            }
        } else {
            GenerationOutcome::Failure {
                reason: "late fixture failure".into(),
                time: "fixture".into(),
            }
        };
        finish_paid_viewer_results(
            app.as_weak(),
            f.context.clone(),
            f.persistence.clone(),
            record,
            vec![outcome],
        );
        video_image_callbacks::tests::scoped_inputs::pump(|| {
            f.context.generations.active.borrow().is_empty()
        });
        let store = f.context.store.borrow();
        assert_eq!(store.generations.len(), 1);
        assert_eq!(store.generations[0].id, "new-generation");
        assert!(store.notifications.iter().all(|item| item.success));
        assert!(!app.global::<AppState>().get_generating());
        assert_eq!(app.global::<AppState>().get_generation_loading_count(), 0);
        assert_eq!(app.global::<AppState>().get_generation_status(), "生成成功");
    }
}
