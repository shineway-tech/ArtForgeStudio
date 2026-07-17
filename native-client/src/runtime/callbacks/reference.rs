use super::*;

pub(super) fn wire_reference_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(files) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_files()
            {
                let category =
                    resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
                let max_references = max_reference_images_for_category(&category);
                let mut store = store.borrow_mut();
                let references = references_for_category_mut(&mut store.references, &category);
                if references.len() >= max_references {
                    app.global::<AppState>()
                        .set_generation_status(reference_limit_message(max_references).into());
                    return;
                }
                for path in files {
                    if references.len() >= max_references {
                        break;
                    }
                    if let Ok(image) = load_image(&path) {
                        references.push(ReferenceData {
                            id: Uuid::new_v4().to_string(),
                            image,
                            source_path: path.display().to_string(),
                        });
                    }
                }
                if references.len() >= max_references {
                    app.global::<AppState>()
                        .set_generation_status(reference_limit_message(max_references).into());
                }
                push_references(&app, &store);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_paste_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let state = app.global::<AppState>();
            let category = resolve_category(&state.get_asset_type().to_string(), "");
            let max_references = max_reference_images_for_category(&category);
            let Ok(mut clipboard) = arboard::Clipboard::new() else {
                return false;
            };
            let Ok(img) = clipboard.get_image() else {
                return false;
            };
            let mut store = store.borrow_mut();
            let references = references_for_category_mut(&mut store.references, &category);
            if references.len() >= max_references {
                state.set_generation_status(reference_limit_message(max_references).into());
                return true;
            }
            let image = image_from_clipboard(img);
            references.push(ReferenceData {
                id: Uuid::new_v4().to_string(),
                image,
                source_path: String::new(),
            });
            push_references(&app, &store);
            state.set_generation_status("已从剪贴板粘贴参考图".into());
            true
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_reference_from_drag(move |mime_type, data| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            add_reference_from_drag_data(&app, &store, mime_type.as_str(), data.as_str())
        });
    }

    state.on_start_thumbnail_drag_preview(move |data| {
        let Some(path) = drag_data_to_path(data.as_str()) else {
            return false;
        };
        drag_preview::start_thumbnail_drag_preview(path)
    });

    state.on_start_thumbnail_file_drag(move |data| {
        let Some(path) = drag_data_to_path(data.as_str()) else {
            return false;
        };
        drag_preview::start_thumbnail_file_drag(path)
    });

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_remove_reference(move |id| {
            if let Some(app) = app_weak.upgrade() {
                let id = id.to_string();
                let category =
                    resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
                references_for_category_mut(&mut store.borrow_mut().references, &category)
                    .retain(|r| r.id != id);
                push_references(&app, &store.borrow());
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_reference(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            let category =
                resolve_category(&app.global::<AppState>().get_asset_type().to_string(), "");
            let store_ref = store.borrow();
            let Some(item) = references_for_category(&store_ref.references, &category)
                .iter()
                .find(|r| r.id == id)
                .cloned()
            else {
                return;
            };
            let state = app.global::<AppState>();
            state.set_viewer_id(item.id.into());
            state.set_viewer_source("reference".into());
            state.set_viewer_image(item.image);
            state.set_viewer_title("参考图".into());
            state.set_viewer_prompt("".into());
            state.set_viewer_prompt_lines(1);
            state.set_viewer_time("".into());
            state.set_viewer_ratio("".into());
            state.set_viewer_quality("".into());
            state.set_viewer_model("".into());
            state.set_viewer_width(0);
            state.set_viewer_height(0);
            state.set_viewer_cutout_done(false);
            state.set_viewer_remove_black_done(false);
            state.set_viewer_upscale_done(false);
            state.set_viewer_open(true);
        });
    }
}
