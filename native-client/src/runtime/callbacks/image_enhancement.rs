use super::*;

const ENHANCEMENT_MAX_INPUT_BYTES: u64 = 20 * 1024 * 1024;
const ENHANCEMENT_MIN_EDGE: u32 = 64;
const ENHANCEMENT_MAX_LONG_EDGE: u32 = 5000;
const ENHANCEMENT_MAX_ASPECT_RATIO: u32 = 2;

#[derive(Clone, Copy)]
enum EnhancementSourceError {
    Unsupported,
    TooLarge,
    Dimensions,
    AspectRatio,
}

pub(super) fn wire_image_enhancement_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        state.on_choose_enhance_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["jpg", "jpeg", "png", "webp"])
                .pick_file()
            else {
                return;
            };
            if let Err(error) = set_enhancement_source_from_path(&app, &path) {
                set_enhancement_source_error(&app, error);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_add_enhance_source_from_drag(move |mime_type, data| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            add_enhancement_from_drag_data(&app, mime_type.as_str(), data.as_str())
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_start_enhance(move |quality| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_enhance_source_path().trim().is_empty() {
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Upload an image first"
                    } else {
                        "请先上传图片"
                    }
                    .into(),
                );
                return;
            }
            let Some(target_quality) = normalized_enhancement_quality(quality.as_str()) else {
                state.set_enhance_message("请选择 2K 或 4K 清晰度".into());
                return;
            };
            start_image_enhancement(&app, context.clone(), target_quality);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reveal_enhance_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_enhance_result_path().to_string());
            if !path.is_file() {
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "No enhanced image is available yet"
                    } else {
                        "暂无可查看的清晰处理结果"
                    }
                    .into(),
                );
                return;
            }
            match reveal_path_in_file_manager(&path) {
                Ok(_) => state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Opened the image folder"
                    } else {
                        "已打开图片所在文件夹"
                    }
                    .into(),
                ),
                Err(error) => state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        format!("Failed to open the image folder: {error}")
                    } else {
                        format!("打开图片所在文件夹失败：{error}")
                    }
                    .into(),
                ),
            }
        });
    }
}

fn enhancement_source_dimensions(image: &Image) -> Option<(u32, u32)> {
    let buffer = image.to_rgba8()?;
    Some((buffer.width(), buffer.height()))
}

fn normalized_enhancement_quality(value: &str) -> Option<&'static str> {
    match value {
        "2K" => Some("2K"),
        "4K" => Some("4K"),
        _ => None,
    }
}

fn validate_enhancement_source(
    path: &Path,
    image: &Image,
) -> std::result::Result<(), EnhancementSourceError> {
    let size = fs::metadata(path)
        .map_err(|_| EnhancementSourceError::Unsupported)?
        .len();
    if size > ENHANCEMENT_MAX_INPUT_BYTES {
        return Err(EnhancementSourceError::TooLarge);
    }
    let Some((width, height)) = enhancement_source_dimensions(image) else {
        return Err(EnhancementSourceError::Unsupported);
    };
    let long_edge = width.max(height);
    let short_edge = width.min(height);
    if short_edge < ENHANCEMENT_MIN_EDGE || long_edge > ENHANCEMENT_MAX_LONG_EDGE {
        return Err(EnhancementSourceError::Dimensions);
    }
    if long_edge > short_edge.saturating_mul(ENHANCEMENT_MAX_ASPECT_RATIO) {
        return Err(EnhancementSourceError::AspectRatio);
    }
    Ok(())
}

