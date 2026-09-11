use super::*;
use crate::directory_migration::MigrationPlan;

struct PendingAccountMigration {
    lease: NamespaceLease,
    area: ManagedUserArea,
    plan: MigrationPlan,
}

enum AccountMigrationOutcome {
    Committed(account_transition::AccountDirectoryMigrationCompletion),
    Failed(String),
    RecoveryRequired,
}

fn migration_area(kind: &str) -> Option<ManagedUserArea> {
    match kind {
        "input" => Some(ManagedUserArea::Input),
        "output" => Some(ManagedUserArea::Output),
        "prompt" => Some(ManagedUserArea::Prompt),
        _ => None,
    }
}

fn current_migration_lease(context: &AppContext) -> Result<NamespaceLease> {
    let scope = context.current_account_session_scope().ok_or_else(|| anyhow!("请先登录，再迁移当前账号目录。"))?;
    context.namespace_for(&scope).map_err(|_| anyhow!("当前账号尚未就绪，请稍后重试。"))
}

fn migration_lease_is_current(context: &AppContext, lease: &NamespaceLease) -> bool {
    current_migration_lease(context).as_ref().ok() == Some(lease)
}

pub(super) fn wire_directory_migration_callbacks(app: &AppWindow, context: AppContext) {
    let pending: Rc<RefCell<Option<PendingAccountMigration>>> = Rc::new(RefCell::new(None));
    let state = app.global::<AppState>();
    {
        let weak = app.as_weak();
        let context = context.clone();
        let pending = pending.clone();
        state.on_pick_dir(move |kind| {
            let Some(app) = weak.upgrade() else { return; };
            if app.global::<AppState>().get_directory_migration_open() { return; }
            let Some(area) = migration_area(kind.as_str()) else {
                show_migration_error(&app, "未知目录类型。"); return;
            };
            let lease = match current_migration_lease(&context) {
                Ok(lease) => lease,
                Err(error) => { show_migration_error(&app, &error.to_string()); return; }
            };
            if migration_has_active_work(&app, &context) {
                show_migration_error(&app, "请等待生成、文件处理或账号切换完成后再迁移目录。"); return;
            }
            let Some(data_root) = context.data_root_capability.clone() else {
                show_migration_error(&app, "账号存储尚未就绪，请稍后重试。"); return;
            };
            let source = lease.namespace.path(area);
            let Some(chosen) = rfd::FileDialog::new()
                .set_title("选择当前账号的迁移目标文件夹")
                .set_directory(&source).pick_folder() else { return; };
            if !migration_lease_is_current(&context, &lease) {
                show_migration_error(&app, "账号已变化，请重新选择迁移目录。"); return;
            }
            let target = chosen.join("ElunviCanvas").join("accounts")
                .join(lease.namespace.user_public_id()).join(match area {
                    ManagedUserArea::Input => "input", ManagedUserArea::Output => "out", _ => "prompt",
                });
            let protected = vec![app_data_dir(), lease.namespace.root().to_path_buf(),
                lease.namespace.path(ManagedUserArea::Input), lease.namespace.path(ManagedUserArea::Output),
                lease.namespace.path(ManagedUserArea::Prompt)];
            let planning_permit = match context.user_activity.begin_recovery_unit(&lease) {
                Ok(permit) => permit,
                Err(_) => { show_migration_error(&app, "账号正在切换，请稍后重试。"); return; }
            };
            let state = app.global::<AppState>();
            *pending.borrow_mut() = None;
            state.set_directory_migration_kind(kind);
            state.set_directory_migration_source(display_directory_path(&source).into());
            state.set_directory_migration_target(display_directory_path(&target).into());
            state.set_directory_migration_stage("checking".into());
            state.set_directory_migration_message("正在检查当前账号目录和同名文件…".into());
            state.set_directory_migration_open(true);
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _planning_permit = planning_permit;
                let result = ExternalExportDestination::open(&data_root, &chosen)
                    .and_then(|_chosen_capability| prepare_account_migration(&chosen, &target, &source, &protected))
                    .map(|plan| PendingAccountMigration { lease, area, plan });
                let _ = sender.send(result.map_err(|error| error.to_string()));
            });
            poll_migration_plan(app.as_weak(), context.clone(), pending.clone(), Rc::new(receiver));
        });
    }
    {
        let weak = app.as_weak();
        let pending = pending.clone();
        state.on_close_directory_migration(move || {
            let Some(app) = weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            if state.get_directory_migration_busy() { return; }
            *pending.borrow_mut() = None;
            state.set_directory_migration_open(false);
        });
    }
    {
        let weak = app.as_weak();
        state.on_confirm_directory_migration(move || {
            let Some(app) = weak.upgrade() else { return; };
            let state = app.global::<AppState>();
            if state.get_directory_migration_busy() { return; }
            let Some(prepared) = pending.borrow_mut().take() else {
                show_migration_error(&app, "迁移信息已失效，请重新选择目录。"); return;
            };
            if !migration_lease_is_current(&context, &prepared.lease) {
                show_migration_error(&app, "账号已变化，原文件未改动，请重新选择目录。"); return;
            }
            if migration_has_active_work(&app, &context) {
                show_migration_error(&app, "当前有文件处理或账号切换，请完成后重新选择目录。"); return;
            }
            start_directory_migration(&app, context.clone(), prepared);
        });
    }
}

