use super::*;

pub(super) fn wire_custom_prompt_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();
    let analysis_backend = context.backend.clone();

    {
        let app_weak = app.as_weak();
        state.on_begin_new_custom_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            reset_custom_prompt_editor(&app);
            app.global::<AppState>()
                .set_custom_prompt_editor_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_begin_edit_custom_prompt(move |prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let prompt = prompt.to_string();
            let profile = store
                .borrow()
                .custom_prompt_profiles
                .get(&prompt)
                .cloned()
                .unwrap_or_default();
            let state = app.global::<AppState>();
            let fallback_name = prompt
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(48)
                .collect::<String>();
            state.set_custom_prompt_name(
                if profile.name.trim().is_empty() {
                    fallback_name
                } else {
                    profile.name
                }
                .into(),
            );
            state.set_custom_prompt_input(prompt.clone().into());
            state.set_custom_prompt_editing_original(prompt.into());
            state.set_custom_prompt_category(
                normalized_custom_prompt_category(&profile.category).into(),
            );
            state
                .set_custom_prompt_format(normalized_custom_prompt_format(&profile.format).into());
            state.set_custom_prompt_negative(profile.negative_prompt.into());
            state.set_custom_prompt_reference_path(profile.reference_path.clone().into());
            state.set_custom_prompt_reference_image(
                if profile.reference_path.is_empty() {
                    Image::default()
                } else {
                    load_image(Path::new(&profile.reference_path)).unwrap_or_default()
                },
            );
            state.set_custom_prompt_message("".into());
            state.set_custom_prompt_editor_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_choose_custom_prompt_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
            else {
                return;
            };
            let state = app.global::<AppState>();
            match load_image(&path) {
                Ok(image) => {
                    state.set_custom_prompt_reference_path(path.display().to_string().into());
                    state.set_custom_prompt_reference_image(image);
                    state.set_custom_prompt_message("".into());
                }
                Err(_) => state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "The selected file is not a supported image"
                    } else {
                        "所选文件不是受支持的图片"
                    }
                    .into(),
                ),
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_clear_custom_prompt_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_custom_prompt_reference_path("".into());
            state.set_custom_prompt_reference_image(Image::default());
        });
    }

    {
        let app_weak = app.as_weak();
        let backend = analysis_backend.clone();
        state.on_analyze_custom_prompt_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_custom_prompt_analyzing() {
                return;
            }
            if !require_online_operation(&app, "分析参考图风格") {
                return;
            }
            if state.get_custom_prompt_reference_path().is_empty() {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "Upload a style reference image first"
                    } else {
                        "请先上传风格参考图"
                    }
                    .into(),
                );
                return;
            }
            let Some(backend) = backend.clone() else {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "The model service is unavailable"
                    } else {
                        "模型服务暂不可用"
                    }
                    .into(),
                );
                return;
            };
            let reference_path =
                PathBuf::from(state.get_custom_prompt_reference_path().to_string());
            if !reference_path.is_file() {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "The reference image file no longer exists"
                    } else {
                        "参考图文件已不存在"
                    }
                    .into(),
                );
                return;
            }
            let model_code = state.get_reasoning_model().to_string();
            if model_code.trim().is_empty() {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "Select a reasoning model first"
                    } else {
                        "请先选择推理模型"
                    }
                    .into(),
                );
                return;
            }
            let english = state.get_language().as_str() == "en";
            state.set_custom_prompt_analyzing(true);
            state.set_custom_prompt_message(
                if english {
                    "Analyzing the reference image..."
                } else {
                    "正在分析参考图风格..."
                }
                .into(),
            );
            start_custom_prompt_reference_analysis(
                &app,
                backend,
                model_code,
                reference_path,
                english,
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_save_custom_prompt(move |original, prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let name = state.get_custom_prompt_name().trim().to_string();
            if name.is_empty() {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "Enter a prompt name"
                    } else {
                        "请输入提示词名称"
                    }
                    .into(),
                );
                return;
            }
            let timestamp = Local::now().format("%Y-%m-%d %H:%M").to_string();
            let format = normalized_custom_prompt_format(
                state.get_custom_prompt_format().as_str(),
            );
            let profile = CustomPromptProfile {
                name,
                category: normalized_custom_prompt_category(
                    state.get_custom_prompt_category().as_str(),
                ),
                format: format.clone(),
                negative_prompt: if format == "json" {
                    state.get_custom_prompt_negative().trim().to_string()
                } else {
                    String::new()
                },
                reference_path: state.get_custom_prompt_reference_path().to_string(),
            };
            let result = {
                let mut store = store.borrow_mut();
                let result =
                    save_custom_prompt_to_store(&mut store, &original, &prompt, &timestamp);
                if result == SaveCustomPromptResult::Saved {
                    save_custom_prompt_profile(&mut store, &original, &prompt, profile);
                }
                result
            };
            match result {
                SaveCustomPromptResult::Saved => {
                    reset_custom_prompt_editor(&app);
                    state.set_custom_prompt_editor_open(false);
                    push_custom_prompts(&app, &store.borrow());
                    save_local_store(&app, &store.borrow());
                }
                SaveCustomPromptResult::Empty => {
                    state.set_custom_prompt_message(
                        if state.get_language().as_str() == "en" {
                            "Enter a prompt first"
                        } else {
                            "请输入提示词"
                        }
                        .into(),
                    );
                }
                SaveCustomPromptResult::Duplicate => {
                    state.set_custom_prompt_message(
                        if state.get_language().as_str() == "en" {
                            "This prompt already exists"
                        } else {
                            "该提示词已存在"
                        }
                        .into(),
                    );
                }
                SaveCustomPromptResult::Missing => {
                    state.set_custom_prompt_message(
                        if state.get_language().as_str() == "en" {
                            "This prompt no longer exists"
                        } else {
                            "该提示词已不存在，请关闭后重试"
                        }
                        .into(),
                    );
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_remove_custom_prompt(move |prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if remove_custom_prompt_from_store(&mut store.borrow_mut(), &prompt) {
                let state = app.global::<AppState>();
                state.set_custom_prompt_message("".into());
                push_custom_prompts(&app, &store.borrow());
                save_local_store(&app, &store.borrow());
            }
        });
    }
}