fn set_enhancement_source_from_path(
    app: &AppWindow,
    path: &Path,
) -> std::result::Result<(), EnhancementSourceError> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !canonical.is_file() {
        return Err(EnhancementSourceError::Unsupported);
    }
    let image = load_image(&canonical).map_err(|_| EnhancementSourceError::Unsupported)?;
    validate_enhancement_source(&canonical, &image)?;
    let name = canonical
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string();
    let state = app.global::<AppState>();
    state.set_enhance_source_path(canonical.display().to_string().into());
    state.set_enhance_source_name(name.into());
    state.set_enhance_source_image(image);
    state.set_enhance_result_path("".into());
    state.set_enhance_result_name("".into());
    state.set_enhance_result_image(Image::default());
    state.set_enhance_processing(false);
    state.set_enhance_progress(0);
    if normalized_enhancement_quality(state.get_enhance_quality().as_str()).is_none() {
        state.set_enhance_quality("2K".into());
    }
    state.set_enhance_estimated_credits("20".into());
    state.set_enhance_message("".into());
    Ok(())
}

fn set_enhancement_source_error(app: &AppWindow, error: EnhancementSourceError) {
    let state = app.global::<AppState>();
    let english = state.get_language().as_str() == "en";
    let message = match error {
        EnhancementSourceError::Unsupported => {
            if english {
                "Choose a supported JPG, PNG or WebP image"
            } else {
                "请选择受支持的 JPG、PNG 或 WebP 图片"
            }
        }
        EnhancementSourceError::TooLarge => {
            if english {
                "The image must not exceed 20 MB"
            } else {
                "图片大小不能超过 20 MB"
            }
        }
        EnhancementSourceError::Dimensions => {
            if english {
                "The image must be at least 64px and its longest edge must not exceed 5000px"
            } else {
                "图片不得小于 64 像素，且最长边不能超过 5000 像素"
            }
        }
        EnhancementSourceError::AspectRatio => {
            if english {
                "The image aspect ratio must not exceed 2:1"
            } else {
                "图片宽高比不能超过 2:1"
            }
        }
    };
    state.set_enhance_message(message.into());
}

pub(super) fn add_enhancement_from_drag_data(app: &AppWindow, mime_type: &str, data: &str) -> bool {
    let state = app.global::<AppState>();
    if state.get_enhance_processing() {
        state.set_enhance_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while processing"
            } else {
                "处理中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    if let Some(url) = external_image_url(data) {
        start_external_enhancement_import(app, url);
        return true;
    }
    if mime_type != URI_LIST_MIME
        && mime_type != TEXT_PLAIN_MIME
        && mime_type != IMAGE_DRAG_MIME
        && mime_type != "text/html"
    {
        return false;
    }
    let paths = drag_data_to_paths(data);
    if paths.is_empty() {
        return false;
    }
    add_enhancement_paths(app, paths)
}

pub(super) fn add_enhancement_paths(app: &AppWindow, paths: Vec<PathBuf>) -> bool {
    let state = app.global::<AppState>();
    if state.get_enhance_processing() {
        state.set_enhance_message(
            if state.get_language().as_str() == "en" {
                "The image cannot be replaced while processing"
            } else {
                "处理中暂时不能更换图片"
            }
            .into(),
        );
        return true;
    }
    let mut last_error = EnhancementSourceError::Unsupported;
    for path in paths {
        match set_enhancement_source_from_path(app, &path) {
            Ok(()) => return true,
            Err(error) => last_error = error,
        }
    }
    set_enhancement_source_error(app, last_error);
    true
}

fn start_external_enhancement_import(app: &AppWindow, url: String) {
    let state = app.global::<AppState>();
    state.set_enhance_message(
        if state.get_language().as_str() == "en" {
            "Importing the dropped image..."
        } else {
            "正在导入拖入的图片..."
        }
        .into(),
    );
    let (sender, receiver) = mpsc::channel::<std::result::Result<PathBuf, String>>();
    std::thread::spawn(move || {
        let _ = sender.send(reference_callbacks::download_external_reference(&url));
    });
    poll_external_enhancement_import(app.as_weak(), Rc::new(RefCell::new(Some(receiver))));
}

