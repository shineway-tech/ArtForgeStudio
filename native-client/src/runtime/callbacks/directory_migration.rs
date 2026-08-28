use super::*;
use crate::directory_migration::MigrationPlan;

pub(super) fn wire_directory_migration_callbacks(app: &AppWindow, context: AppContext) {
    let pending: Rc<RefCell<Option<(String, MigrationPlan)>>> = Rc::new(RefCell::new(None));
    let state = app.global::<AppState>();
    {
        let weak = app.as_weak();
        let context = context.clone();
        let pending = pending.clone();
        state.on_pick_dir(move |kind| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_directory_migration_open() {
                return;
            }
            if migration_has_active_work(&app, &context) {
                show_migration_error(&app, "请等待生成、文件处理或账号恢复完成后再迁移目录。");
                return;
            }
            let config = directory_locations();
            let Some(source) = config.directory(kind.as_str()) else {
                return;
            };
            let english = state.get_language() == "en";
            let Some(destination) = rfd::FileDialog::new()
                .set_title(if english {
                    "Choose migration destination"
                } else {
                    "选择迁移目标文件夹"
                })
                .set_directory(&source)
                .pick_folder()
            else {
                return;
            };
            let mut protected = vec![app_data_dir()];
            for other in ["input", "output", "prompt"] {
                if other != kind.as_str() {
                    protected.push(config.directory(other).unwrap());
                }
            }
            state.set_directory_migration_kind(kind.clone());
            state.set_directory_migration_source(display_directory_path(&source).into());
            state.set_directory_migration_target(display_directory_path(&destination).into());
            state.set_directory_migration_stage("checking".into());
            state.set_directory_migration_message("正在检查目录和文件冲突…".into());
            state.set_directory_migration_open(true);
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(
                    MigrationPlan::prepare(&source, &destination, &protected)
                        .map_err(|e| e.to_string()),
                );
            });
            poll_migration_plan(
                app.as_weak(),
                kind.to_string(),
                pending.clone(),
                Rc::new(receiver),
            );
        });
    }
    {
        let weak = app.as_weak();
        let pending = pending.clone();
        state.on_close_directory_migration(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_directory_migration_busy() {
                return;
            }
            pending.borrow_mut().take();
            state.set_directory_migration_open(false);
        });
    }
    {
        let weak = app.as_weak();
        let context = context.clone();
        state.on_confirm_directory_migration(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if app.global::<AppState>().get_directory_migration_stage() != "confirm" {
                return;
            }
            if migration_has_active_work(&app, &context) {
                show_migration_error(&app, "仍有文件操作正在运行，请完成后重试。");
                return;
            }
            let Some((kind, plan)) = pending.borrow_mut().take() else {
                return;
            };
            start_directory_migration(&app, context.clone(), kind, plan);
        });
    }
    let weak = app.as_weak();
    app.window().on_close_requested(move || {
        if weak
            .upgrade()
            .is_some_and(|app| app.global::<AppState>().get_directory_migration_busy())
        {
            slint::CloseRequestResponse::KeepWindowShown
        } else {
            slint::CloseRequestResponse::HideWindow
        }
    });
}

fn migration_has_active_work(app: &AppWindow, context: &AppContext) -> bool {
    let state = app.global::<AppState>();
    !context.generations.active.borrow().is_empty()
        || !context
            .active_prompt_task_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        || context.prompt_optimization_polling.borrow().is_some()
        || state.get_auth_busy()
        || state.get_generating()
        || state.get_storage_busy()
        || state.get_video_generating()
        || state.get_image_editor_generating()
        || state.get_viewer_processing()
        || state.get_cutout_processing()
        || state.get_compression_processing()
        || state.get_conversion_processing()
        || state.get_crop_processing()
        || state.get_enhance_processing()
        || state.get_watermark_processing()
        || state.get_colorize_processing()
        || state.get_optimizing_prompt()
        || state.get_optimizing_video_prompt()
        || state.get_custom_prompt_analyzing()
        || state.get_translating_prompt()
        || !state.get_canvas_split_loading_node_id().is_empty()
        || !state.get_canvas_extraction_loading_node_id().is_empty()
        || state.get_update_active()
}

