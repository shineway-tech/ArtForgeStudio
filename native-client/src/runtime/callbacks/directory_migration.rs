use super::*;
use crate::directory_migration::{copy_batch_retaining_source, MigrationPlan};

struct PendingAccountMigration {
    lease: NamespaceLease,
    plans: Vec<(ManagedUserArea, MigrationPlan)>,
}

enum AccountMigrationOutcome {
    Committed(account_transition::AccountDirectoryMigrationCompletion),
    Failed(String),
    RecoveryRequired,
}

const MATERIAL_AREAS: [ManagedUserArea; 3] = [ManagedUserArea::Input, ManagedUserArea::Output, ManagedUserArea::Prompt];

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
            if !matches!(kind.as_str(), "all" | "materials" | "input" | "output" | "prompt") {
                show_migration_error(&app, "未知目录类型。"); return;
            }
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
            let common = material_directory_display(&lease.namespace);
            let source = if common.is_empty() { lease.namespace.root().to_path_buf() } else { PathBuf::from(&common) };
            let source_display = if common.is_empty() {
                MATERIAL_AREAS.iter().map(|&area| display_directory_path(&lease.namespace.path(area))).collect::<Vec<_>>().join("\n")
            } else { common };
            let Some(chosen) = rfd::FileDialog::new()
                .set_title("选择素材存储目录（统一迁移输入、输出和提示词模板）")
                .set_directory(&source).pick_folder() else { return; };
            if !migration_lease_is_current(&context, &lease) {
                show_migration_error(&app, "账号已变化，请重新选择迁移目录。"); return;
            }
            let target = chosen.join(MATERIAL_DIRECTORY_NAME);
            let protected = vec![app_data_dir(), lease.namespace.root().to_path_buf(),
                lease.namespace.path(ManagedUserArea::Input), lease.namespace.path(ManagedUserArea::Output),
                lease.namespace.path(ManagedUserArea::Prompt)];
            let planning_permit = match context.user_activity.begin_recovery_unit(&lease) {
                Ok(permit) => permit,
                Err(_) => { show_migration_error(&app, "账号正在切换，请稍后重试。"); return; }
            };
            let state = app.global::<AppState>();
            *pending.borrow_mut() = None;
            state.set_directory_migration_kind("materials".into());
            state.set_directory_migration_source(source_display.into());
            state.set_directory_migration_target(display_directory_path(&target).into());
            state.set_directory_migration_stage("checking".into());
            state.set_directory_migration_message("正在检查输入素材、输出和提示词目录…".into());
            state.set_directory_migration_open(true);
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _planning_permit = planning_permit;
                let result = prepare_account_material_migration(&data_root, &chosen, &lease.namespace, &protected)
                    .map(|plans| PendingAccountMigration { lease, plans });
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