fn poll_external_enhancement_import(
    app_weak: Weak<AppWindow>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<PathBuf, String>>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("图片导入任务已中断，请重试".to_string()))
                }
            }
        };
        let Some(result) = result else {
            poll_external_enhancement_import(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        match result {
            Ok(path) => {
                add_enhancement_paths(&app, vec![path]);
            }
            Err(error) => app.global::<AppState>().set_enhance_message(error.into()),
        }
    });
}

fn start_image_enhancement(app: &AppWindow, context: AppContext, target_quality: &str) {
    let state = app.global::<AppState>();
    if state.get_session_state().as_str() != "online" {
        state.set_auth_open(true);
        state.set_enhance_message(
            if state.get_language().as_str() == "en" {
                "Sign in and connect to the service before enhancing an image"
            } else {
                "请先登录并连接服务后再处理图片"
            }
            .into(),
        );
        return;
    }
    if context.backend.is_none() || state.get_enhance_processing() {
        return;
    }

    let source = PathBuf::from(state.get_enhance_source_path().to_string());
    let persisted_source = match persist_reference_source(&source) {
        Ok(path) => path,
        Err(_) => {
            state.set_enhance_message(
                if state.get_language().as_str() == "en" {
                    "The selected image could not be prepared"
                } else {
                    "无法处理所选图片，请更换图片后重试"
                }
                .into(),
            );
            return;
        }
    };
    let preview = match load_image(&persisted_source) {
        Ok(image) => image,
        Err(_) => {
            set_enhancement_source_error(app, EnhancementSourceError::Unsupported);
            return;
        }
    };
    if let Err(error) = validate_enhancement_source(&persisted_source, &preview) {
        set_enhancement_source_error(app, error);
        return;
    }
    state.set_enhance_source_path(persisted_source.display().to_string().into());
    state.set_enhance_source_image(preview);

    let client_request_id = Uuid::new_v4().simple().to_string();
    let record = PendingGenerationRecord {
        schema_version: 1,
        created_at_epoch_ms: Local::now().timestamp_millis(),
        client_request_id,
        local_task_id: Uuid::new_v4().to_string(),
        server_task_id: String::new(),
        raw_prompt: "图片清晰增强".to_string(),
        generation_prompt: String::new(),
        task_type: "image_enhancement".to_string(),
        category: "other".to_string(),
        mode: "game".to_string(),
        ratio: String::new(),
        quality: target_quality.to_string(),
        model_code: "aliyun_super_resolution".to_string(),
        conversation_id: String::new(),
        count: 1,
        target_width: 0,
        target_height: 0,
        create_conversation: false,
        reference_paths: vec![persisted_source.display().to_string()],
        uploaded_file_ids: vec![],
        deliveries: vec![],
        terminal: false,
        expected_success_count: 0,
    };
    if upsert_pending_generation(record.clone()).is_err() {
        state.set_enhance_message(
            if state.get_language().as_str() == "en" {
                "The task could not be saved locally"
            } else {
                "任务准备失败，请重试"
            }
            .into(),
        );
        return;
    }
    launch_image_enhancement(app, context, record, false);
}

pub(super) fn resume_pending_image_enhancement(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
) {
    if app.global::<AppState>().get_enhance_processing() {
        return;
    }
    if let Some(source_path) = record.reference_paths.first() {
        let path = PathBuf::from(source_path);
        if path.is_file() {
            let state = app.global::<AppState>();
            state.set_enhance_source_path(source_path.clone().into());
            state.set_enhance_source_name(
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .into(),
            );
            if let Ok(image) = load_image(&path) {
                state.set_enhance_source_image(image);
            }
        }
    }
    launch_image_enhancement(app, context, record, true);
}

