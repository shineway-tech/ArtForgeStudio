use super::*;

pub(super) fn wire_viewer_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();

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
            drag_preview::start_thumbnail_file_drag(path)
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_viewer_cutout_image(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_viewer_image_processing(&app, store.clone(), ProcessImageMode::Cutout);
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
        let context = context.clone();
        state.on_viewer_regenerate(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let prompt = app.global::<AppState>().get_viewer_prompt().to_string();
            app.global::<AppState>().set_viewer_open(false);
            start_generation(&app, context.clone(), Some(prompt), false, None, None);
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
                        source_path: String::new(),
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
            {
                let mut store_mut = store.borrow_mut();
                references_for_category_mut(&mut store_mut.references, &category).push(
                    ReferenceData {
                        id: Uuid::new_v4().to_string(),
                        image: state.get_viewer_image(),
                        source_path: String::new(),
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

pub(super) fn add_reference_from_drag_data(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mime_type: &str,
    data: &str,
) -> bool {
    if mime_type != URI_LIST_MIME && mime_type != TEXT_PLAIN_MIME && mime_type != IMAGE_DRAG_MIME {
        return false;
    }
    let Some(path) = drag_data_to_path(data) else {
        return false;
    };
    add_reference_from_path(app, store, &path)
}

pub(super) fn drag_data_to_path(data: &str) -> Option<PathBuf> {
    let raw = data
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let raw = if let Some(rest) = raw.strip_prefix("file:///") {
        rest
    } else if let Some(rest) = raw.strip_prefix("file://") {
        rest
    } else {
        raw
    };
    let decoded = percent_decode_path(raw);
    #[cfg(windows)]
    let decoded = decoded.trim_start_matches('/').replace('/', "\\");
    Some(PathBuf::from(decoded))
}

pub(super) fn external_image_url(data: &str) -> Option<String> {
    let raw = data
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let candidate = if raw.starts_with('<') {
        let source = raw
            .split_once("src=\"")
            .and_then(|(_, tail)| tail.split_once('"').map(|(value, _)| value))
            .or_else(|| {
                raw.split_once("src='")
                    .and_then(|(_, tail)| tail.split_once('\'').map(|(value, _)| value))
            })?;
        source
    } else {
        raw
    };
    let url = reqwest::Url::parse(candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
        return None;
    }
    Some(url.to_string())
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

pub(super) fn add_reference_from_path(app: &AppWindow, store: &Rc<RefCell<Store>>, path: &Path) -> bool {
    let state = app.global::<AppState>();
    if !path.exists() {
        state.set_generation_status("参考图文件不存在".into());
        return false;
    }
    let category = resolve_category(&state.get_asset_type().to_string(), "");
    let max_references = max_reference_images_for_category(&category);
    let Ok(image) = load_image(path) else {
        state.set_generation_status("无法读取参考图".into());
        return false;
    };
    let mut store = store.borrow_mut();
    let references = references_for_category_mut(&mut store.references, &category);
    if references.len() >= max_references {
        state.set_generation_status(reference_limit_message(max_references).into());
        return true;
    }
    references.push(ReferenceData {
        id: Uuid::new_v4().to_string(),
        image,
        source_path: path.display().to_string(),
    });
    push_references(app, &store);
    state.set_generation_status("已添加参考图".into());
    true
}