fn prepare_account_migration(chosen: &Path, target: &Path, source: &Path, protected: &[PathBuf]) -> Result<MigrationPlan> {
    use crate::directory_migration::checked_directory;
    let relative = target.strip_prefix(chosen)?.to_path_buf();
    let chosen = checked_directory(chosen)?;
    // Only create fixed account-owned suffix components; reject links at each level.
    let mut directory = chosen.clone();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else { anyhow::bail!("目标目录无效"); };
        directory.push(name);
        if protected.iter().any(|root| directory.starts_with(root) || root.starts_with(&directory)) {
            anyhow::bail!("目标目录与现有账号数据重叠，请选择独立文件夹。");
        }
        match fs::create_dir(&directory) {
            Ok(()) => {},
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error.into()),
        }
        checked_directory(&directory)?;
        #[cfg(unix)]
        {
            fs::File::open(&directory)?.sync_all()?;
            fs::File::open(directory.parent().ok_or_else(|| anyhow::anyhow!("目标目录无父目录"))?)?.sync_all()?;
        }
    }
    Ok(MigrationPlan::prepare(source, &directory, protected)?)
}

fn poll_migration_plan(weak: Weak<AppWindow>, context: AppContext,
    pending: Rc<RefCell<Option<PendingAccountMigration>>>,
    receiver: Rc<mpsc::Receiver<std::result::Result<PendingAccountMigration, String>>>) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let Some(app) = weak.upgrade() else { return; };
        match receiver.try_recv() {
            Ok(Ok(prepared)) => {
                if !migration_lease_is_current(&context, &prepared.lease) {
                    show_migration_error(&app, "账号已变化，原文件未改动，请重新选择目录。"); return;
                }
                let state = app.global::<AppState>();
                state.set_directory_migration_message(format!("仅迁移当前账号，共 {} 个文件（{}），包含子文件夹。\n复制校验后切换保存位置，原文件保留；不会覆盖目标中的同名文件。", prepared.plan.files, format_storage_bytes(prepared.plan.bytes)).into());
                state.set_directory_migration_stage("confirm".into());
                *pending.borrow_mut() = Some(prepared);
            }
            Ok(Err(error)) => show_migration_error(&app, &error),
            Err(TryRecvError::Empty) => poll_migration_plan(weak, context, pending, receiver),
            Err(TryRecvError::Disconnected) => show_migration_error(&app, "目录检查未完成，原文件未改动。"),
        }
    });
}

fn start_directory_migration(app: &AppWindow, context: AppContext, prepared: PendingAccountMigration) {
    let Some(coordinator) = context.account_transition.clone() else {
        show_migration_error(app, "当前账号尚未就绪，请重新登录。"); return;
    };
    let worker = coordinator.directory_migration_worker();
    let state = app.global::<AppState>();
    state.set_directory_migration_stage("copying".into());
    state.set_directory_migration_progress(0);
    state.set_directory_migration_message("正在复制并校验文件，原文件将保留，请勿断开磁盘…".into());
    let progress = Arc::new(AtomicU64::new(0));
    let worker_progress = progress.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = match worker.begin(&prepared.lease) {
            Err(error) => AccountMigrationOutcome::Failed(error.to_string()),
            Ok(mut session) => {
                let copied = session.prepare(prepared.area, &prepared.plan.destination, &prepared.plan.manifest())
                    .map_err(std::io::Error::other).and_then(|()| prepared.plan.copy_retaining_source(
                    || session.commit(prepared.area, &prepared.plan.destination).map_err(std::io::Error::other),
                    |done, total| worker_progress.store(if total == 0 { 0 } else {
                        (done as f64 / total as f64 * 95.0) as u64
                    }, Ordering::Relaxed),
                ));
                match copied {
                    Err(error) => AccountMigrationOutcome::Failed(error.to_string()),
                    Ok(()) => match session.finish() {
                        Ok(lease) => AccountMigrationOutcome::Committed(lease),
                        Err(_) => AccountMigrationOutcome::RecoveryRequired,
                    },
                }
            }
        };
        let _ = sender.send(outcome);
    });
    poll_directory_migration(app.as_weak(), context, Rc::new(receiver), progress);
}