fn show_migration_error(app: &AppWindow, message: &str) {
    let state = app.global::<AppState>();
    state.set_directory_migration_stage("error".into());
    state.set_directory_migration_message(message.into());
    state.set_directory_migration_open(true);
}

fn poll_migration_plan(
    weak: Weak<AppWindow>,
    kind: String,
    pending: Rc<RefCell<Option<(String, MigrationPlan)>>>,
    receiver: Rc<mpsc::Receiver<std::result::Result<MigrationPlan, String>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(plan)) => {
                let state = app.global::<AppState>();
                state.set_directory_migration_message(format!("共 {} 个文件（{}），包含全部子文件夹。\n确认后更新保存位置；新位置有同名内容时不会覆盖。", plan.files, format_storage_bytes(plan.bytes)).into());
                state.set_directory_migration_stage("confirm".into());
                *pending.borrow_mut() = Some((kind, plan));
            }
            Ok(Err(error)) => show_migration_error(&app, &error),
            Err(TryRecvError::Empty) => poll_migration_plan(weak, kind, pending, receiver),
            Err(TryRecvError::Disconnected) => {
                show_migration_error(&app, "目录检查未完成，原文件未改动。")
            }
        }
    });
}

fn start_directory_migration(
    app: &AppWindow,
    context: AppContext,
    kind: String,
    plan: MigrationPlan,
) {
    let config = match directory_locations().migrated(
        &kind,
        plan.source.clone(),
        plan.destination.clone(),
    ) {
        Ok(config) => config,
        Err(error) => {
            show_migration_error(app, &error.to_string());
            return;
        }
    };
    let state = app.global::<AppState>();
    state.set_directory_migration_stage("copying".into());
    state.set_directory_migration_progress(0);
    state.set_directory_migration_message("正在复制并校验文件，请勿断开磁盘…".into());
    let progress = Arc::new(AtomicU64::new(0));
    let worker_progress = progress.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = plan
            .execute(
                || persist_directory_locations_checked(config).map_err(std::io::Error::other),
                |done, total| {
                    worker_progress.store(
                        if total == 0 {
                            0
                        } else {
                            (done as f64 / total as f64 * 95.0) as u64
                        },
                        Ordering::Relaxed,
                    );
                },
            )
            .map_err(|e| e.to_string());
        let _ = sender.send(result);
    });
    poll_directory_migration(app.as_weak(), context, Rc::new(receiver), progress);
}

fn poll_directory_migration(
    weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<mpsc::Receiver<std::result::Result<Vec<PathBuf>, String>>>,
    progress: Arc<AtomicU64>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_directory_migration_progress(progress.load(Ordering::Relaxed) as i32);
        match receiver.try_recv() {
            Ok(Ok(leftovers)) => {
                sync_migrated_file_locations(&app, &context);
                state.set_directory_migration_progress(100);
                state.set_directory_migration_stage("done".into());
                state.set_directory_migration_message(if leftovers.is_empty() {
                    "迁移完成，全部文件已移至新目录，保存位置已更新。".to_string()
                } else {
                    format!("文件已复制并切换到新目录。旧目录中有 {} 项因占用或内容变化未清理，已保留，请检查。", leftovers.len())
                }.into());
                refresh_storage_usage_async(&app);
            }
            Ok(Err(error)) => show_migration_error(&app, &error),
            Err(TryRecvError::Empty) => poll_directory_migration(weak, context, receiver, progress),
            Err(TryRecvError::Disconnected) => {
                sync_migrated_file_locations(&app, &context);
                show_migration_error(
                    &app,
                    "迁移线程异常结束，请检查新旧目录；已保存的数据不会覆盖。",
                )
            }
        }
    });
}