fn prepare_account_material_migration(data_root: &DataRootCapability, chosen: &Path, namespace: &UserNamespace, protected: &[PathBuf]) -> Result<Vec<(ManagedUserArea, MigrationPlan)>> {
    let target = prepare_material_directory(data_root, chosen, namespace.user_public_id())?;
    let owner = material_directory_owner(&target, namespace.user_public_id())?;
    // The selected parent may be a drive root; only inspect/create children inside
    // the application-owned folder, never treat the drive itself as a material area.
    let target = crate::directory_migration::checked_directory(&target)?;
    for area in MATERIAL_AREAS {
        let directory = target.join(material_folder_name(area)?);
        if protected.iter().any(|root| crate::directory_migration::overlaps(&directory, root)) {
            anyhow::bail!("目标目录与现有账号数据重叠，请选择独立文件夹。");
        }
    }
    let mut plans = Vec::new();
    for area in MATERIAL_AREAS {
        let directory = target.join(material_folder_name(area)?);
        match fs::create_dir(&directory) {
            Ok(()) => {},
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error.into()),
        }
        crate::directory_migration::checked_directory(&directory)?;
        #[cfg(unix)]
        {
            fs::File::open(&directory)?.sync_all()?;
            fs::File::open(&target)?.sync_all()?;
        }
        plans.push((area, MigrationPlan::prepare(&namespace.path(area), &directory, protected)?));
    }
    anyhow::ensure!(material_directory_owner(&target, namespace.user_public_id())? == owner, "素材目录归属发生变化，请重新选择位置");
    Ok(plans)
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
                let files: usize = prepared.plans.iter().map(|(_, plan)| plan.files).sum();
                let bytes: u64 = prepared.plans.iter().map(|(_, plan)| plan.bytes).sum();
                state.set_directory_migration_message(format!("统一迁移当前账号的输入素材、生成图片和提示词模板，共 {files} 个文件（{}）。\n全部复制并校验成功后保存新位置，重启后从新目录读取素材。原文件保留。同名文件将覆盖，文件夹合并；确认后开始迁移。", format_storage_bytes(bytes)).into());
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
                let plans: Vec<_> = prepared.plans.iter().map(|(_, plan)| plan.clone()).collect();
                let copied = session.prepare_all(&prepared.plans)
                    .map_err(std::io::Error::other).and_then(|()| copy_batch_retaining_source(&plans,
                    || session.commit_all().map_err(std::io::Error::other),
                    |done, total| {
                        worker_progress.store(if total == 0 { 0 } else {
                            (done as f64 / total as f64 * 95.0) as u64
                        }, Ordering::Relaxed);
                        Ok(())
                    },
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
                state.set_directory_migration_stage("restarting".into());
                state.set_directory_migration_message("迁移完成，新目录已保存，正在重启软件。原文件已保留作为恢复副本。".into());
                schedule_material_restart(app.as_weak());
            }
            Ok(AccountMigrationOutcome::Failed(error)) => show_migration_error(&app, &error),
            Ok(AccountMigrationOutcome::RecoveryRequired) => show_migration_error(&app, "文件已复制并保存新位置，原文件也已保留。请重启客户端恢复账号状态。"),
            Err(TryRecvError::Empty) => poll_directory_migration(weak, context, receiver, progress),
            Err(TryRecvError::Disconnected) => show_migration_error(&app, "迁移未能正常结束，请重启客户端检查保存位置；原文件已保留。"),
        }
    });
}

