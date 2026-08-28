use super::*;

const CUTOUT_MIN_EDGE: u32 = 33;
const CUTOUT_POLL_RETRY_LIMIT: usize = 4;

#[derive(Clone, Copy)]
enum CutoutSourceError {
    Unsupported,
    TooSmall(u32),
}

enum ImageCutoutOutcome {
    Accepted {
        task_id: String,
    },
    Progress {
        percent: i32,
    },
    Success {
        bytes: Vec<u8>,
        delivery: DeliveryConfirmation,
    },
    Recovered {
        local_path: String,
        delivery: Option<DeliveryConfirmation>,
    },
    CreditInsufficient {
        message: String,
    },
    Failure {
        reason: String,
    },
}

pub(super) fn wire_image_cutout_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_submit_cutout(move |subject_type| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_image_cutout(&app, context.clone(), subject_type.as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reveal_cutout_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_cutout_result_path().to_string());
            if !path.is_file() {
                state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        "No cutout image is available yet"
                    } else {
                        "暂无可保存的抠图结果"
                    }
                    .into(),
                );
                return;
            }
            let default_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("抠图结果.png");
            let Some(destination) = rfd::FileDialog::new()
                .add_filter("PNG", &["png"])
                .set_file_name(default_name)
                .save_file()
            else {
                return;
            };
            let destination = normalize_cutout_destination(destination);
            let result = if destination == path {
                Ok(())
            } else {
                fs::read(&path).and_then(|bytes| {
                    atomic_write_file(&destination, &bytes)
                        .map_err(|error| std::io::Error::other(error.to_string()))
                })
            };
            match result {
                Ok(()) => state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        "Saved the PNG image"
                    } else {
                        "抠图结果已保存到本地"
                    }
                    .into(),
                ),
                Err(error) => state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        format!("Failed to save the PNG image: {error}")
                    } else {
                        format!("保存抠图结果失败：{error}")
                    }
                    .into(),
                ),
            }
        });
    }
}

fn normalize_cutout_destination(mut path: PathBuf) -> PathBuf {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("png"))
    {
        path.set_extension("png");
    }
    path
}

fn normalized_cutout_type(value: &str) -> Option<&'static str> {
    match value {
        "general" => Some("general"),
        "portrait" => Some("portrait"),
        "avatar" => Some("avatar"),
        "skin" => Some("skin"),
        "product" => Some("product"),
        "clothing" => Some("clothing"),
        "sky" => Some("sky"),
        _ => None,
    }
}

fn cutout_type_label(value: &str) -> &'static str {
    match value {
        "portrait" => "人像",
        "avatar" => "头像",
        "skin" => "皮肤",
        "product" => "商品",
        "clothing" => "服饰",
        "sky" => "天空",
        _ => "通用",
    }
}

fn minimum_cutout_edge(subject_type: &str) -> u32 {
    if matches!(subject_type, "clothing" | "sky") {
        51
    } else {
        CUTOUT_MIN_EDGE
    }
}

fn validate_cutout_dimensions(
    width: u32,
    height: u32,
    subject_type: &str,
) -> std::result::Result<(), CutoutSourceError> {
    let min_edge = minimum_cutout_edge(subject_type);
    if width < min_edge || height < min_edge {
        return Err(CutoutSourceError::TooSmall(min_edge));
    }
    Ok(())
}

fn validate_cutout_source(
    path: &Path,
    subject_type: &str,
) -> std::result::Result<(), CutoutSourceError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return Err(CutoutSourceError::Unsupported);
    }
    let (width, height) =
        inspect_image_dimensions(path).map_err(|_| CutoutSourceError::Unsupported)?;
    validate_cutout_dimensions(width, height, subject_type)
}

fn set_cutout_source_error(app: &AppWindow, error: CutoutSourceError) {
    let state = app.global::<AppState>();
    let english = state.get_language().as_str() == "en";
    let message = match error {
        CutoutSourceError::Unsupported => {
            if english {
                "Cutout supports JPG, PNG and WebP images"
            } else {
                "抠图仅支持 JPG、PNG 和 WebP 图片"
            }
        }
        CutoutSourceError::TooSmall(min_edge) => {
            if english {
                return state.set_cutout_message(
                    format!("Both image edges must be at least {min_edge} pixels").into(),
                );
            } else {
                return state
                    .set_cutout_message(format!("图片宽高均不能小于 {min_edge} 像素").into());
            }
        }
    };
    state.set_cutout_message(message.into());
}