fn sync_migrated_file_locations(app: &AppWindow, context: &AppContext) {
    let config = directory_locations();
    config.remap_store(&mut context.store.borrow_mut());
    context
        .canvas_history
        .borrow_mut()
        .remap_file_locations(&config);
    sync_directory_locations(app);
    let state = app.global::<AppState>();
    macro_rules! remap_property {
        ($get:ident, $set:ident) => {{
            let mut value = state.$get().to_string();
            config.remap(&mut value);
            state.$set(value.into());
        }};
    }
    remap_property!(get_viewer_source_path, set_viewer_source_path);
    remap_property!(get_video_source_path, set_video_source_path);
    remap_property!(get_video_result_path, set_video_result_path);
    remap_property!(get_image_editor_source_path, set_image_editor_source_path);
    remap_property!(
        get_custom_prompt_reference_path,
        set_custom_prompt_reference_path
    );
    remap_property!(get_crop_source_path, set_crop_source_path);
    remap_property!(get_enhance_source_path, set_enhance_source_path);
    remap_property!(get_enhance_result_path, set_enhance_result_path);
    remap_property!(get_watermark_source_path, set_watermark_source_path);
    remap_property!(get_watermark_result_path, set_watermark_result_path);
    remap_property!(get_colorize_source_path, set_colorize_source_path);
    remap_property!(get_colorize_result_path, set_colorize_result_path);
    remap_property!(get_cutout_result_path, set_cutout_result_path);
    let remap_shared = |value: &mut SharedString| {
        let mut path = value.to_string();
        config.remap(&mut path);
        *value = path.into();
    };
    let references: Vec<_> = state
        .get_custom_prompt_reference_items()
        .iter()
        .map(|mut item| {
            remap_shared(&mut item.source_path);
            item
        })
        .collect();
    state.set_custom_prompt_reference_items(ModelRc::new(VecModel::from(references)));
    macro_rules! remap_image_model {
        ($get:ident, $set:ident) => {{
            let items: Vec<_> = state
                .$get()
                .iter()
                .map(|mut item| {
                    remap_shared(&mut item.source_path);
                    remap_shared(&mut item.result_path);
                    item
                })
                .collect();
            state.$set(ModelRc::new(VecModel::from(items)));
        }};
    }
    remap_image_model!(get_compression_images, set_compression_images);
    remap_image_model!(get_conversion_images, set_conversion_images);
    let store = context.store.borrow();
    clear_preview_memory_cache();
    push_assets(app, &store);
    push_generations(app, &store);
    push_references(app, &store);
    push_custom_prompts(app, &store);
    push_canvas_notes(app, &store);
    save_local_store(app, &store);
    rebuild_storage_references(&store);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_migration_guards_busy_operations_and_requires_confirmation() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let context = AppContext::default();
        wire_directory_migration_callbacks(&app, context.clone());
        let state = app.global::<AppState>();
        state.set_generating(true);
        state.invoke_pick_dir("output".into());
        assert_eq!(state.get_directory_migration_stage(), "error");
        assert!(state.get_directory_migration_open());
        state.invoke_close_directory_migration();
        assert!(!state.get_directory_migration_open());
        state.set_generating(false);
        state.set_directory_migration_open(true);
        state.set_directory_migration_stage("copying".into());
        assert!(state.get_directory_migration_busy());
        state.invoke_close_directory_migration();
        assert!(state.get_directory_migration_open());
        state.invoke_confirm_directory_migration();
        assert_eq!(state.get_directory_migration_stage(), "copying");
        state.set_directory_migration_stage("confirm".into());
        state.invoke_confirm_directory_migration();
        assert_eq!(state.get_directory_migration_stage(), "confirm");
        state.invoke_close_directory_migration();
        assert!(!state.get_directory_migration_open());
        context
            .generations
            .active
            .borrow_mut()
            .insert("background".into(), ActiveGeneration::default());
        assert!(migration_has_active_work(&app, &context));
    }

    #[test]
    fn about_config_path_migration_uses_the_displayed_input_directory() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true, renderer_name: Some("software".into()), ..Default::default()
            },
        ))).unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_page("settings".into());
        state.set_settings_section("about".into());
        state.set_input_dir(r"E:\我的素材\input".into());
        let observed = Rc::new(RefCell::new(Vec::new()));
        let captured = observed.clone();
        state.on_pick_dir(move |kind| captured.borrow_mut().push(kind.to_string()));
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().unwrap();
        let button = ElementHandle::find_by_element_id(&app, "SettingsPage::config-path-migrate")
            .next().expect("configuration path migration button");
        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0), (1920.0, 1080.0)] {
            app.window().set_size(slint::LogicalSize::new(width, height));
            assert!(button.absolute_position().x + button.size().width < width);
            assert!(button.absolute_position().y + button.size().height < height);
        }
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
            let directory = PathBuf::from(directory);
            fs::create_dir_all(&directory).unwrap();
            let pixels = app.window().take_snapshot().unwrap();
            image::save_buffer(directory.join("about-config-migration.png"), pixels.as_bytes(), pixels.width(), pixels.height(), image::ColorType::Rgba8).unwrap();
        }
        button.mock_single_click(PointerEventButton::Left);
        assert_eq!(observed.borrow().as_slice(), &["input"]);
        assert_eq!(state.get_input_dir(), r"E:\我的素材\input");
        state.set_directory_migration_open(true);
        button.mock_single_click(PointerEventButton::Left);
        assert_eq!(observed.borrow().len(), 1);
    }

    #[test]
    fn directory_migration_dialog_layout_and_cancel_are_usable() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                renderer_name: Some("software".into()),
                ..Default::default()
            },
        )))
        .unwrap();
        let app = AppWindow::new().unwrap();
        wire_directory_migration_callbacks(&app, AppContext::default());
        apply_theme(&app, "light");
        let state = app.global::<AppState>();
        state.set_page("settings".into());
        state.set_settings_section("basic".into());
        state.set_contact_popup_open(false);
        state.set_directory_migration_source(r"E:\Elunvi Canvas\data\out".into());
        state.set_directory_migration_target(r"D:\我的作品\Elunvi Canvas\新输出目录".into());
        state.set_directory_migration_message("共 128 个文件（2.4 GB），包含全部子文件夹。\n确认后更新保存位置；新位置有同名内容时不会覆盖。".into());
        state.set_directory_migration_stage("confirm".into());
        state.set_directory_migration_open(true);
        app.show().unwrap();
        for (width, height) in [(1180.0, 760.0), (1440.0, 900.0), (1920.0, 1080.0)] {
            app.window()
                .set_size(slint::LogicalSize::new(width, height));
            let close =
                ElementHandle::find_by_element_id(&app, "DirectoryMigrationDialog::close-button")
                    .next()
                    .unwrap();
            let confirm =
                ElementHandle::find_by_element_id(&app, "DirectoryMigrationDialog::confirm-button")
                    .next()
                    .unwrap();
            assert!(
                close.absolute_position().x + close.size().width < confirm.absolute_position().x
            );
            assert!(confirm.absolute_position().x + confirm.size().width < width);
            assert!(confirm.absolute_position().y + confirm.size().height < height);
        }
        app.window()
            .set_size(slint::LogicalSize::new(1440.0, 900.0));
        if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
            let directory = PathBuf::from(directory);
            fs::create_dir_all(&directory).unwrap();
            let pixels = app.window().take_snapshot().unwrap();
            image::save_buffer(
                directory.join("directory-migration-confirm.png"),
                pixels.as_bytes(),
                pixels.width(),
                pixels.height(),
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
        let close =
            ElementHandle::find_by_element_id(&app, "DirectoryMigrationDialog::close-button")
                .next()
                .unwrap();
        close.mock_single_click(PointerEventButton::Left);
        assert!(!state.get_directory_migration_open());
        state.set_directory_migration_stage("copying".into());
        state.set_directory_migration_open(true);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(5));
        let close =
            ElementHandle::find_by_element_id(&app, "DirectoryMigrationDialog::close-button")
                .next()
                .unwrap();
        close.mock_single_click(PointerEventButton::Left);
        assert!(state.get_directory_migration_open());
        app.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(state.get_directory_migration_open());
        state.set_directory_migration_stage("done".into());
        app.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!state.get_directory_migration_open());
    }
}