fn launch_image_enhancement(
    app: &AppWindow,
    context: AppContext,
    record: PendingGenerationRecord,
    recovering: bool,
) {
    let Some(backend) = context.backend.clone() else {
        return;
    };
    let state = app.global::<AppState>();
    state.set_enhance_processing(true);
    state.set_enhance_progress(if recovering { 5 } else { 1 });
    let target_quality = normalized_enhancement_quality(&record.quality).unwrap_or("2K");
    state.set_enhance_quality(target_quality.into());
    state.set_enhance_estimated_credits("20".into());
    state.set_enhance_result_path("".into());
    state.set_enhance_result_name("".into());
    state.set_enhance_result_image(Image::default());
    state.set_enhance_message(
        if state.get_language().as_str() == "en" {
            if recovering {
                "Recovering the image-enhancement task..."
            } else {
                "Uploading the image..."
            }
        } else if recovering {
            "正在恢复未完成的清晰增强任务..."
        } else {
            "正在上传图片..."
        }
        .into(),
    );

    let source_path = record.reference_paths.first().cloned().unwrap_or_default();
    let (sender, receiver) = mpsc::channel::<ImageEnhancementOutcome>();
    std::thread::spawn(move || run_image_enhancement_worker(backend, record, sender));
    poll_image_enhancement_outcomes(
        app.as_weak(),
        context,
        Rc::new(RefCell::new(Some(receiver))),
        source_path,
    );
}