fn start_custom_prompt_reference_analysis(
    app: &AppWindow,
    backend: Arc<BackendRuntime>,
    model_code: String,
    reference_path: PathBuf,
    english: bool,
) {
    let (sender, receiver) = mpsc::channel::<std::result::Result<String, String>>();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        let result = (|| {
            let file_id = api
                .upload_reference(&reference_path)
                .map_err(|error| error.user_message())?;
            let request = CreateGenerationTask {
                client_request_id: Uuid::new_v4().simple().to_string(),
                task_type: "prompt_optimize".to_string(),
                model_code,
                prompt: if english {
                    "Analyze the uploaded image's visual style. Return only one concise, reusable \
                     English image-generation style description covering composition, palette, \
                     lighting, rendering medium, texture, detail, and atmosphere. Do not describe \
                     file metadata and do not add headings."
                        .to_string()
                } else {
                    "分析上传参考图的视觉风格。只输出一段可直接复用的中文生图风格描述，覆盖构图、\
                     配色、光影、绘制媒介、纹理、细节与氛围；不要描述文件元数据，不要添加标题。"
                        .to_string()
                },
                quality: None,
                count: None,
                aspect_ratio: None,
                reference_file_ids: Some(vec![file_id.clone()]),
                target_language: None,
            };
            let task_result = (|| {
                let mut detail = api
                    .create_task(&request)
                    .map_err(|error| error.user_message())?;
                loop {
                    if detail.terminal() {
                        if matches!(detail.status.as_str(), "completed" | "partially_completed") {
                            return detail
                                .result_prompt
                                .filter(|value| !value.trim().is_empty())
                                .ok_or_else(|| {
                                    if english {
                                        "The model did not return a style description".to_string()
                                    } else {
                                        "模型未返回风格描述".to_string()
                                    }
                                });
                        }
                        return Err(detail
                            .failure
                            .map(|failure| failure.message)
                            .unwrap_or_else(|| {
                                if english {
                                    "Image style analysis failed".to_string()
                                } else {
                                    "图片风格分析失败".to_string()
                                }
                            }));
                    }
                    std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
                    detail = api.task(&detail.id).map_err(|error| error.user_message())?;
                }
            })();
            api.delete_reference(&file_id);
            task_result
        })();
        let _ = sender.send(result);
    });
    poll_custom_prompt_reference_analysis(
        app.as_weak(),
        Rc::new(RefCell::new(Some(receiver))),
    );
}

fn poll_custom_prompt_reference_analysis(
    app_weak: Weak<AppWindow>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<String, String>>>>>,
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
                    Some(Err("图片风格分析任务已中断，请重试".to_string()))
                }
            }
        };
        let Some(result) = result else {
            poll_custom_prompt_reference_analysis(app_weak, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_custom_prompt_analyzing(false);
        match result {
            Ok(analysis) => {
                let current = state.get_custom_prompt_input().trim().to_string();
                state.set_custom_prompt_input(
                    if current.is_empty() {
                        analysis
                    } else {
                        format!("{current}\n\n{analysis}")
                    }
                    .into(),
                );
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "Image style analysis completed"
                    } else {
                        "图片风格分析完成"
                    }
                    .into(),
                );
            }
            Err(error) => state.set_custom_prompt_message(error.into()),
        }
    });
}