fn prepare_current_cutout_source(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    subject_type: &str,
) -> std::result::Result<PathBuf, CutoutSourceError> {
    let state = app.global::<AppState>();
    let id = state.get_viewer_id().to_string();
    let source = state.get_viewer_source().to_string();
    let original_path = {
        let store = store.borrow();
        viewer_item(&store, &id, &source).map(|item| item.source_path.clone())
    };
    let persisted = original_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "failed" && *value != "asset")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .map(|path| persist_reference_source(&path))
        .unwrap_or_else(|| persist_slint_reference(&state.get_viewer_image()))
        .map_err(|_| CutoutSourceError::Unsupported)?;
    validate_cutout_source(&persisted, subject_type)?;
    Ok(persisted)
}

fn start_image_cutout(app: &AppWindow, context: AppContext, subject_type: &str) {
    let state = app.global::<AppState>();
    if state.get_session_state().as_str() != "online" {
        state.set_auth_open(true);
        state.set_cutout_message(
            if state.get_language().as_str() == "en" {
                "Sign in and connect to the service before starting cutout"
            } else {
                "请先登录并连接服务后再开始抠图"
            }
            .into(),
        );
        return;
    }
    if context.backend.is_none() || state.get_cutout_processing() {
        return;
    }
    let Some(session_scope) = current_generation_session_scope(&context) else {
        state.set_cutout_message("登录状态已变化，请重新发起抠图".into());
        return;
    };
    let Some(subject_type) = normalized_cutout_type(subject_type) else {
        state.set_cutout_message("请选择有效的抠图类型".into());
        return;
    };
    let source_path = match prepare_current_cutout_source(app, &context.store, subject_type) {
        Ok(path) => path,
        Err(error) => {
            set_cutout_source_error(app, error);
            return;
        }
    };

    let source_title = {
        let title = state.get_viewer_title().to_string();
        if title.trim().is_empty() {
            "图片".to_string()
        } else {
            title
        }
    };
    let client_request_id = Uuid::new_v4().simple().to_string();
    let (reference_sha256, reference_size_bytes) =
        match reference_fingerprints(std::slice::from_ref(&source_path)) {
            Ok(fingerprints) => fingerprints,
            Err(error) => {
                state.set_cutout_message(format!("原图校验失败：{error}").into());
                return;
            }
        };
    let record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id,
        owner_user_id: session_scope.owner_user_id.clone(),
        auth_epoch: session_scope.auth_epoch,
        local_task_id: Uuid::new_v4().to_string(),
        server_task_id: String::new(),
        raw_prompt: source_title,
        generation_prompt: "智能抠图".to_string(),
        task_type: "image_cutout".to_string(),
        category: "other".to_string(),
        mode: "game".to_string(),
        ratio: String::new(),
        quality: subject_type.to_string(),
        model_code: "aliyun_image_segmentation".to_string(),
        conversation_id: String::new(),
        count: 1,
        target_width: 0,
        target_height: 0,
        create_conversation: false,
        reference_paths: vec![source_path.display().to_string()],
        reference_sha256,
        reference_size_bytes,
        lineage_reference_paths: vec![source_path.display().to_string()],
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
        canvas_source_node_id: String::new(),
        canvas_ui_extraction: false,
    };
    if upsert_pending_generation_scoped(
        record.clone(),
        &session_scope.owner_user_id,
        session_scope.auth_epoch,
    )
    .is_err()
    {
        state.set_cutout_message(
            if state.get_language().as_str() == "en" {
                "The task could not be saved locally"
            } else {
                "任务准备失败，请重试"
            }
            .into(),
        );
        return;
    }
    launch_image_cutout(app, context, record, false);
}

pub(super) fn resume_pending_image_cutout(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
) {
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !generation_scope_matches_context(&context, &session_scope) {
        return;
    }
    if app.global::<AppState>().get_cutout_processing() {
        return;
    }
    let state = app.global::<AppState>();
    state.set_viewer_open(false);
    state.set_cutout_open(true);
    state.set_viewer_title(record.raw_prompt.clone().into());
    state.set_cutout_type(
        normalized_cutout_type(&record.quality)
            .unwrap_or("general")
            .into(),
    );
    if let Some(source_path) = record.reference_paths.first() {
        let path = PathBuf::from(source_path);
        if path.is_file() {
            state.set_viewer_source_path(path.display().to_string().into());
            if let Ok(image) = load_preview_image(&path, PreviewPurpose::Canvas) {
                state.set_viewer_image(image);
            }
        }
    }
    launch_image_cutout(app, context, record, true);
}