fn run_image_enhancement_worker(
    backend: Arc<BackendRuntime>,
    mut record: PendingGenerationRecord,
    sender: mpsc::Sender<ImageEnhancementOutcome>,
) {
    let api = GenerationApi::new(backend.api.clone());
    if record.terminal {
        if let Some(saved) = record.deliveries.iter().find(|delivery| {
            !delivery.local_path.is_empty() && Path::new(&delivery.local_path).is_file()
        }) {
            let delivery = (!saved.acknowledged).then(|| DeliveryConfirmation {
                client_request_id: record.client_request_id.clone(),
                item_index: saved.item_index,
                task_id: record.server_task_id.clone(),
                file_id: saved.file_id.clone(),
                sha256: saved.sha256.clone(),
                size_bytes: saved.size_bytes,
            });
            let _ = sender.send(ImageEnhancementOutcome::Recovered {
                local_path: saved.local_path.clone(),
                delivery,
            });
            return;
        }
    }

    let mut uploaded = record.uploaded_file_ids.clone();
    if uploaded.is_empty() && record.server_task_id.is_empty() {
        let Some(path) = record.reference_paths.first() else {
            let _ = remove_pending_generation(&record.client_request_id);
            let _ = sender.send(ImageEnhancementOutcome::Failure {
                reason: "找不到待处理的原图，请重新上传".to_string(),
            });
            return;
        };
        match api.upload_reference(Path::new(path)) {
            Ok(file_id) => {
                uploaded.push(file_id);
                let snapshot = uploaded.clone();
                let _ = update_pending_generation(&record.client_request_id, |item| {
                    item.uploaded_file_ids = snapshot;
                });
            }
            Err(error) => {
                if !error.should_preserve_generation_recovery() {
                    let _ = remove_pending_generation(&record.client_request_id);
                }
                let _ = sender.send(ImageEnhancementOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    }

    let mut detail = if record.server_task_id.is_empty() {
        let request = CreateImageEnhancement {
            client_request_id: record.client_request_id.clone(),
            reference_file_id: uploaded[0].clone(),
            target_quality: normalized_enhancement_quality(&record.quality)
                .unwrap_or("2K")
                .to_string(),
        };
        match api.create_image_enhancement(&request) {
            Ok(detail) => detail,
            Err(error) => {
                if error.is_insufficient_credits() {
                    for file_id in &uploaded {
                        api.delete_reference(file_id);
                    }
                    let _ = remove_pending_generation(&record.client_request_id);
                    let _ = sender.send(ImageEnhancementOutcome::CreditInsufficient {
                        message: "本次图片清晰增强需要 20 积分，请先充值".to_string(),
                    });
                    return;
                }
                if !error.should_preserve_generation_recovery() {
                    for file_id in &uploaded {
                        api.delete_reference(file_id);
                    }
                    let _ = remove_pending_generation(&record.client_request_id);
                }
                let _ = sender.send(ImageEnhancementOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        }
    } else {
        match api.task(&record.server_task_id) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ImageEnhancementOutcome::Failure {
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
    let _ = update_pending_generation(&record.client_request_id, |item| {
        item.server_task_id = server_id_snapshot;
        item.uploaded_file_ids = uploaded_snapshot;
    });
    let _ = sender.send(ImageEnhancementOutcome::Accepted {
        task_id: server_task_id.clone(),
    });

    loop {
        let _ = sender.send(ImageEnhancementOutcome::Progress {
            percent: detail.progress_percent,
        });
        if let Some(item) = detail.items.iter().find(|item| item.status == "succeeded") {
            if let Some(file) = item.file.as_ref() {
                match api.download_verified(file) {
                    Ok(bytes) => {
                        let _ = update_pending_generation(&record.client_request_id, |pending| {
                            pending.terminal = true;
                            pending.expected_success_count = 1;
                        });
                        let _ = sender.send(ImageEnhancementOutcome::Success {
                            bytes,
                            delivery: DeliveryConfirmation {
                                client_request_id: record.client_request_id.clone(),
                                item_index: item.index,
                                task_id: server_task_id,
                                file_id: file.id.clone(),
                                sha256: file.sha256.clone(),
                                size_bytes: file.size_bytes.parse().unwrap_or(0),
                            },
                        });
                        return;
                    }
                    Err(error) if detail.terminal() => {
                        let _ = sender.send(ImageEnhancementOutcome::Failure {
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
                .unwrap_or_else(|| "服务端未能完成图片清晰增强".to_string());
            let _ = update_pending_generation(&record.client_request_id, |pending| {
                pending.terminal = true;
                pending.expected_success_count = 0;
            });
            let _ = sender.send(ImageEnhancementOutcome::Failure { reason });
            return;
        }
        std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
        detail = match api.task(&record.server_task_id) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = sender.send(ImageEnhancementOutcome::Failure {
                    reason: error.generation_message(),
                });
                return;
            }
        };
    }
}

fn poll_image_enhancement_outcomes(
    app_weak: Weak<AppWindow>,
    context: AppContext,
    receiver: Rc<RefCell<Option<mpsc::Receiver<ImageEnhancementOutcome>>>>,
    source_path: String,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
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
                    Some(ImageEnhancementOutcome::Failure {
                        reason: "图片清晰增强任务已中断，请重试".to_string(),
                    })
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_image_enhancement_outcomes(app_weak, context, receiver, source_path);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        let mut keep_polling = true;
        match outcome {
            ImageEnhancementOutcome::Accepted { task_id } => {
                state.set_enhance_progress(state.get_enhance_progress().max(8));
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        format!("Task {task_id} is queued")
                    } else {
                        "任务已提交，正在排队处理...".to_string()
                    }
                    .into(),
                );
            }
            ImageEnhancementOutcome::Progress { percent } => {
                state.set_enhance_progress(percent.clamp(1, 99));
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Enhancing image details..."
                    } else {
                        "正在进行智能超分与细节增强..."
                    }
                    .into(),
                );
            }
            ImageEnhancementOutcome::Success { bytes, delivery } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                match save_image_enhancement_asset(&app, &context.store, &source_path, &bytes) {
                    Ok((result_path, result_image)) => {
                        let result_name = Path::new(&result_path)
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("清晰增强结果")
                            .to_string();
                        state.set_enhance_result_path(result_path.clone().into());
                        state.set_enhance_result_name(result_name.into());
                        state.set_enhance_result_image(result_image);
                        state.set_enhance_progress(100);
                        state.set_enhance_processing(false);
                        state.set_enhance_message(
                            if state.get_language().as_str() == "en" {
                                "Enhanced image saved to My Assets / Other"
                            } else {
                                "处理完成，已保存到“我的资产 / 其他”"
                            }
                            .into(),
                        );
                        let _ = pending_delivery_saved(
                            &delivery.client_request_id,
                            &delivery,
                            &result_path,
                        );
                        if let Some(backend) = context.backend.clone() {
                            acknowledge_delivery_after_local_save(backend, delivery);
                        }
                    }
                    Err(error) => {
                        state.set_enhance_processing(false);
                        state.set_enhance_message(
                            format!("处理结果保存失败：{}", zh_error(&error.to_string())).into(),
                        );
                    }
                }
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            ImageEnhancementOutcome::Recovered {
                local_path,
                delivery,
            } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                let path = PathBuf::from(&local_path);
                if let Ok(image) = load_image(&path) {
                    state.set_enhance_result_path(local_path.clone().into());
                    state.set_enhance_result_name(
                        path.file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("清晰增强结果")
                            .into(),
                    );
                    state.set_enhance_result_image(image);
                    state.set_enhance_progress(100);
                    state.set_enhance_message("已恢复上次完成的图片清晰增强结果".into());
                }
                state.set_enhance_processing(false);
                if let (Some(backend), Some(delivery)) = (context.backend.clone(), delivery) {
                    acknowledge_delivery_after_local_save(backend, delivery);
                }
            }
            ImageEnhancementOutcome::CreditInsufficient { message } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                state.set_enhance_processing(false);
                state.set_enhance_progress(0);
                state.set_enhance_message(message.clone().into());
                state.set_credit_insufficient_message(message.into());
                state.set_credit_insufficient_open(true);
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
            ImageEnhancementOutcome::Failure { reason } => {
                keep_polling = false;
                receiver.borrow_mut().take();
                state.set_enhance_processing(false);
                state.set_enhance_progress(0);
                state.set_enhance_message(reason.into());
                if context.backend.is_some() {
                    refresh_backend_snapshot(&app, context.clone());
                }
            }
        }
        if keep_polling {
            poll_image_enhancement_outcomes(app_weak, context, receiver, source_path);
        }
    });
}

fn enhancement_quality(width: i32, height: i32) -> String {
    match width.max(height) {
        value if value <= 1024 => "1K".to_string(),
        value if value <= 2048 => "2K".to_string(),
        _ => "4K".to_string(),
    }
}

fn save_image_enhancement_asset(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    source_path: &str,
    bytes: &[u8],
) -> Result<(String, Image)> {
    let (bytes, image, width, height) = generated_image_from_bytes(bytes)?;
    let source = Path::new(source_path);
    let source_title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("图片");
    let title = format!("{} 清晰增强", short_text(source_title, 18));
    let result_path = save_generated_bytes(app, &bytes, &title)?;
    let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: String::new(),
        title: title.clone(),
        category: "other".to_string(),
        kind: "game".to_string(),
        time: now.clone(),
        prompt: "图片清晰增强".to_string(),
        ratio: ratio_from_actual_dimensions(width, height),
        quality: enhancement_quality(width, height),
        model: "图片清晰".to_string(),
        origin: "image_enhancement".to_string(),
        width,
        height,
        image: image.clone(),
        source_path: result_path.clone(),
        reference_paths: (!source_path.is_empty())
            .then(|| source_path.to_string())
            .into_iter()
            .collect(),
        cutout_done: false,
        remove_black_done: false,
        upscale_done: true,
        is_new: false,
    };
    let mut store = store.borrow_mut();
    store.assets.insert(0, item);
    store.notifications.insert(
        0,
        NotificationData {
            id: Uuid::new_v4().to_string(),
            title: format!("图片清晰增强完成：{title}"),
            model: "图片清晰".to_string(),
            time: now,
            reason: String::new(),
            success: true,
            read: false,
        },
    );
    save_local_store(app, &store);
    push_all(app, &store);
    Ok((result_path, image))
}