fn reset_custom_prompt_editor(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_custom_prompt_name("".into());
    state.set_custom_prompt_input("".into());
    state.set_custom_prompt_category("default".into());
    state.set_custom_prompt_format("json".into());
    state.set_custom_prompt_negative("".into());
    state.set_custom_prompt_reference_path("".into());
    state.set_custom_prompt_reference_image(Image::default());
    state.set_custom_prompt_message("".into());
    state.set_custom_prompt_analyzing(false);
    state.set_custom_prompt_editing_original("".into());
}

pub(super) fn normalized_custom_prompt_category(value: &str) -> String {
    match value {
        "character" | "scene" | "ui" | "effect" => value.to_string(),
        _ => "default".to_string(),
    }
}

pub(super) fn normalized_custom_prompt_format(value: &str) -> String {
    if value == "txt" {
        "txt".to_string()
    } else {
        "json".to_string()
    }
}

#[allow(dead_code)]
fn legacy_reference_style(
    rgba: &[u8],
    width: u32,
    height: u32,
    english: bool,
) -> Option<String> {
    let pixel_count = rgba.len() / 4;
    if pixel_count == 0 || width == 0 || height == 0 {
        return None;
    }

    let sample_step = (pixel_count / 50_000).max(1);
    let mut samples = 0_f64;
    let mut red = 0_f64;
    let mut green = 0_f64;
    let mut blue = 0_f64;
    let mut luminance = 0_f64;
    let mut luminance_squared = 0_f64;
    let mut saturation = 0_f64;

    for pixel in rgba.chunks_exact(4).step_by(sample_step) {
        if pixel[3] == 0 {
            continue;
        }
        let r = pixel[0] as f64 / 255.0;
        let g = pixel[1] as f64 / 255.0;
        let b = pixel[2] as f64 / 255.0;
        let maximum = r.max(g).max(b);
        let minimum = r.min(g).min(b);
        let value = 0.2126 * r + 0.7152 * g + 0.0722 * b;

        samples += 1.0;
        red += r;
        green += g;
        blue += b;
        luminance += value;
        luminance_squared += value * value;
        saturation += if maximum <= f64::EPSILON {
            0.0
        } else {
            (maximum - minimum) / maximum
        };
    }

    if samples == 0.0 {
        return None;
    }

    let average_red = red / samples;
    let average_green = green / samples;
    let average_blue = blue / samples;
    let average_luminance = luminance / samples;
    let average_saturation = saturation / samples;
    let variance =
        (luminance_squared / samples - average_luminance * average_luminance).max(0.0);
    let contrast = variance.sqrt();
    let warm_balance =
        average_red - average_blue + (average_green - average_blue) * 0.12;

    let orientation = if width > height.saturating_mul(6) / 5 {
        if english { "landscape" } else { "横向" }
    } else if height > width.saturating_mul(6) / 5 {
        if english { "portrait" } else { "竖向" }
    } else if english {
        "square"
    } else {
        "方形"
    };
    let brightness = if average_luminance > 0.68 {
        if english { "bright and airy" } else { "明亮通透" }
    } else if average_luminance < 0.34 {
        if english { "deep low-key lighting" } else { "低调暗部" }
    } else if english {
        "balanced lighting"
    } else {
        "明暗均衡"
    };
    let temperature = if warm_balance > 0.07 {
        if english { "warm palette" } else { "暖色调" }
    } else if warm_balance < -0.06 {
        if english { "cool palette" } else { "冷色调" }
    } else if english {
        "neutral palette"
    } else {
        "中性色调"
    };
    let chroma = if average_saturation > 0.55 {
        if english { "vivid saturated color" } else { "色彩高饱和鲜明" }
    } else if average_saturation < 0.20 {
        if english { "soft restrained color" } else { "色彩低饱和柔和" }
    } else if english {
        "natural color saturation"
    } else {
        "色彩饱和度自然"
    };
    let tonal_contrast = if contrast > 0.24 {
        if english { "strong tonal contrast" } else { "强对比光影" }
    } else if contrast < 0.11 {
        if english { "soft low contrast" } else { "柔和低对比光影" }
    } else if english {
        "balanced tonal contrast"
    } else {
        "均衡对比光影"
    };
    let detail = if width.max(height) >= 2_000 {
        if english { "fine detailed texture" } else { "细节与纹理丰富" }
    } else if english {
        "clean controlled detail"
    } else {
        "细节简洁克制"
    };

    Some(if english {
        format!(
            "Reference style: {orientation} composition, {brightness}, {temperature}, \
             {chroma}, {tonal_contrast}, {detail}."
        )
    } else {
        format!(
            "参考图风格：{orientation}构图，{brightness}，{temperature}，{chroma}，\
             {tonal_contrast}，{detail}。"
        )
    })
}