fn schedule_material_restart(weak: Weak<AppWindow>) {
    // Leave the verified 100% state visible briefly. Normal shutdown below still
    // drains workers and flushes settings; the new process waits for this one.
    slint::Timer::single_shot(Duration::from_millis(1000), move || {
        let Some(app) = weak.upgrade() else { return; };
        if app.global::<AppState>().get_directory_migration_stage() != "restarting" { return; }
        match crate::restart::spawn_waiting_child() {
            Ok(mut child) => {
                if slint::quit_event_loop().is_err() {
                    let _ = child.kill();
                    let _ = child.wait();
                    show_migration_error(&app, "迁移已完成并保存新目录，但自动重启失败，请手动关闭后重新打开软件。");
                }
            }
            Err(_) => show_migration_error(&app, "迁移已完成并保存新目录，但自动重启失败，请手动关闭后重新打开软件。"),
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
    state.set_material_dir(material_directory_display(namespace).into());
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
        for kind in ["materials", "input", "output", "prompt"] {
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
    fn material_migration_dialog_shows_restart_notice_and_locks_during_restart() {
        use i_slint_backend_testing::ElementHandle;
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true, renderer_name: Some("software".into()), ..Default::default()
            },
        ))).unwrap();
        let app = AppWindow::new().unwrap();
        wire_directory_migration_callbacks(&app, AppContext::default());
        apply_theme(&app, "light");
        let state = app.global::<AppState>();
        state.set_contact_popup_open(false);
        state.set_directory_migration_open(true);
        state.set_directory_migration_source(["输入素材", "生成图片", "提示词模板"].map(|name|
            format!(r"C:\Users\示例用户\AppData\Local\ElunviCanvas\data\accounts\11111111-1111-4111-8111-111111111111\{name}"))
            .join("\n").into());
        state.set_directory_migration_target(r"D:\创作素材\Elunvi Canvas".into());
        state.set_directory_migration_message("正在复制并校验文件，原文件将保留，请勿断开磁盘…".into());
        state.set_directory_migration_stage("copying".into());
        state.set_directory_migration_progress(50);
        app.window().set_size(slint::LogicalSize::new(1180.0, 760.0));
        app.show().unwrap();
        let labels: Vec<_> = ElementHandle::find_by_element_type_name(&app, "Text")
            .filter_map(|element| element.accessible_label()).collect();
        assert!(labels.iter().any(|label| label.contains("迁移完成后将重启软件")));
        assert!(labels.iter().any(|label| label.as_str() == "50%"));
        let close = ElementHandle::find_by_element_id(&app, "DirectoryMigrationDialog::close-button").next().unwrap();
        let progress = ElementHandle::find_by_element_type_name(&app, "Text")
            .find(|element| element.accessible_label().is_some_and(|label| label.as_str() == "50%")).unwrap();
        assert!(progress.absolute_position().y + progress.size().height < close.absolute_position().y,
            "long source paths must not push progress out of the visible dialog");
        let track = ElementHandle::find_by_element_id(&app, "DirectoryMigrationDialog::progress-track").next().unwrap();
        let fill = ElementHandle::find_by_element_id(&app, "DirectoryMigrationDialog::progress-fill").next().unwrap();
        assert!((fill.absolute_position().x - track.absolute_position().x).abs() < 0.5,
            "progress must grow from the left edge, not the center");
        assert!((fill.size().width - track.size().width * 0.5).abs() < 0.5);
        let progress_y = progress.absolute_position().y;
        let previous_source = state.get_directory_migration_source();
        state.set_directory_migration_source(previous_source.repeat(5).into());
        assert!((progress.absolute_position().y - progress_y).abs() < 0.5,
            "progress stays visible even when directory details must scroll");
        state.set_directory_migration_source(previous_source);
        if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
            let directory = PathBuf::from(directory);
            fs::create_dir_all(&directory).unwrap();
            let pixels = app.window().take_snapshot().unwrap();
            image::save_buffer(directory.join("material-migration-progress.png"), pixels.as_bytes(), pixels.width(), pixels.height(), image::ColorType::Rgba8).unwrap();
        }
        state.set_directory_migration_stage("restarting".into());
        state.set_directory_migration_progress(100);
        assert!(state.get_directory_migration_busy());
        state.invoke_close_directory_migration();
        assert!(state.get_directory_migration_open());
    }

    #[test]
    fn material_migration_prepares_readable_folders_inside_the_selected_location() {
        let source = tempfile::tempdir().unwrap();
        let chosen = tempfile::tempdir().unwrap();
        let namespace = UserNamespace::new(source.path(), "11111111-1111-4111-8111-111111111111").unwrap();
        for area in MATERIAL_AREAS {
            fs::create_dir_all(namespace.path(area)).unwrap();
            fs::write(namespace.path(area).join("sample.txt"), area.storage_name()).unwrap();
        }
        let target = chosen.path().join("Elunvi Canvas");
        let data_root = NamespaceFs::open_data_root(source.path()).unwrap();
        for selected in [chosen.path().to_owned(), fs::canonicalize(chosen.path()).unwrap()] {
        let plans = prepare_account_material_migration(&data_root, &selected, &namespace, &[source.path().to_owned()]).unwrap();
        assert_eq!(plans.len(), 3);
        for ((_, plan), folder) in plans.iter().zip(["输入素材", "生成图片", "提示词模板"]) {
            assert_eq!(plan.destination, fs::canonicalize(target.join(folder)).unwrap());
            assert_eq!(plan.files, 1);
            assert!(!plan.destination.join("sample.txt").exists(), "planning must not copy before confirmation");
        }
        }
    }

    #[test]
    fn basic_settings_has_one_material_migration_entry() {
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
        state.set_settings_section("basic".into());
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_material_dir(r"E:\我的素材\ElunviCanvas\accounts\fixture".into());
        let observed = Rc::new(RefCell::new(Vec::new()));
        let captured = observed.clone();
        state.on_pick_dir(move |kind| captured.borrow_mut().push(kind.to_string()));
        app.window().set_size(slint::LogicalSize::new(1440.0, 1500.0));
        app.show().unwrap();
        let buttons: Vec<_> = ElementHandle::find_by_element_id(&app, "DirectorySettings::migrate-all").collect();
        assert_eq!(buttons.len(), 1, "the three folders share one migration entry");
        buttons[0].mock_single_click(PointerEventButton::Left);
        assert_eq!(observed.borrow().as_slice(), &["all"]);
        if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
            let directory = PathBuf::from(directory);
            fs::create_dir_all(&directory).unwrap();
            let pixels = app.window().take_snapshot().unwrap();
            image::save_buffer(directory.join("materials-directory-settings.png"), pixels.as_bytes(), pixels.width(), pixels.height(), image::ColorType::Rgba8).unwrap();
        }
    }

    #[test]
    fn about_config_path_migration_requests_all_material_folders() {
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
        state.set_contact_popup_open(false);
        state.set_material_dir(r"E:\我的素材\ElunviCanvas\accounts\fixture".into());
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
        assert_eq!(observed.borrow().as_slice(), &["all"]);
        assert_eq!(state.get_material_dir(), r"E:\我的素材\ElunviCanvas\accounts\fixture");
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
    #[test]
    fn unified_directory_migration_plans_existing_scattered_sources() {
        let root = tempfile::tempdir().unwrap();
        let previous = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let mut namespace = UserNamespace::new(root.path(), "11111111-1111-4111-8111-111111111111").unwrap();
        for area in MATERIAL_AREAS { fs::create_dir_all(namespace.path(area)).unwrap(); }
        let old_prompt = namespace.path(ManagedUserArea::Prompt);
        let moved_prompt = previous.path().join("ElunviCanvas/accounts").join(namespace.user_public_id()).join("prompt");
        fs::create_dir_all(&moved_prompt).unwrap();
        namespace = namespace.with_mapping(AccountDirectoryMapping {
            version: 1, area: "prompt".into(), source: old_prompt.clone(), target: moved_prompt.clone(),
            identity: NamespaceFs::directory_identity_at(&moved_prompt).unwrap(),
            source_identity: NamespaceFs::directory_identity_at(&old_prompt).unwrap(),
            manifest_json: "[]".into(), pending_rebind: Vec::new(), material_owner: None,
        }).unwrap();
        for area in MATERIAL_AREAS { fs::write(namespace.path(area).join("sample.txt"), material_folder_name(area).unwrap()).unwrap(); }
        let protected: Vec<_> = MATERIAL_AREAS.iter().map(|area| namespace.path(*area)).collect();
        let data_root = NamespaceFs::open_data_root(root.path()).unwrap();
        let prepared = prepare_account_material_migration(&data_root, target.path(), &namespace, &protected).unwrap();
        assert_eq!(prepared[2].1.source, fs::canonicalize(moved_prompt).unwrap());
        let plans: Vec<_> = prepared.into_iter().map(|(_, plan)| plan).collect();
        copy_batch_retaining_source(&plans, || Ok(()), |_, _| Ok(())).unwrap();
        for area in MATERIAL_AREAS {
            assert_eq!(fs::read(target.path().join(MATERIAL_DIRECTORY_NAME).join(material_folder_name(area).unwrap()).join("sample.txt")).unwrap(), material_folder_name(area).unwrap().as_bytes());
            assert!(namespace.path(area).join("sample.txt").exists());
        }
        assert!(!target.path().join("ElunviCanvas").exists());
        assert!(target.path().join(MATERIAL_DIRECTORY_NAME).is_dir());
    }
}