fn launch_image_cutout(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
    recovering: bool,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let session_scope = SessionScope {
        owner_user_id: record.owner_user_id.clone(),
        auth_epoch: record.auth_epoch,
    };
    if !generation_scope_matches_context(&context, &session_scope) {
        return;
    }
    let state = app.global::<AppState>();
    state.set_cutout_processing(true);
    state.set_cutout_progress(if recovering { 5 } else { 1 });
    state.set_cutout_type(
        normalized_cutout_type(&record.quality)
            .unwrap_or("general")
            .into(),
    );
    state.set_cutout_estimated_credits("20".into());
    state.set_cutout_result_path("".into());
    state.set_cutout_result_name("".into());
    state.set_cutout_result_image(Image::default());
    state.set_cutout_message(
        if state.get_language().as_str() == "en" {
            if recovering {
                "Recovering the cutout task..."
            } else {
                "Uploading the image..."
            }
        } else if recovering {
            "正在恢复未完成的抠图任务..."
        } else {
            "正在上传图片..."
        }
        .into(),
    );

    let source_path = record.reference_paths.first().cloned().unwrap_or_default();
    let source_title = record.raw_prompt.clone();
    let subject_type = normalized_cutout_type(&record.quality)
        .unwrap_or("general")
        .to_string();
    let (sender, receiver) = mpsc::channel::<ImageCutoutOutcome>();
    let worker_scope = session_scope.clone();
    std::thread::spawn(move || run_image_cutout_worker(backend, worker_scope, record, sender));
    poll_image_cutout_outcomes(
        app.as_weak(),
        context,
        session_scope,
        Rc::new(RefCell::new(Some(receiver))),
        source_path,
        source_title,
        subject_type,
    );
}

