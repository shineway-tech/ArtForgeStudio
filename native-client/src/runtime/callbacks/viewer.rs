use super::*;

pub(super) fn wire_viewer_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();
    let image_editor_points = Rc::new(VecModel::<BrushPoint>::default());
    let image_editor_last_point = Rc::new(RefCell::new(None::<(f32, f32, f32)>));
    state.set_image_editor_points(image_editor_points.clone().into());

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_viewer(move |id, source| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open_viewer(&app, &store.borrow(), id.as_str(), source.as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_viewer(move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_viewer_message("".into());
                state.set_viewer_open(false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_prev(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            move_viewer(&app, &store.borrow(), -1);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_next(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            move_viewer(&app, &store.borrow(), 1);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_download_asset(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            download_asset(&app, &store, id.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_viewer_copy_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            copy_viewer_image(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_download_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            download_viewer_image(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_open_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open_viewer_image(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_start_viewer_file_drag(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let state = app.global::<AppState>();
            let id = state.get_viewer_id().to_string();
            let source = state.get_viewer_source().to_string();
            let path = viewer_item(&store.borrow(), &id, &source)
                .map(|item| PathBuf::from(item.source_path.trim()));
            let Some(path) = path else {
                return false;
            };
            let result = drag_preview::start_thumbnail_file_drag(path);
            reset_pointer_after_native_drag(&app);
            result
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_viewer_cutout_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_cutout_type("general".into());
            state.set_cutout_message("".into());
            state.set_cutout_progress(0);
            state.set_cutout_result_path("".into());
            state.set_cutout_result_name("".into());
            state.set_cutout_result_image(Image::default());
            state.set_cutout_estimated_credits("20".into());
            state.set_viewer_open(false);
            state.set_cutout_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_cutout(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_cutout_processing() {
                state.set_cutout_message(
                    if state.get_language().as_str() == "en" {
                        "Please wait for the cutout task to finish"
                    } else {
                        "抠图处理中，请等待任务完成"
                    }
                    .into(),
                );
                return;
            }
            state.set_cutout_open(false);
            state.set_cutout_message("".into());
            state.set_viewer_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_remove_black(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_viewer_image_processing(&app, store.clone(), ProcessImageMode::RemoveBlack);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_open_upscale_dialog(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_viewer_upscale_done() {
                state.set_viewer_message(
                    processing_done_message(
                        &app,
                        ProcessImageMode::Upscale {
                            scale: 2,
                            target_long_edge: 2048,
                        },
                    )
                    .into(),
                );
                return;
            }
            state.set_upscale_scale(2);
            state.set_upscale_quality("2K".into());
            state.set_upscale_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_close_upscale_dialog(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_viewer_processing() {
                state.set_upscale_open(false);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_start_upscale_image(move |scale, quality| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_backend_upscale(
                &app,
                context.clone(),
                scale.clamp(2, 4) as u32,
                quality.to_string(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let context = context.clone();
        state.on_viewer_regenerate(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let id = state.get_viewer_id().to_string();
            let source = state.get_viewer_source().to_string();
            let item = viewer_item(&store.borrow(), &id, &source).cloned();
            if let Some(item) = item {
                start_asset_regeneration(&app, context.clone(), item);
            } else {
                state.set_viewer_message("找不到原生成记录，无法再次生成".into());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_viewer_edit(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_prompt(state.get_viewer_prompt());
            state.set_quote_title(state.get_viewer_title());
            state.set_quote_prompt(state.get_viewer_prompt());
            state.set_quote_ratio(state.get_viewer_ratio());
            state.set_quote_quality(state.get_viewer_quality());
            state.set_viewer_open(false);
            navigate_to(&app, "generation");
        });
    }

    {
        let app_weak = app.as_weak();
        let points = image_editor_points.clone();
        let last_point = image_editor_last_point.clone();
        state.on_viewer_open_image_editor(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let viewer_image = state.get_viewer_image();
            let mut source_width = state.get_viewer_width();
            let mut source_height = state.get_viewer_height();
            if source_width <= 0 || source_height <= 0 {
                if let Some(buffer) = viewer_image.to_rgba8() {
                    source_width = buffer.width() as i32;
                    source_height = buffer.height() as i32;
                }
            }

            points.clear();
            *last_point.borrow_mut() = None;
            state.set_image_editor_image(viewer_image);
            state.set_image_editor_source_width(source_width.max(1));
            state.set_image_editor_source_height(source_height.max(1));
            state.set_image_editor_brush_size(28.0);
            state.set_image_editor_brush_shape("circle".into());
            state.set_image_editor_brush_color(slint::Color::from_rgb_u8(255, 77, 79));
            state.set_image_editor_prompt("".into());
            state.set_image_editor_status("".into());
            state.set_image_editor_generating(false);
            state.set_image_editor_return_page(state.get_page());
            state.set_viewer_open(false);
            navigate_to(&app, "image-editor");
        });
    }

    {
        let app_weak = app.as_weak();
        let last_point = image_editor_last_point.clone();
        state.on_close_image_editor(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let return_page = state.get_image_editor_return_page().to_string();
            *last_point.borrow_mut() = None;
            navigate_to(&app, &return_page);
            state.set_viewer_open(true);
        });
    }

    {
        let points = image_editor_points.clone();
        let last_point = image_editor_last_point.clone();
        state.on_begin_image_editor_stroke(move |x, y, size, aspect, shape, color| {
            append_brush_segment(&points, None, (x, y, size), aspect, &shape, color);
            *last_point.borrow_mut() = Some((x, y, size));
        });
    }

    {
        let points = image_editor_points.clone();
        let last_point = image_editor_last_point.clone();
        state.on_continue_image_editor_stroke(move |x, y, size, aspect, shape, color| {
            let previous = *last_point.borrow();
            append_brush_segment(&points, previous, (x, y, size), aspect, &shape, color);
            *last_point.borrow_mut() = Some((x, y, size));
        });
    }

    {
        let last_point = image_editor_last_point;
        state.on_end_image_editor_stroke(move || {
            *last_point.borrow_mut() = None;
        });
    }

    {
        let app_weak = app.as_weak();
        let points = image_editor_points.clone();
        state.on_submit_image_edit(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if points.row_count() == 0 {
                state.set_image_editor_status("请先用笔刷标记需要修改的区域".into());
                return;
            }
            if state.get_image_editor_prompt().trim().is_empty() {
                state.set_image_editor_status("请填写希望如何修改涂抹区域".into());
                return;
            }
            state.set_image_editor_status("局部重绘服务接口待接入".into());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_use_same(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let max_references = max_reference_images_for_category(&category);
            if references_for_category(&store.borrow().references, &category).len()
                >= max_references
            {
                state.set_viewer_message(reference_limit_message(max_references).into());
                return;
            }
            let source_path = match persist_slint_reference(&state.get_viewer_image()) {
                Ok(path) => path.display().to_string(),
                Err(_) => {
                    state.set_viewer_message("无法保存当前图片作为参考图".into());
                    return;
                }
            };
            let prompt = state.get_viewer_prompt().to_string();
            let title = short_text(&prompt, 10);
            let conversation_id = Uuid::new_v4().to_string();
            let mut conversations = state.get_conversations().iter().collect::<Vec<_>>();
            conversations.insert(
                0,
                ConversationItem {
                    id: SharedString::from(conversation_id.clone()),
                    title: SharedString::from(title),
                    image: Image::default(),
                    loading: false,
                },
            );
            state.set_conversations(ModelRc::new(VecModel::from(conversations)));
            state.set_current_conversation_id(conversation_id.into());
            {
                let mut store_mut = store.borrow_mut();
                references_for_category_mut(&mut store_mut.references, &category).push(
                    ReferenceData {
                        id: Uuid::new_v4().to_string(),
                        image: state.get_viewer_image(),
                        source_path,
                    },
                );
            }
            push_references(&app, &store.borrow());
            state.set_viewer_open(false);
            state.set_prompt(prompt.into());
            navigate_to(&app, "generation");
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_use_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let max_references = max_reference_images_for_category(&category);
            if references_for_category(&store.borrow().references, &category).len()
                >= max_references
            {
                state.set_viewer_message(reference_limit_message(max_references).into());
                return;
            }
            let source_path = match persist_slint_reference(&state.get_viewer_image()) {
                Ok(path) => path.display().to_string(),
                Err(_) => {
                    state.set_viewer_message("无法保存当前图片作为参考图".into());
                    return;
                }
            };
            {
                let mut store_mut = store.borrow_mut();
                references_for_category_mut(&mut store_mut.references, &category).push(
                    ReferenceData {
                        id: Uuid::new_v4().to_string(),
                        image: state.get_viewer_image(),
                        source_path,
                    },
                );
            }
            push_references(&app, &store.borrow());
            state.set_viewer_open(false);
            navigate_to(&app, "generation");
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_request_delete_asset(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_pending_delete_kind("asset".into());
                state.set_pending_delete_id(id);
                state.set_pending_delete_source(state.get_viewer_source());
                state.set_delete_confirm_open(true);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_request_delete_thumbnail(move |id, source| {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                state.set_pending_delete_kind("asset".into());
                state.set_pending_delete_id(id);
                state.set_pending_delete_source(source);
                state.set_delete_confirm_open(true);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_confirm_delete(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let id = state.get_pending_delete_id().to_string();
            let source = state.get_pending_delete_source().to_string();
            {
                let mut store_mut = store.borrow_mut();
                match source.as_str() {
                    "asset" => store_mut.assets.retain(|a| a.id != id),
                    "inspiration" => store_mut.inspiration.retain(|a| a.id != id),
                    "reference" => {
                        let category = resolve_category(&state.get_asset_type().to_string(), "");
                        references_for_category_mut(&mut store_mut.references, &category)
                            .retain(|item| item.id != id);
                    }
                    _ => store_mut.generations.retain(|a| a.id != id),
                }
                save_local_store(&app, &store_mut);
            }
            state.set_pending_delete_id("".into());
            state.set_pending_delete_source("".into());
            state.set_delete_confirm_open(false);
            state.set_viewer_open(false);
            push_all(&app, &store.borrow());
        });
    }
}

fn append_brush_segment(
    model: &VecModel<BrushPoint>,
    from: Option<(f32, f32, f32)>,
    to: (f32, f32, f32),
    aspect: f32,
    shape: &str,
    color: slint::Color,
) {
    const MAX_BRUSH_POINTS: usize = 25_000;
    if model.row_count() >= MAX_BRUSH_POINTS {
        return;
    }

    for point in interpolated_brush_points(from, to, aspect, shape, color)
        .into_iter()
        .take(MAX_BRUSH_POINTS - model.row_count())
    {
        model.push(point);
    }
}

fn interpolated_brush_points(
    from: Option<(f32, f32, f32)>,
    to: (f32, f32, f32),
    aspect: f32,
    shape: &str,
    color: slint::Color,
) -> Vec<BrushPoint> {
    let shape = if shape == "square" {
        "square"
    } else {
        "circle"
    };
    let clamp_point = |(x, y, size): (f32, f32, f32)| {
        (
            if x.is_finite() {
                x.clamp(0.0, 1.0)
            } else {
                0.0
            },
            if y.is_finite() {
                y.clamp(0.0, 1.0)
            } else {
                0.0
            },
            if size.is_finite() {
                size.clamp(0.002, 0.5)
            } else {
                0.02
            },
        )
    };
    let to = clamp_point(to);
    let Some(from) = from.map(clamp_point) else {
        return vec![BrushPoint {
            x: to.0,
            y: to.1,
            size: to.2,
            shape: shape.into(),
            color,
        }];
    };

    let safe_aspect = if aspect.is_finite() {
        aspect.clamp(0.05, 20.0)
    } else {
        1.0
    };
    let dx = to.0 - from.0;
    let dy = (to.1 - from.1) / safe_aspect;
    let distance = (dx * dx + dy * dy).sqrt();
    let spacing = (to.2 * 0.32).max(0.0005);
    let steps = ((distance / spacing).ceil() as usize).clamp(1, 512);

    (1..=steps)
        .map(|index| {
            let progress = index as f32 / steps as f32;
            BrushPoint {
                x: from.0 + (to.0 - from.0) * progress,
                y: from.1 + (to.1 - from.1) * progress,
                size: from.2 + (to.2 - from.2) * progress,
                shape: shape.into(),
                color,
            }
        })
        .collect()
}

#[cfg(test)]
mod image_editor_tests {
    use super::*;

    #[test]
    fn brush_segments_are_interpolated_without_large_gaps() {
        let color = slint::Color::from_rgb_u8(36, 184, 255);
        let points = interpolated_brush_points(
            Some((0.1, 0.2, 0.02)),
            (0.9, 0.2, 0.02),
            1.0,
            "square",
            color,
        );

        assert!(points.len() > 20);
        assert!((points.last().unwrap().x - 0.9).abs() < f32::EPSILON);
        assert!(points.iter().all(|point| {
            (0.0..=1.0).contains(&point.x)
                && (0.0..=1.0).contains(&point.y)
                && point.size > 0.0
                && point.shape == "square"
                && point.color == color
        }));
    }

    #[test]
    fn unknown_brush_shape_falls_back_to_circle() {
        let color = slint::Color::from_rgb_u8(255, 77, 79);
        let points = interpolated_brush_points(None, (0.5, 0.5, 0.02), 1.0, "triangle", color);

        assert_eq!(points.len(), 1);
        assert_eq!(points[0].shape, "circle");
        assert_eq!(points[0].color, color);
    }
}

pub(super) fn add_reference_from_drag_data(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    drag_data_to_paths(data)
        .into_iter()
        .fold(false, |added, path| {
            add_reference_from_path(app, store, &path) || added
        })
}

pub(super) fn drag_data_to_path(data: &str) -> Option<PathBuf> {
    drag_data_to_paths(data).into_iter().next()
}

pub(super) fn drag_data_to_paths(data: &str) -> Vec<PathBuf> {
    data.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(drag_line_to_path)
        .collect()
}

fn drag_line_to_path(raw: &str) -> Option<PathBuf> {
    if raw
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        let url = reqwest::Url::parse(raw).ok()?;
        if let Ok(path) = url.to_file_path() {
            return Some(path);
        }
        let decoded = percent_decode_path(url.path());
        #[cfg(windows)]
        let decoded = decoded.trim_start_matches('/').replace('/', "\\");
        return Some(PathBuf::from(decoded));
    }

    let decoded = percent_decode_path(raw);
    #[cfg(windows)]
    let decoded = decoded.trim_start_matches('/').replace('/', "\\");
    Some(PathBuf::from(decoded))
}

pub(super) fn external_image_url(data: &str) -> Option<String> {
    let html_source = data
        .split_once("src=\"")
        .and_then(|(_, tail)| tail.split_once('"').map(|(value, _)| value))
        .or_else(|| {
            data.split_once("src='")
                .and_then(|(_, tail)| tail.split_once('\'').map(|(value, _)| value))
        });
    let candidates = html_source.into_iter().chain(
        data.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| {
                line.strip_prefix("SourceURL:")
                    .map(str::trim)
                    .or_else(|| line.starts_with("http").then_some(line))
            }),
    );
    for candidate in candidates {
        let Ok(url) = reqwest::Url::parse(candidate) else {
            continue;
        };
        if matches!(url.scheme(), "http" | "https") && url.username().is_empty() {
            return Some(url.to_string());
        }
    }
    None
}

pub(super) fn file_uri_for_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "failed" {
        return String::new();
    }
    #[cfg(windows)]
    {
        let normalized = path.replace('\\', "/");
        let encoded = percent_encode_uri_path(&normalized);
        if encoded.starts_with("//") {
            format!("file:{encoded}")
        } else {
            format!("file:///{encoded}")
        }
    }
    #[cfg(not(windows))]
    {
        let encoded = percent_encode_uri_path(path);
        if encoded.starts_with('/') {
            format!("file://{encoded}")
        } else {
            format!("file:///{encoded}")
        }
    }
}

pub(super) fn percent_encode_uri_path(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                output.push(*byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

pub(super) fn percent_decode_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

pub(super) fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub(super) fn add_reference_from_path(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    path: &Path,
) -> bool {
    let state = app.global::<AppState>();
    if !path.exists() {
        state.set_generation_status("参考图文件不存在".into());
        return false;
    }
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    let max_references = max_reference_images_for_category(&category);
    {
        let mut store_mut = store.borrow_mut();
        let references = references_for_category_mut(&mut store_mut.references, &category);
        if references.len() >= max_references {
            state.set_generation_status(reference_limit_message(max_references).into());
            return true;
        }
    }

    let Ok(image) = load_image(path) else {
        state.set_generation_status(
            format!(
                "无法读取参考图，支持：{}",
                crate::image_formats::supported_image_formats_label()
            )
            .into(),
        );
        return false;
    };
    let source_path = match persist_reference_source(path) {
        Ok(path) => path.display().to_string(),
        Err(_) => {
            state.set_generation_status("无法保存参考图，请检查磁盘空间和文件权限".into());
            return false;
        }
    };
    let mut store_mut = store.borrow_mut();
    let references = references_for_category_mut(&mut store_mut.references, &category);
    if references.len() >= max_references {
        state.set_generation_status(reference_limit_message(max_references).into());
        return true;
    }
    references.push(ReferenceData {
        id: Uuid::new_v4().to_string(),
        image,
        source_path,
    });
    push_references(app, &store_mut);
    state.set_generation_status("已添加参考图".into());
    true
}