fn poll_directory_migration(weak: Weak<AppWindow>, context: AppContext,
    receiver: Rc<mpsc::Receiver<AccountMigrationOutcome>>, progress: Arc<AtomicU64>) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let Some(app) = weak.upgrade() else { return; };
        let state = app.global::<AppState>();
        state.set_directory_migration_progress(progress.load(Ordering::Relaxed) as i32);
        match receiver.try_recv() {
            Ok(AccountMigrationOutcome::Committed(lease)) => {
                let applied = context.account_transition.as_ref().ok_or_else(|| anyhow!("账号尚未就绪"))
                    .and_then(|coordinator| coordinator.finish_directory_migration_ui(&app, &context, &lease));
                if applied.is_err() {
                    show_migration_error(&app, "文件已复制并保存新位置，原文件也已保留。请重启客户端恢复账号状态。"); return;
                }
                state.set_directory_migration_progress(100);
                state.set_directory_migration_stage("done".into());
                state.set_directory_migration_message("当前账号目录迁移完成，保存位置已更新。原目录文件已保留作为恢复副本。".into());
            }
            Ok(AccountMigrationOutcome::Failed(error)) => show_migration_error(&app, &error),
            Ok(AccountMigrationOutcome::RecoveryRequired) => show_migration_error(&app, "文件已复制并保存新位置，原文件也已保留。请重启客户端恢复账号状态。"),
            Err(TryRecvError::Empty) => poll_directory_migration(weak, context, receiver, progress),
            Err(TryRecvError::Disconnected) => show_migration_error(&app, "迁移未能正常结束，请重启客户端检查保存位置；原文件已保留。"),
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
        || state.get_account_group_switching()
        || state.get_update_active()
}

fn show_migration_error(app: &AppWindow, message: &str) {
    let state = app.global::<AppState>();
    state.set_directory_migration_stage("error".into());
    state.set_directory_migration_message(message.into());
    state.set_directory_migration_open(true);
}


pub(super) fn sync_account_migrated_file_locations(app: &AppWindow, context: &AppContext, namespace: &UserNamespace) {
    let config = namespace.remap_locations();
    config.remap_store(&mut context.store.borrow_mut());
    context
        .canvas_history
        .borrow_mut()
        .remap_file_locations(&config);
    let state = app.global::<AppState>();
    state.set_input_dir(display_directory_path(&namespace.path(ManagedUserArea::Input)).into());
    state.set_output_dir(display_directory_path(&namespace.path(ManagedUserArea::Output)).into());
    state.set_prompt_dir(display_directory_path(&namespace.path(ManagedUserArea::Prompt)).into());
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

}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthenticated_directory_migration_rejects_before_io() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("private.txt"), b"unchanged").unwrap();
        wire_directory_migration_callbacks(&app, AppContext::default());
        let state = app.global::<AppState>();
        state.set_input_dir(source.display().to_string().into());
        state.set_directory_migration_source(source.display().to_string().into());
        state.set_directory_migration_target(destination.display().to_string().into());
        for kind in ["input", "output", "prompt"] {
            state.invoke_pick_dir(kind.into());
            assert_eq!(state.get_directory_migration_stage(), "error");
            state.set_directory_migration_stage("confirm".into());
            state.invoke_confirm_directory_migration();
            assert_eq!(state.get_directory_migration_stage(), "error");
            assert!(!state.get_directory_migration_busy());
            assert!(!destination.exists());
            assert_eq!(fs::read(source.join("private.txt")).unwrap(), b"unchanged");
        }
        state.invoke_close_directory_migration();
        assert!(!state.get_directory_migration_open());
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
        state.set_logged_in(true);
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