fn run_image_cutout_worker(
    backend: Arc<BackendRuntime>,
    session_scope: SessionScope,
    mut record: PendingGenerationRecord,
    sender: mpsc::Sender<ImageCutoutOutcome>,
) {
    if record.owner_user_id != session_scope.owner_user_id
        || record.auth_epoch != session_scope.auth_epoch
        || !backend_generation_scope_active(&backend, &session_scope)
    {
        return;
    }
    let api = GenerationApi::new(backend.api.clone());
    let verified_delivery_file_ids = match sanitize_recovered_delivery_paths(&mut record) {
        Ok(file_ids) => file_ids,
        Err(_) => {
            let _ = sender.send(ImageCutoutOutcome::Failure {
                reason: "本地抠图恢复记录无法安全更新，已暂停交付，请重启后重试"
                    .to_string(),
            });
            return;
        }
    };
    if record.terminal {
        if let Some(saved) = record
            .deliveries
            .iter()
            .find(|delivery| verified_delivery_file_ids.contains(&delivery.file_id))
        {
            let delivery = (!saved.acknowledged).then(|| DeliveryConfirmation {
                client_request_id: record.client_request_id.clone(),
                item_index: saved.item_index,
                task_id: record.server_task_id.clone(),
                file_id: saved.file_id.clone(),
                sha256: saved.sha256.clone(),
                size_bytes: saved.size_bytes,
                failed_asset_id: None,
            });
            let _ = sender.send(ImageCutoutOutcome::Recovered {
                local_path: saved.local_path.clone(),
                delivery,
            });
            return;
        }
    }

    let mut uploaded = record.uploaded_file_ids.clone();
    if uploaded.is_empty() && record.server_task_id.is_empty() {
        if !generation_references_match(&record) {
            let _ = sender.send(ImageCutoutOutcome::Failure {
                reason: "原图内容已变化，恢复任务已暂停，请重新发起".to_string(),
            });
            return;
        }
        let Some(path) = record.reference_paths.first() else {
            let _ = sender.send(ImageCutoutOutcome::Failure {
                reason: "找不到待处理的原图，请重新选择".to_string(),
            });
            return;
        };
        match api.upload_reference_scoped(Path::new(path), &session_scope) {
            Ok(file_id) => {
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                if !matches!(
                    update_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                        |item| item.uploaded_file_ids = snapshot,
                    ),
                    Ok(true)
                ) {
                    if let Some(file_id) = uploaded.last() {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    return;
                }
            }
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                }
                let _ = sender.send(ImageCutoutOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    }

    let mut detail = if record.server_task_id.is_empty() {
        let request = CreateImageCutout {
            client_request_id: record.client_request_id.clone(),
            reference_file_id: uploaded[0].clone(),
            subject_type: normalized_cutout_type(&record.quality)
                .unwrap_or("general")
                .to_string(),
        };
        match api.create_image_cutout_scoped(&request, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if error.is_insufficient_credits() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                    let _ = sender.send(ImageCutoutOutcome::CreditInsufficient {
                        message: "本次智能抠图需要 20 积分，请先充值".to_string(),
                    });
                    return;
                }
                if !backend_generation_scope_active(&backend, &session_scope) {
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        let _ = api.delete_reference_scoped(file_id, &session_scope);
                    }
                    let _ = remove_pending_generation_scoped(
                        &session_scope.owner_user_id,
                        session_scope.auth_epoch,
                        &record.client_request_id,
                    );
                }
                let _ = sender.send(ImageCutoutOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    } else {
        match fetch_image_cutout_task(&api, &record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ImageCutoutOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    };

    record.server_task_id = detail.id.clone();
    let server_task_id = detail.id.clone();
    let server_id_snapshot = server_task_id.clone();
    let uploaded_snapshot = uploaded.clone();
    if !matches!(
        update_pending_generation_scoped(
            &session_scope.owner_user_id,
            session_scope.auth_epoch,
            &record.client_request_id,
            |item| {
                item.server_task_id = server_id_snapshot;
                item.uploaded_file_ids = uploaded_snapshot;
            },
        ),
        Ok(true)
    ) {
        return;
    }
    let _ = sender.send(ImageCutoutOutcome::Accepted {
        task_id: server_task_id.clone(),
    });

    loop {
        if !backend_generation_scope_active(&backend, &session_scope) {
            return;
        }
        let _ = sender.send(ImageCutoutOutcome::Progress {
            percent: detail.progress_percent,
        });
        if let Some(item) = detail.items.iter().find(|item| item.status == "succeeded") {
            if let Some(file) = item.file.as_ref() {
                match api.download_verified_scoped(file, &session_scope) {
                    Ok(bytes) => {
                        if !matches!(
                            update_pending_generation_scoped(
                                &session_scope.owner_user_id,
                                session_scope.auth_epoch,
                                &record.client_request_id,
                                |pending| {
                                    pending.terminal = true;
                                    pending.expected_success_count = 1;
                                },
                            ),
                            Ok(true)
                        ) {
                            return;
                        }
                        let _ = sender.send(ImageCutoutOutcome::Success {
                            bytes,
                            delivery: DeliveryConfirmation {
                                client_request_id: record.client_request_id.clone(),
                                item_index: item.index,
                                task_id: server_task_id,
                                file_id: file.id.clone(),
                                sha256: file.sha256.clone(),
                                size_bytes: file.size_bytes.parse().unwrap_or(0),
                                failed_asset_id: None,
                            },
                        });
                        return;
                    }
                    Err(error) if detail.terminal() => {
                        let _ = sender.send(ImageCutoutOutcome::Failure {
                            reason: error.generation_message(),
                        });
                        return;
                    }
                    Err(_) => {}
                }
            }
        }
        if detail.terminal() {
            let reason = detail
                .failure
                .as_ref()
                .map(|failure| failure.message.clone())
                .or_else(|| {
                    detail.items.iter().find_map(|item| {
                        item.failure.as_ref().map(|failure| failure.message.clone())
                    })
                })
                .unwrap_or_else(|| "服务端未能完成智能抠图".to_string());
            if !matches!(
                update_pending_generation_scoped(
                    &session_scope.owner_user_id,
                    session_scope.auth_epoch,
                    &record.client_request_id,
                    |pending| {
                        pending.terminal = true;
                        pending.expected_success_count = 0;
                    },
                ),
                Ok(true)
            ) {
                return;
            }
            let _ = sender.send(ImageCutoutOutcome::Failure { reason });
            return;
        }
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        detail = match fetch_image_cutout_task(&api, &record.server_task_id, &session_scope) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ImageCutoutOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        };
    }
}

fn fetch_image_cutout_task(
    api: &GenerationApi,
    task_id: &str,
    session_scope: &SessionScope,
) -> std::result::Result<GenerationTaskDetail, ApiError> {
    let mut retries = 0;
    loop {
        match api.task_scoped(task_id, session_scope) {
            Ok(detail) => return Ok(detail),
            Err(error)
                if error.should_preserve_generation_recovery()
                    && retries < CUTOUT_POLL_RETRY_LIMIT =>
            {
                retries += 1;
                std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
            }
            Err(error) => return Err(error),
        }
    }
}

fn poll_image_cutout_outcomes(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    session_scope: SessionScope,
    receiver: Rc<RefCell<Option<mpsc::Receiver<ImageCutoutOutcome>>>>,
    source_path: String,
    source_title: String,
    subject_type: String,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(ImageCutoutOutcome::Failure {
                        reason: "智能抠图任务已中断，请重试".to_string(),
                    })
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_image_cutout_outcomes(
                app_weak,
                context,
                session_scope,
                receiver,
                source_path,
                source_title,
                subject_type,
            );
            return;
        };
        if !generation_scope_allows_polling(&app_weak, &context, &session_scope) {
            receiver.borrow_mut().take();
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        match outcome {
            ImageCutoutOutcome::Accepted { task_id } => {
                state.set_cutout_progress(state.get_cutout_progress().max(8));
                state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        format!("Task {task_id} is queued")
                    } else {
                        "任务已提交，正在排队处理...".to_string()
                    }
                    .into(),
                );
            }
            ImageCutoutOutcome::Progress { percent } => {
                state.set_cutout_progress(state.get_cutout_progress().max(percent.clamp(1, 99)));
                state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        "Extracting the selected subject..."
                    } else {
                        "正在识别主体并生成透明背景..."
                    }
                    .into(),
                );
            }
            ImageCutoutOutcome::Success { bytes, delivery } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                match save_image_cutout_asset(
                    &app,
                    &context.store,
                    &source_path,
                    &source_title,
                    &subject_type,
                    &bytes,
                ) {
                    Ok((result_path, result_image)) => {
                        let result_name = Path::new(&result_path)
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("抠图结果.png")
                            .to_string();
                        state.set_cutout_result_path(result_path.clone().into());
                        state.set_cutout_result_name(result_name.into());
                        state.set_cutout_result_image(result_image);
                        state.set_cutout_progress(100);
                        state.set_cutout_processing(false);
                        state.set_cutout_message(
                            if state.get_language().as_str() == "en" {
                                "Cutout saved to My Assets / Other"
                            } else {
                                "抠图完成，已保存到“我的资产 / 其他”"
                            }
                            .into(),
                        );
                        let saved = pending_delivery_saved(
                            &session_scope.owner_user_id,
                            session_scope.auth_epoch,
                            &delivery.client_request_id,
                            &delivery,
                            &result_path,
                        );
                        if matches!(saved, Ok(true)) {
                            acknowledge_delivery_after_local_save(
                                app.as_weak(),
                                context.clone(),
                                session_scope.clone(),
                                delivery,
                            );
                        }
                    }
                    Err(error) => {
                        state.set_cutout_processing(false);
                        state.set_cutout_message(format!("抠图结果保存失败：{error}").into());
                    }
                }
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            ImageCutoutOutcome::Recovered {
                local_path,
                delivery,
            } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let path = PathBuf::from(&local_path);
                let locally_verified = delivery.as_ref().map_or(true, |delivery| {
                    recovered_delivery_path_matches(
                        &local_path,
                        &delivery.sha256,
                        delivery.size_bytes,
                    )
                });
                match (
                    locally_verified,
                    load_preview_image(&path, PreviewPurpose::Canvas),
                ) {
                    (true, Ok(image)) => {
                        state.set_cutout_result_path(local_path.clone().into());
                        state.set_cutout_result_name(
                            path.file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or("抠图结果.png")
                                .into(),
                        );
                        state.set_cutout_result_image(image);
                        state.set_cutout_progress(100);
                        state.set_cutout_processing(false);
                        state.set_cutout_message("已恢复上次完成的智能抠图结果".into());
                        if let Some(delivery) = delivery {
                            acknowledge_delivery_after_local_save(
                                app.as_weak(),
                                context.clone(),
                                session_scope.clone(),
                                delivery,
                            );
                        }
                    }
                    _ => {
                        let retrying = delivery.as_ref().is_some_and(|delivery| {
                            matches!(
                                clear_recovered_delivery_local_path(
                                    &session_scope,
                                    &delivery.client_request_id,
                                    &delivery.file_id,
                                ),
                                Ok(true)
                            )
                        });
                        if retrying {
                            state.set_cutout_processing(false);
                            state.set_cutout_message(
                                "本地抠图结果校验失败，正在从服务端重新下载...".into(),
                            );
                            recover_pending_generations(&app, context.clone());
                        } else {
                            state.set_cutout_processing(false);
                            state.set_cutout_message(
                                "本地抠图结果已损坏，且暂时无法恢复，请重启后重试".into(),
                            );
                        }
                    }
                }
            }
            ImageCutoutOutcome::CreditInsufficient { message } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                state.set_cutout_processing(false);
                state.set_cutout_progress(0);
                state.set_cutout_message(message.clone().into());
                state.set_credit_insufficient_message(message.into());
                state.set_credit_insufficient_open(true);
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            ImageCutoutOutcome::Failure { reason } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                state.set_cutout_processing(false);
                state.set_cutout_progress(0);
                state.set_cutout_message(reason.into());
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
        }
        if keep_polling {
            poll_image_cutout_outcomes(
                app_weak,
                context,
                session_scope,
                receiver,
                source_path,
                source_title,
                subject_type,
            );
        }
    });
}

fn decode_cutout_result(
    source_path: &Path,
    subject_type: &str,
    bytes: &[u8],
) -> Result<(Vec<u8>, i32, i32)> {
    if image::guess_format(bytes)? != image::ImageFormat::Png {
        return Err(anyhow!("服务端抠图结果不是 PNG 图片"));
    }
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?;
    let result = if decoded.color().has_alpha() {
        decoded.to_rgba8()
    } else if matches!(subject_type, "skin" | "sky") {
        apply_cutout_mask(source_path, &decoded)?
    } else {
        return Err(anyhow!("服务端抠图结果缺少透明通道"));
    };
    let (width, height) = result.dimensions();
    if width == 0 || height == 0 {
        return Err(anyhow!("服务端抠图结果尺寸无效"));
    }
    let result_bytes = if decoded.color().has_alpha() {
        bytes.to_vec()
    } else {
        encode_png_rgba(&result, width, height)?
    };
    Ok((result_bytes, width as i32, height as i32))
}

fn apply_cutout_mask(source_path: &Path, mask: &image::DynamicImage) -> Result<image::RgbaImage> {
    let source = image::ImageReader::open(source_path)
        .with_context(|| format!("无法读取抠图原图 {}", source_path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("无法解码抠图原图 {}", source_path.display()))?;
    let source = source.to_rgb8();
    let (width, height) = source.dimensions();
    if width == 0 || height == 0 || mask.width() == 0 || mask.height() == 0 {
        return Err(anyhow!("抠图原图或蒙版尺寸无效"));
    }
    let alpha = mask
        .resize_exact(width, height, image::imageops::FilterType::Lanczos3)
        .to_luma8();
    let mut result = image::RgbaImage::new(width, height);
    for (x, y, pixel) in source.enumerate_pixels() {
        result.put_pixel(
            x,
            y,
            image::Rgba([pixel[0], pixel[1], pixel[2], alpha.get_pixel(x, y)[0]]),
        );
    }
    Ok(result)
}

fn save_image_cutout_asset(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    source_path: &str,
    source_title: &str,
    subject_type: &str,
    bytes: &[u8],
) -> Result<(String, Image)> {
    let (result_bytes, width, height) =
        decode_cutout_result(Path::new(source_path), subject_type, bytes)?;
    let source_title = if source_title.trim().is_empty() {
        Path::new(source_path)
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("图片")
    } else {
        source_title.trim()
    };
    let title = format!("{} 抠图", short_text(source_title, 18));
    let result_path = save_generated_bytes(app, &result_bytes, &title)?;
    let image = load_preview_image(Path::new(&result_path), PreviewPurpose::Canvas)?;
    let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
    let prompt = format!("智能抠图（{}）", cutout_type_label(subject_type));
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: String::new(),
        title: title.clone(),
        category: "other".to_string(),
        kind: "game".to_string(),
        time: now.clone(),
        prompt,
        ratio: ratio_from_actual_dimensions(width, height),
        quality: quality_from_actual_dimensions(width, height),
        model: "智能抠图".to_string(),
        origin: "image_cutout".to_string(),
        width,
        height,
        source_path: result_path.clone(),
        reference_paths: (!source_path.is_empty())
            .then(|| source_path.to_string())
            .into_iter()
            .collect(),
        cutout_done: true,
        remove_black_done: false,
        upscale_done: false,
        is_new: false,
        delivery_recoverable: false,
        delivery_downloading: false,
    };
    let notification = NotificationData {
        id: Uuid::new_v4().to_string(),
        title: format!("智能抠图完成：{title}"),
        model: "智能抠图".to_string(),
        time: now,
        reason: String::new(),
        success: true,
        read: false,
    };
    let item_id = item.id.clone();
    let notification_id = notification.id.clone();
    let mut store = store.borrow_mut();
    store.assets.insert(0, item);
    store.notifications.insert(0, notification);
    if let Err(error) = save_local_store_checked(app, &store) {
        if store.assets.first().is_some_and(|item| item.id == item_id) {
            store.assets.remove(0);
        }
        if store
            .notifications
            .first()
            .is_some_and(|item| item.id == notification_id)
        {
            store.notifications.remove(0);
        }
        return Err(error);
    }
    push_all(app, &store);
    Ok((result_path, image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutout_subject_types_match_the_server_contract() {
        for value in [
            "general", "portrait", "avatar", "skin", "product", "clothing", "sky",
        ] {
            assert_eq!(normalized_cutout_type(value), Some(value));
        }
        assert_eq!(normalized_cutout_type("unknown"), None);
    }

    #[test]
    fn general_cutout_accepts_a_1536_by_2048_source() {
        assert!(validate_cutout_dimensions(1536, 2048, "general").is_ok());
        assert!(validate_cutout_dimensions(33, 33, "general").is_ok());
        assert!(validate_cutout_dimensions(50, 50, "clothing").is_err());
        assert!(validate_cutout_dimensions(51, 51, "clothing").is_ok());
        assert!(validate_cutout_dimensions(50, 50, "sky").is_err());
        assert!(validate_cutout_dimensions(51, 51, "sky").is_ok());
    }

    #[test]
    fn alpha_cutout_result_is_kept_as_a_png() {
        let rgba = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 0]));
        let bytes = encode_png_rgba(&rgba, 2, 2).expect("encode png");
        let (result, width, height) =
            decode_cutout_result(Path::new("unused"), "general", &bytes).expect("decode result");
        assert_eq!(result, bytes);
        assert_eq!((width, height), (2, 2));

        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&[1, 2, 3], 1, 1, image::ExtendedColorType::Rgb8)
            .expect("encode jpeg");
        assert!(decode_cutout_result(Path::new("unused"), "general", &jpeg).is_err());
    }

    #[test]
    fn skin_and_sky_masks_are_composed_with_the_local_source() {
        let source_path = std::env::temp_dir().join(format!(
            "artforge-cutout-mask-source-{}.png",
            Uuid::new_v4()
        ));
        let source =
            image::RgbImage::from_fn(2, 2, |x, y| image::Rgb([10 + x as u8, 20 + y as u8, 30]));
        source
            .save_with_format(&source_path, image::ImageFormat::Png)
            .expect("write source");

        let mask = image::GrayImage::from_raw(2, 2, vec![0, 64, 128, 255]).expect("mask");
        let mut mask_bytes = Vec::new();
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(Cursor::new(&mut mask_bytes)),
            mask.as_raw(),
            2,
            2,
            image::ExtendedColorType::L8,
        )
        .expect("encode mask");

        for subject_type in ["skin", "sky"] {
            let (result_bytes, width, height) =
                decode_cutout_result(&source_path, subject_type, &mask_bytes)
                    .expect("compose mask");
            let result =
                image::load_from_memory_with_format(&result_bytes, image::ImageFormat::Png)
                    .expect("decode composed result")
                    .to_rgba8();
            assert_eq!((width, height), (2, 2));
            assert_eq!(result.get_pixel(0, 0).0, [10, 20, 30, 0]);
            assert_eq!(result.get_pixel(1, 1).0, [11, 21, 30, 255]);
        }

        assert!(decode_cutout_result(&source_path, "general", &mask_bytes).is_err());
        let _ = fs::remove_file(source_path);
    }
}
